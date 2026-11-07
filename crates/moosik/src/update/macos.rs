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
    cell::{Cell, RefCell},
    collections::VecDeque,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

pub(crate) enum Event {
    Available(String),
    ShowOffer,
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
    updater: Retained<AnyObject>,
    feed_url: String,
}
thread_local! {
    static NATIVE: RefCell<Option<Native>> = const { RefCell::new(None) };
    static REQUEST: RefCell<Option<InstallRequest>> = const { RefCell::new(None) };
    static DIRECT_INSTALL: Cell<bool> = const { Cell::new(false) };
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
    // Canceling a staged installer finishes its Sparkle cycle before its XPC
    // service exits. Allow that service to withdraw before looking for a new item.
    let due = REQUEST.with(|r| r.borrow().as_ref().and_then(|r| r.check_at));
    if let Some(due) = due {
        let wait = due.saturating_duration_since(Instant::now());
        if wait.is_zero() {
            if !session_busy() {
                REQUEST.with(|r| {
                    if let Some(r) = r.borrow_mut().as_mut() {
                        r.check_at = None;
                    }
                });
                check();
            }
        } else if let Some(ctx) = CONTEXT.get() {
            ctx.request_repaint_after(wait);
        }
    }
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
extern "C" fn valid(_: &AnyObject, _: Sel, updater: &AnyObject, item: &AnyObject) {
    let downloading: Bool = unsafe { msg_send![updater, automaticallyDownloadsUpdates] };
    if !downloading.as_bool() {
        found(item);
    }
}
extern "C" fn prepared(
    _: &AnyObject,
    _: Sel,
    _: &AnyObject,
    item: &AnyObject,
    _: &Block<dyn Fn()>,
) -> Bool {
    // Release the automatic update session. Sparkle keeps the verified installer staged
    // and presents it through its user driver on the next interactive check.
    found(item);
    Bool::NO
}

struct InstallRequest {
    version: String,
    skipped_stale: bool,
    check_at: Option<Instant>,
}

pub(crate) fn install_in_progress() -> bool {
    DIRECT_INSTALL.get() || REQUEST.with(|slot| slot.borrow().is_some())
}

pub(crate) fn install_latest(version: String) {
    if install_in_progress() {
        return;
    }
    REQUEST.with(|slot| {
        *slot.borrow_mut() = Some(InstallRequest {
            version,
            skipped_stale: false,
            check_at: Some(Instant::now()),
        })
    });
    if let Some(ctx) = CONTEXT.get() {
        ctx.request_repaint();
    }
}

fn session_busy() -> bool {
    NATIVE.with(|n| {
        n.borrow().as_ref().is_some_and(|n| unsafe {
            let busy: Bool = msg_send![&*n.updater, sessionInProgress];
            busy.as_bool()
        })
    })
}

#[derive(Debug, PartialEq, Eq)]
enum OfferAction {
    Notify,
    Install,
    SkipStale,
    Changed,
}

fn select_offer(request: &mut Option<InstallRequest>, version: &str) -> OfferAction {
    let Some(target) = request else {
        return OfferAction::Notify;
    };
    if target.version == version {
        *request = None;
        OfferAction::Install
    } else if !target.skipped_stale {
        target.skipped_stale = true;
        target.check_at = Some(Instant::now() + Duration::from_secs(2));
        OfferAction::SkipStale
    } else {
        *request = None;
        OfferAction::Changed
    }
}

// Sparkle may resume a staged installer without fetching its appcast. Its public Skip
// response discards that installer. Once the cycle finishes, ask again for the live item.
extern "C" fn offer(
    driver: &AnyObject,
    _: Sel,
    item: &AnyObject,
    state: &AnyObject,
    reply: &Block<dyn Fn(isize)>,
) {
    let version: Retained<NSString> = unsafe { msg_send![item, versionString] };
    let action = REQUEST.with(|slot| select_offer(&mut slot.borrow_mut(), &version.to_string()));
    match action {
        OfferAction::SkipStale => reply.call((0,)),
        OfferAction::Changed => {
            reply.call((2,));
            emit(Event::Failed(
                "The release feed changed. Click Update to check again.".into(),
            ));
        }
        action => {
            let info: Bool = unsafe { msg_send![item, isInformationOnlyUpdate] };
            let major: Bool = unsafe { msg_send![item, isMajorUpgrade] };
            let critical: Bool = unsafe { msg_send![item, isCriticalUpdate] };
            let signing: isize = unsafe { msg_send![item, signingValidationStatus] };
            if info.as_bool() || major.as_bool() || critical.as_bool() || signing == 2 {
                let parent =
                    AnyClass::get(c"SPUStandardUserDriver").expect("loaded Sparkle user driver");
                unsafe {
                    let _: () = msg_send![super(driver, parent), showUpdateFoundWithAppcastItem: item, state: state, reply: reply];
                }
            } else if matches!(action, OfferAction::Install) {
                DIRECT_INSTALL.set(true);
                reply.call((1,));
            } else {
                found(item);
                let manual: Bool = unsafe { msg_send![state, userInitiated] };
                if manual.as_bool() {
                    emit(Event::ShowOffer);
                }
                reply.call((2,));
            }
        }
    }
}

extern "C" fn ready(driver: &AnyObject, _: Sel, reply: &Block<dyn Fn(isize)>) {
    if DIRECT_INSTALL.get() {
        reply.call((1,));
    } else {
        let parent = AnyClass::get(c"SPUStandardUserDriver").expect("loaded Sparkle user driver");
        unsafe {
            let _: () = msg_send![super(driver, parent), showReadyToInstallAndRelaunch: reply];
        }
    }
}

fn user_driver_class() -> Result<&'static AnyClass> {
    static CLASS: OnceLock<&'static AnyClass> = OnceLock::new();
    let parent =
        AnyClass::get(c"SPUStandardUserDriver").context("Sparkle user driver is missing")?;
    Ok(CLASS.get_or_init(|| {
        let mut class =
            ClassBuilder::new(c"MoosikUpdateUserDriver", parent).expect("unique user driver");
        unsafe {
            class.add_method(
                sel!(showReadyToInstallAndRelaunch:),
                ready as extern "C" fn(_, _, _),
            );
            class.add_method(
                sel!(showUpdateFoundWithAppcastItem:state:reply:),
                offer as extern "C" fn(_, _, _, _, _),
            );
        }
        class.register()
    }))
}
extern "C" fn showing(_: &AnyObject, _: Sel, _: Bool, item: &AnyObject, _: &AnyObject) {
    found(item);
}
extern "C" fn current(_: &AnyObject, _: Sel, _: &AnyObject) {
    REQUEST.with(|slot| slot.borrow_mut().take());
    emit(Event::Current);
}
extern "C" fn failed(_: &AnyObject, _: Sel, _: &AnyObject, error: &NSError) {
    // Sparkle also reports SUNoUpdateError after the ordinary "current" callback.
    if error.code() == 1001 && error.domain().to_string() == "SUSparkleErrorDomain" {
        return;
    }
    // Sparkle presents manual errors itself; the app records the status without a second alert.
    REQUEST.with(|slot| slot.borrow_mut().take());
    emit(Event::Failed(error.localizedDescription().to_string()));
}
extern "C" fn finished(_: &AnyObject, _: Sel, _: &AnyObject, _: isize, _: *const NSError) {
    DIRECT_INSTALL.set(false);
    REQUEST.with(|r| {
        if r.borrow().as_ref().is_some_and(|r| r.check_at.is_none()) {
            r.borrow_mut().take();
        }
    });
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
    if choice == 0 && REQUEST.with(|slot| slot.borrow().is_none()) {
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
                sel!(updater:willInstallUpdateOnQuit:immediateInstallationBlock:),
                prepared as extern "C" fn(_, _, _, _, _) -> _,
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
    let class = AnyClass::get(c"SPUUpdater").context("Sparkle updater class is missing")?;
    let delegate: Retained<AnyObject> = unsafe { msg_send![delegate_class(), new] };
    // Preserve an existing Moosik opt-out on the first Sparkle-enabled launch.
    // Thereafter Sparkle's persisted preference is authoritative.
    let defaults = NSUserDefaults::standardUserDefaults();
    let preference = NSString::from_str("SUEnableAutomaticChecks");
    if !legacy_automatic && defaults.objectForKey(&preference).is_none() {
        defaults.setBool_forKey(false, &preference);
    }
    let driver: Retained<AnyObject> = unsafe {
        msg_send![msg_send![user_driver_class()?, alloc], initWithHostBundle: &*bundle, delegate: &*delegate]
    };
    let updater: Retained<AnyObject> = unsafe {
        msg_send![msg_send![class, alloc], initWithHostBundle: &*bundle,
            applicationBundle: &*bundle, userDriver: &*driver, delegate: &*delegate]
    };
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
            updater,
            feed_url,
        })
    });
    check_in_background();
    Ok(true)
}

