//! Sparkle 2 owns download verification, installation, authorization and relaunch.
//! Loaded from our signed bundle; source builds and unconfigured bundles stay manual.
use anyhow::{Context, Result, anyhow, bail};
use block2::{Block, RcBlock};
use objc2::{
    ClassType, msg_send,
    rc::Retained,
    runtime::{AnyClass, AnyObject, AnyProtocol, Bool, ClassBuilder, Sel},
    sel,
};
use objc2_foundation::{NSBundle, NSError, NSObject, NSString, NSUserDefaults};
use std::{
    cell::RefCell,
    collections::VecDeque,
    sync::{Mutex, OnceLock},
};

pub(crate) enum Event {
    Available(String),
    Current,
    Skipped,
    Finished,
    Failed(String),
    Relaunch,
}
static EVENTS: Mutex<VecDeque<Event>> = Mutex::new(VecDeque::new());
static CONTEXT: OnceLock<egui::Context> = OnceLock::new();
struct Native {
    _framework: Retained<NSBundle>,
    _delegate: Retained<AnyObject>,
    controller: Retained<AnyObject>,
    feed_url: String,
}
thread_local! {
    static NATIVE: RefCell<Option<Native>> = const { RefCell::new(None) };
    static RELAUNCH: RefCell<Option<RcBlock<dyn Fn()>>> = const { RefCell::new(None) };
}
fn emit(event: Event) {
    if let Ok(mut events) = EVENTS.lock() {
        events.push_back(event);
    }
    if let Some(ctx) = CONTEXT.get() {
        ctx.request_repaint();
    }
}
pub(crate) fn poll() -> Option<Event> {
    EVENTS.lock().ok()?.pop_front()
}

// These callbacks are registered with exactly the signatures in Sparkle's public headers.
extern "C" fn gentle(_: &AnyObject, _: Sel) -> Bool {
    Bool::YES
}
extern "C" fn handle_scheduled(_: &AnyObject, _: Sel, _: &AnyObject, _: Bool) -> Bool {
    Bool::NO
}
fn found(item: &AnyObject) {
    let version: Retained<NSString> = unsafe { msg_send![item, displayVersionString] };
    emit(Event::Available(version.to_string()));
}
extern "C" fn valid(_: &AnyObject, _: Sel, _: &AnyObject, item: &AnyObject) {
    found(item);
}
extern "C" fn showing(_: &AnyObject, _: Sel, _: Bool, item: &AnyObject, _: &AnyObject) {
    found(item);
}
extern "C" fn current(_: &AnyObject, _: Sel, _: &AnyObject) {
    emit(Event::Current);
}
extern "C" fn failed(_: &AnyObject, _: Sel, _: &AnyObject, error: &NSError) {
    // Sparkle presents manual errors itself; the app records the status without a second alert.
    emit(Event::Failed(error.localizedDescription().to_string()));
}
extern "C" fn finished(_: &AnyObject, _: Sel, _: &AnyObject, _: isize, _: *const NSError) {
    emit(Event::Finished);
}
extern "C" fn postpone(
    _: &AnyObject,
    _: Sel,
    _: &AnyObject,
    _: &AnyObject,
    handler: &Block<dyn Fn()>,
) -> Bool {
    RELAUNCH.with(|slot| *slot.borrow_mut() = Some(handler.copy()));
    emit(Event::Relaunch);
    Bool::YES
}
extern "C" fn choice(
    _: &AnyObject,
    _: Sel,
    _: &AnyObject,
    choice: isize,
    _: &AnyObject,
    _: &AnyObject,
) {
    // SPUUserUpdateChoice: Skip=0, Install=1, Dismiss=2 (public SPUUserUpdateState.h).
    if choice == 0 {
        emit(Event::Skipped);
    }
}
fn delegate_class() -> &'static AnyClass {
    static CLASS: OnceLock<&'static AnyClass> = OnceLock::new();
    CLASS.get_or_init(|| {
        let mut class = ClassBuilder::new(c"MoosikSparkleDelegate", NSObject::class())
            .expect("unique updater class");
        for name in [c"SPUUpdaterDelegate", c"SPUStandardUserDriverDelegate"] {
            if let Some(protocol) = AnyProtocol::get(name) {
                class.add_protocol(protocol);
            }
        }
        unsafe {
            class.add_method(
                sel!(supportsGentleScheduledUpdateReminders),
                gentle as extern "C" fn(_, _) -> _,
            );
            class.add_method(
                sel!(standardUserDriverShouldHandleShowingScheduledUpdate:andInImmediateFocus:),
                handle_scheduled as extern "C" fn(_, _, _, _) -> _,
            );
            class.add_method(
                sel!(standardUserDriverWillHandleShowingUpdate:forUpdate:state:),
                showing as extern "C" fn(_, _, _, _, _),
            );
            class.add_method(
                sel!(updater:userDidMakeChoice:forUpdate:state:),
                choice as extern "C" fn(_, _, _, _, _, _),
            );
            class.add_method(
                sel!(updater:didFindValidUpdate:),
                valid as extern "C" fn(_, _, _, _),
            );
            class.add_method(
                sel!(updaterDidNotFindUpdate:),
                current as extern "C" fn(_, _, _),
            );
            class.add_method(
                sel!(updater:didAbortWithError:),
                failed as extern "C" fn(_, _, _, _),
            );
            class.add_method(
                sel!(updater:didFinishUpdateCycleForUpdateCheck:error:),
                finished as extern "C" fn(_, _, _, _, _),
            );
            class.add_method(
                sel!(updater:shouldPostponeRelaunchForUpdate:untilInvokingBlock:),
                postpone as extern "C" fn(_, _, _, _, _) -> _,
            );
        }
        class.register()
    })
}

