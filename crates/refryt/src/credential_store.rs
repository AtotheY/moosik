//! Verified migration into a credential store. Tests use dummy sessions and an in-memory store.
use anyhow::{Context, Result, bail};
use std::path::Path;

pub(crate) trait Store {
    fn read(&self) -> Result<Option<Vec<u8>>>;
    fn write(&self, bytes: &[u8]) -> Result<()>;
    fn delete(&self) -> Result<()>;
}

pub(crate) fn load_migrating(
    store: &impl Store,
    legacy: &Path,
    validate: impl Fn(&[u8]) -> Result<()>,
) -> Result<Option<Vec<u8>>> {
    if let Some(bytes) = store.read()? {
        validate(&bytes)?;
        remove_legacy(legacy)?;
        return Ok(Some(bytes));
    }
    let bytes = match std::fs::read(legacy) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).context("could not read the legacy session"),
    };
    validate(&bytes)?;
    save_verified(store, legacy, &bytes)?;
    Ok(Some(bytes))
}

pub(crate) fn save_verified(store: &impl Store, legacy: &Path, bytes: &[u8]) -> Result<()> {
    store.write(bytes)?;
    if store.read()?.as_deref() != Some(bytes) {
        bail!("Keychain verification failed; the legacy session was preserved");
    }
    remove_legacy(legacy)
}

pub(crate) fn delete(store: &impl Store, legacy: &Path) -> Result<()> {
    // Remove the fallback first so a later launch cannot import it again.
    remove_legacy(legacy)?;
    store.delete()
}

fn remove_legacy(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).context("could not remove the legacy session file"),
    }
}

#[cfg(target_os = "macos")]
pub(crate) struct Keychain;
#[cfg(target_os = "macos")]
impl Store for Keychain {
    fn read(&self) -> Result<Option<Vec<u8>>> {
        match security_framework::passwords::get_generic_password(SERVICE, ACCOUNT) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.code() == -25300 => Ok(None), // errSecItemNotFound only
            Err(e) => Err(e).context("could not unlock the Refryt session in macOS Keychain"),
        }
    }
    fn write(&self, bytes: &[u8]) -> Result<()> {
        security_framework::passwords::set_generic_password(SERVICE, ACCOUNT, bytes)
            .context("could not save the Refryt session in macOS Keychain")
    }
    fn delete(&self) -> Result<()> {
        match security_framework::passwords::delete_generic_password(SERVICE, ACCOUNT) {
            Ok(()) => Ok(()),
            Err(e) if e.code() == -25300 => Ok(()),
            Err(e) => Err(e).context("could not remove the Refryt session from macOS Keychain"),
        }
    }
}
#[cfg(target_os = "macos")]
const SERVICE: &str = "com.sistilli.refryt";
#[cfg(target_os = "macos")]
const ACCOUNT: &str = "youtube-session-v1";

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    #[derive(Default)]
    struct Fake {
        value: RefCell<Option<Vec<u8>>>,
        fail_read: bool,
        fail_write: bool,
        corrupt: bool,
    }
    impl Store for Fake {
        fn read(&self) -> Result<Option<Vec<u8>>> {
            if self.fail_read {
                bail!("locked")
            }
            Ok(self.value.borrow().clone())
        }
        fn write(&self, bytes: &[u8]) -> Result<()> {
            if self.fail_write {
                bail!("denied")
            }
            *self.value.borrow_mut() = Some(if self.corrupt {
                b"wrong".to_vec()
            } else {
                bytes.to_vec()
            });
            Ok(())
        }
        fn delete(&self) -> Result<()> {
            *self.value.borrow_mut() = None;
            Ok(())
        }
    }
    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let p = std::env::temp_dir().join(format!(
                "refryt-dummy-migration-{}-{}",
                std::process::id(),
                N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::write(&p, b"dummy-session").unwrap();
            Self(p)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    #[test]
    fn migration_reads_back_before_removing_plaintext() {
        let f = Fixture::new();
        let store = Fake::default();
        assert_eq!(
            load_migrating(&store, &f.0, |_| Ok(())).unwrap().unwrap(),
            b"dummy-session"
        );
        assert!(!f.0.exists());
        assert_eq!(store.read().unwrap().unwrap(), b"dummy-session");
        assert!(load_migrating(&store, &f.0, |_| Ok(())).unwrap().is_some());
    }
    #[test]
    fn migration_preserves_file_on_locked_denied_or_corrupt_store() {
        for store in [
            Fake {
                fail_read: true,
                ..Fake::default()
            },
            Fake {
                fail_write: true,
                ..Fake::default()
            },
            Fake {
                corrupt: true,
                ..Fake::default()
            },
        ] {
            let f = Fixture::new();
            assert!(load_migrating(&store, &f.0, |_| Ok(())).is_err());
            assert_eq!(std::fs::read(&f.0).unwrap(), b"dummy-session");
        }
    }
    #[test]
    fn invalid_legacy_session_is_not_migrated() {
        let f = Fixture::new();
        let store = Fake::default();
        assert!(load_migrating(&store, &f.0, |_| anyhow::bail!("invalid")).is_err());
        assert!(f.0.exists());
        assert!(store.read().unwrap().is_none());
    }
    #[test]
    fn sign_out_removes_both_copies_and_cannot_reimport() {
        let f = Fixture::new();
        let store = Fake::default();
        store.write(b"dummy-session").unwrap();
        delete(&store, &f.0).unwrap();
        assert!(load_migrating(&store, &f.0, |_| Ok(())).unwrap().is_none());
    }
    #[test]
    fn keychain_failure_never_creates_a_plaintext_fallback() {
        let f = Fixture::new();
        std::fs::remove_file(&f.0).unwrap();
        let store = Fake {
            fail_write: true,
            ..Fake::default()
        };
        assert!(save_verified(&store, &f.0, b"dummy-session").is_err());
        assert!(!f.0.exists());
    }
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "writes and deletes one isolated dummy Keychain entry"]
    fn macos_keychain_dummy_round_trip() {
        use security_framework::passwords::{
            delete_generic_password, get_generic_password, set_generic_password,
        };
        let service = format!("com.sistilli.refryt.test.{}", std::process::id());
        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = delete_generic_password(&self.0, "dummy-test");
            }
        }
        let _cleanup = Cleanup(service.clone());
        set_generic_password(&service, "dummy-test", b"dummy-only-no-user-data").unwrap();
        assert!(
            get_generic_password(&service, "dummy-test").unwrap() == b"dummy-only-no-user-data"
        );
        delete_generic_password(&service, "dummy-test").unwrap();
        assert_eq!(
            get_generic_password(&service, "dummy-test")
                .unwrap_err()
                .code(),
            -25300
        );
    }
}
