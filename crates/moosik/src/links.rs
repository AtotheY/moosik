//! Strictly scoped playback links from moosik.dev. Only an opaque song or room ID crosses
//! the OS handler boundary. A remote link cannot supply a file path, command or network host.

use std::collections::VecDeque;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Link {
    Play(String),
    Join(String),
}

impl Link {
    pub fn parse(raw: &str) -> Option<Self> {
        if let Some(id) = raw.strip_prefix("moosik://play/") {
            return (id.len() == 11
                && id
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'))
            .then(|| Self::Play(id.to_owned()));
        }
        let room = raw.strip_prefix("moosik://join/")?;
        (room.len() == 32
            && room
                .bytes()
                .all(|c| c.is_ascii_digit() || matches!(c, b'a'..=b'f')))
        .then(|| Self::Join(room.to_owned()))
    }
}

static QUEUE: Mutex<VecDeque<Link>> = Mutex::new(VecDeque::new());
static CTX: OnceLock<egui::Context> = OnceLock::new();

fn enqueue(link: Link) {
    let mut queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    if queue.len() >= 32 {
        queue.pop_front();
    }
    queue.push_back(link);
    drop(queue);
    if let Some(ctx) = CTX.get() {
        #[cfg(target_os = "macos")]
        crate::tray::inject(ctx, crate::tray::TrayAction::Show);
        #[cfg(not(target_os = "macos"))]
        ctx.request_repaint();
    }
}

pub fn take() -> Option<Link> {
    QUEUE.lock().unwrap_or_else(|e| e.into_inner()).pop_front()
}

// Shells on Windows and Linux start a second process. Persist validated requests before the
// process signals the existing single-instance event. This also covers consecutive activations
// while the first process is still opening its window. Disk access happens only on a launch.
fn inbox() -> Option<PathBuf> {
    Some(crate::profile_dirs()?.cache_dir().join("link-inbox"))
}

fn write_request(dir: &Path, raw: &str) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)?;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = dir.join(format!("{nanos:039}-{}.link", std::process::id()));
    crate::persist::write_atomic(&path, raw.as_bytes())
}

/// Return whether the command line was a link launch. Reject additional arguments before
/// the general CLI parser can interpret them as flags.
pub fn accept_cli(args: &[String]) -> io::Result<bool> {
    let values = match args {
        [flag, rest @ ..] if flag == "--open-url" => rest,
        [first, ..] if first.starts_with("moosik:") => args,
        _ => return Ok(false),
    };
    let [raw] = values else {
        return Err(io::Error::other("expected one Moosik URL"));
    };
    Link::parse(raw).ok_or_else(|| io::Error::other("invalid Moosik URL"))?;
    let dir = inbox().ok_or_else(|| io::Error::other("could not locate the link inbox"))?;
    write_request(&dir, raw)?;
    Ok(true)
}

fn read_requests(dir: &Path, mut emit: impl FnMut(Link)) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut entries = entries.flatten().collect::<Vec<_>>();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "link") {
            continue;
        }
        let valid = entry.file_type().is_ok_and(|ty| ty.is_file())
            && entry.metadata().is_ok_and(|meta| {
                meta.len() <= 64
                    && meta
                        .modified()
                        .and_then(|t| t.elapsed().map_err(io::Error::other))
                        .is_ok_and(|age| age < Duration::from_secs(120))
            });
        let link = valid
            .then(|| fs::read_to_string(&path).ok())
            .flatten()
            .and_then(|raw| Link::parse(&raw));
        if fs::remove_file(&path).is_ok()
            && let Some(link) = link
        {
            emit(link);
        }
    }
}

/// Called when an OS launch brings the primary window forward.
pub fn refresh() {
    if CTX.get().is_some()
        && let Some(dir) = inbox()
    {
        read_requests(&dir, enqueue);
    }
}

pub fn attach(ctx: &egui::Context) {
    let _ = CTX.set(ctx.clone());
    refresh();
}