/// Main-thread only. A missing feed/key is a development build, never a reason to
/// fall back to an unsigned installer or a different application's update feed.
pub(crate) fn start(ctx: &egui::Context, legacy_automatic: bool) -> Result<bool> {
    if NATIVE.with(|n| n.borrow().is_some()) {
        return Ok(true);
    }
    let bundle = NSBundle::mainBundle();
    let feed = bundle
        .objectForInfoDictionaryKey(&NSString::from_str("SUFeedURL"))
        .and_then(|value| value.downcast::<NSString>().ok());
    let key = bundle.objectForInfoDictionaryKey(&NSString::from_str("SUPublicEDKey"));
    if feed.is_none() || key.is_none() {
        return Ok(false);
    }
    let feed_url = feed.context("update feed is missing")?.to_string();
    let exe = std::env::current_exe()?;
    let contents = exe
        .parent()
        .and_then(|p| p.parent())
        .context("not in an app bundle")?;
    let path = contents.join("Frameworks/Sparkle.framework");
    let framework = NSBundle::bundleWithPath(&NSString::from_str(&path.to_string_lossy()))
        .context("Sparkle is missing")?;
    // The framework is part of the host's signed bundle; never search PATH or dlopen a remote file.
    if !unsafe { framework.load() } {
        bail!("could not load the bundled Sparkle framework");
    }
    let class = AnyClass::get(c"SPUStandardUpdaterController")
        .context("Sparkle updater class is missing")?;
    let delegate: Retained<AnyObject> = unsafe { msg_send![delegate_class(), new] };
    // Preserve an existing Moosik opt-out on the first Sparkle-enabled launch.
    // Thereafter Sparkle's persisted preference is authoritative.
    let defaults = NSUserDefaults::standardUserDefaults();
    let preference = NSString::from_str("SUEnableAutomaticChecks");
    if !legacy_automatic && defaults.objectForKey(&preference).is_none() {
        defaults.setBool_forKey(false, &preference);
    }
    let controller: Retained<AnyObject> = unsafe {
        msg_send![msg_send![class, alloc], initWithStartingUpdater: Bool::NO,
            updaterDelegate: &*delegate, userDriverDelegate: &*delegate]
    };
    let updater: Retained<AnyObject> = unsafe { msg_send![&*controller, updater] };
    let mut error: Option<Retained<NSError>> = None;
    let started: Bool = unsafe { msg_send![&*updater, startUpdater: &mut error] };
    if !started.as_bool() {
        return Err(anyhow!(
            "{}",
            error
                .map(|e| e.localizedDescription().to_string())
                .unwrap_or_else(|| "could not start Sparkle".into())
        ));
    }
    let _ = CONTEXT.set(ctx.clone());
    NATIVE.with(|n| {
        *n.borrow_mut() = Some(Native {
            _framework: framework,
            _delegate: delegate,
            controller,
            feed_url,
        })
    });
    Ok(true)
}
pub(crate) fn feed_url() -> Option<String> {
    NATIVE.with(|n| n.borrow().as_ref().map(|native| native.feed_url.clone()))
}
pub(crate) fn check() {
    NATIVE.with(|n| {
        if let Some(native) = n.borrow().as_ref() {
            unsafe {
                let _: () =
                    msg_send![&*native.controller, checkForUpdates: std::ptr::null::<AnyObject>()];
            }
        }
    });
}
pub(crate) fn automatic_checks() -> Option<bool> {
    NATIVE.with(|n| {
        n.borrow().as_ref().map(|native| unsafe {
            let updater: Retained<AnyObject> = msg_send![&*native.controller, updater];
            let enabled: Bool = msg_send![&*updater, automaticallyChecksForUpdates];
            enabled.as_bool()
        })
    })
}
pub(crate) fn set_automatic_checks(enabled: bool) {
    NATIVE.with(|n| {
        if let Some(native) = n.borrow().as_ref() {
            unsafe {
                let updater: Retained<AnyObject> = msg_send![&*native.controller, updater];
                let _: () =
                    msg_send![&*updater, setAutomaticallyChecksForUpdates: Bool::new(enabled)];
            }
        }
    });
}
/// Invoke only after synchronously saving the queue/settings on the UI thread.
pub(crate) fn continue_relaunch() {
    let handler = RELAUNCH.with(|slot| slot.borrow_mut().take());
    if let Some(handler) = handler {
        handler.call(());
    }
}