/// Sparkle owns the session, preference and staged installer. Never interrupt them to poll.
pub(crate) fn check_in_background() {
    if REQUEST.with(|slot| slot.borrow().is_some()) {
        return;
    }
    NATIVE.with(|n| {
        if let Some(native) = n.borrow().as_ref() {
            unsafe {
                let updater = &native.updater;
                let automatic: Bool = msg_send![&**updater, automaticallyChecksForUpdates];
                let busy: Bool = msg_send![&**updater, sessionInProgress];
                if automatic.as_bool() && !busy.as_bool() {
                    let _: () = msg_send![&**updater, checkForUpdatesInBackground];
                }
            }
        }
    });
}

pub(crate) fn feed_url() -> Option<String> {
    NATIVE.with(|n| n.borrow().as_ref().map(|native| native.feed_url.clone()))
}
pub(crate) fn check() {
    let updater = NATIVE.with(|n| n.borrow().as_ref().map(|n| n.updater.clone()));
    if let Some(updater) = updater {
        unsafe {
            let _: () = msg_send![&*updater, checkForUpdates];
        }
    }
}
pub(crate) fn automatic_checks() -> Option<bool> {
    NATIVE.with(|n| {
        n.borrow().as_ref().map(|native| unsafe {
            let updater = &native.updater;
            let enabled: Bool = msg_send![&**updater, automaticallyChecksForUpdates];
            enabled.as_bool()
        })
    })
}
pub(crate) fn set_automatic_checks(enabled: bool) {
    NATIVE.with(|n| {
        if let Some(native) = n.borrow().as_ref() {
            unsafe {
                let updater = &native.updater;
                let _: () =
                    msg_send![&**updater, setAutomaticallyChecksForUpdates: Bool::new(enabled)];
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discard_one_superseded_installer_and_only_accept_the_refreshed_version() {
        let request = || {
            Some(InstallRequest {
                version: "0.1.2".into(),
                skipped_stale: false,
                check_at: None,
            })
        };
        let mut target = request();
        assert_eq!(select_offer(&mut target, "0.1.1"), OfferAction::SkipStale);
        assert!(target.as_ref().unwrap().check_at.is_some());
        assert_eq!(select_offer(&mut target, "0.1.2"), OfferAction::Install);
        assert!(target.is_none());
        let mut target = request();
        assert_eq!(select_offer(&mut target, "0.1.1"), OfferAction::SkipStale);
        assert_eq!(select_offer(&mut target, "0.1.1"), OfferAction::Changed);
        assert_eq!(select_offer(&mut target, "0.1.1"), OfferAction::Notify);
    }
}