#[cfg(target_os = "macos")]
pub mod macos {
    use objc2::{
        MainThreadOnly, class, define_class, msg_send, rc::Retained, runtime::AnyObject, sel,
    };
    use objc2_app_kit::NSApplicationWillFinishLaunchingNotification;
    use objc2_foundation::{
        MainThreadMarker, NSNotification, NSNotificationCenter, NSObject, NSString,
    };
    use std::cell::OnceCell;

    define_class!(
        #[unsafe(super = NSObject)]
        #[thread_kind = MainThreadOnly]
        struct LinkDelegate;

        impl LinkDelegate {
            #[unsafe(method(moosikHandleURL:withReplyEvent:))]
            fn handle_url(&self, event: &AnyObject, _: Option<&AnyObject>) {
                // NSAppleEventDescriptor's direct-object parameter holds the incoming URL.
                let direct: Option<Retained<AnyObject>> = unsafe {
                    msg_send![event, paramDescriptorForKeyword: u32::from_be_bytes(*b"----")]
                };
                let value: Option<Retained<NSString>> = direct.and_then(|param| unsafe { msg_send![&*param, stringValue] });
                if let Some(link) = value.and_then(|url| super::Link::parse(&url.to_string())) {
                    super::enqueue(link);
                }
            }

            #[unsafe(method(moosikWillLaunch:))]
            fn will_launch(&self, _: &NSNotification) {
                // Install a handler for Get URL before LaunchServices delivers the initial event.
                // Winit retains its own NSApplication delegate and all app lifecycle events.
                let get_url = u32::from_be_bytes(*b"GURL");
                unsafe {
                    let manager: Retained<AnyObject> = msg_send![class!(NSAppleEventManager), sharedAppleEventManager];
                    let _: () = msg_send![&*manager, setEventHandler: self,
                        andSelector: sel!(moosikHandleURL:withReplyEvent:),
                        forEventClass: get_url, andEventID: get_url];
                }
            }
        }
    );

    thread_local! {
        static DELEGATE: OnceCell<Retained<LinkDelegate>> = const { OnceCell::new() };
    }
    pub fn install() {
        let mtm = MainThreadMarker::new().expect("URL handler installed on main thread");
        DELEGATE.with(|slot| {
            slot.get_or_init(|| unsafe {
                let delegate: Retained<LinkDelegate> = msg_send![LinkDelegate::alloc(mtm), init];
                NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                    &delegate,
                    sel!(moosikWillLaunch:),
                    Some(NSApplicationWillFinishLaunchingNotification),
                    None,
                );
                delegate
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_canonical_play_and_join_links() {
        assert_eq!(
            Link::parse("moosik://play/dQw4w9WgXcQ"),
            Some(Link::Play("dQw4w9WgXcQ".into()))
        );
        let room = "0123456789abcdef0123456789abcdef";
        assert_eq!(
            Link::parse(&format!("moosik://join/{room}")),
            Some(Link::Join(room.into()))
        );
        for raw in [
            "moosik://play/123",
            "moosik://play/dQw4w9WgXcQ?t=9",
            "moosik://play/../../file",
            "moosik://play/abcdefghij%",
            "moosik://join/0123456789abcdef0123456789abcdeF",
            "https://play/dQw4w9WgXcQ",
        ] {
            assert_eq!(Link::parse(raw), None);
        }
        assert!(
            accept_cli(&[
                "--open-url".into(),
                "moosik://play/dQw4w9WgXcQ".into(),
                "--search".into()
            ])
            .is_err()
        );
    }

    #[test]
    fn handoff_consumes_each_valid_file_once_and_ignores_partial_writes() {
        let dir = std::env::temp_dir().join(format!("moosik-links-test-{}", std::process::id()));
        write_request(&dir, "moosik://play/dQw4w9WgXcQ").unwrap();
        fs::write(dir.join("invalid.link"), "moosik://play/../../file").unwrap();
        fs::write(dir.join("pending.tmp"), "moosik://play/01234567890").unwrap();
        let mut found = Vec::new();
        read_requests(&dir, |link| found.push(link));
        read_requests(&dir, |link| found.push(link));
        assert_eq!(found, vec![Link::Play("dQw4w9WgXcQ".into())]);
        assert!(dir.join("pending.tmp").is_file());
        fs::remove_dir_all(dir).unwrap();
    }
}
