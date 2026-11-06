//! Explorer can render the taskbar group from shell metadata instead of WM_SETICON.
//! Keep the window, installer shortcuts, and pins from older releases on one identity.
use super::AppIcon;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{
    ffi::{OsStr, OsString},
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
};
use windows::{
    Win32::{
        Foundation::{HWND, PROPERTYKEY},
        Storage::EnhancedStorage::{
            PKEY_AppUserModel_ID, PKEY_AppUserModel_RelaunchCommand,
            PKEY_AppUserModel_RelaunchDisplayNameResource, PKEY_AppUserModel_RelaunchIconResource,
        },
        System::Com::{
            CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile, STGM_READWRITE,
            StructuredStorage::PROPVARIANT,
        },
        UI::Shell::PropertiesSystem::{
            IPropertyStore, PSCoerceToCanonicalValue, SHGetPropertyStoreForWindow,
        },
        UI::Shell::SetCurrentProcessExplicitAppUserModelID,
        UI::Shell::{IShellLinkW, SHCNE_UPDATEITEM, SHCNF_PATHW, SHChangeNotify, ShellLink},
    },
    core::{BSTR, HSTRING, Interface},
};

// Must match [Icons] in windows-installer.iss, independent of finish and version.
const APP_ID: &str = "Moosik.Desktop";

pub fn init_process_identity() -> windows::core::Result<()> {
    unsafe { SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(APP_ID)) }
}

fn set_string(
    store: &IPropertyStore,
    key: &PROPERTYKEY,
    text: &OsStr,
) -> windows::core::Result<()> {
    let mut value = PROPVARIANT::from(BSTR::from_wide(&text.encode_wide().collect::<Vec<_>>()));
    unsafe {
        PSCoerceToCanonicalValue(key, &mut value)?;
        store.SetValue(key, &value)
    }
}

fn window_store(window: &winit::window::Window) -> anyhow::Result<IPropertyStore> {
    let RawWindowHandle::Win32(raw) = window.window_handle()?.as_raw() else {
        anyhow::bail!("not a Windows window")
    };
    Ok(unsafe { SHGetPropertyStoreForWindow(HWND(raw.hwnd.get() as *mut _))? })
}

impl AppIcon {
    fn ico(self, exe: &Path) -> PathBuf {
        exe.with_file_name("icons")
            .join(format!("{}.ico", self.label().to_ascii_lowercase()))
    }

    pub(super) fn set_taskbar_group(self, window: &winit::window::Window) -> anyhow::Result<()> {
        let exe = std::env::current_exe()?;
        let ico = self.ico(&exe);
        if !ico.is_file() {
            anyhow::bail!("missing {}", ico.display())
        }
        let store = window_store(window)?;
        let mut command = OsString::from("\"");
        command.push(exe);
        command.push("\"");
        let mut resource = ico.into_os_string();
        resource.push(",0");
        set_string(&store, &PKEY_AppUserModel_RelaunchCommand, &command)?;
        set_string(
            &store,
            &PKEY_AppUserModel_RelaunchDisplayNameResource,
            OsStr::new("Moosik"),
        )?;
        set_string(&store, &PKEY_AppUserModel_RelaunchIconResource, &resource)?;
        // Setting the ID last tells Explorer to reread the complete group metadata.
        set_string(&store, &PKEY_AppUserModel_ID, OsStr::new(APP_ID))?;
        Ok(())
    }

    pub(crate) fn clear_taskbar_group(window: &winit::window::Window) {
        if let Ok(store) = window_store(window) {
            for key in [
                PKEY_AppUserModel_ID,
                PKEY_AppUserModel_RelaunchCommand,
                PKEY_AppUserModel_RelaunchDisplayNameResource,
                PKEY_AppUserModel_RelaunchIconResource,
            ] {
                let _ = unsafe { store.SetValue(&key, &PROPVARIANT::default()) };
            }
        }
    }

    pub(crate) fn refresh_pin(self) {
        if let Err(error) = refresh(self) {
            tracing::debug!(%error, "could not refresh pinned Moosik shortcut");
        }
    }
}

fn refresh(icon: AppIcon) -> anyhow::Result<()> {
    let exe = std::env::current_exe()?;
    let icon_file = icon.ico(&exe);
    if !icon_file.is_file() {
        return Ok(());
    }
    let dirs =
        directories::BaseDirs::new().ok_or_else(|| anyhow::anyhow!("missing user profile"))?;
    let pins = dirs
        .data_dir()
        .join(r"Microsoft\Internet Explorer\Quick Launch\User Pinned\TaskBar");
    if !pins.is_dir() {
        return Ok(());
    }
    let exe = exe.canonicalize()?;
    for entry in std::fs::read_dir(pins)?.flatten() {
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("lnk"))
            && let Err(error) = update_link(&path, &exe, &icon_file)
        {
            tracing::debug!(%error, "could not read or refresh a taskbar shortcut");
        }
    }
    Ok(())
}

fn path_from_buffer(buf: &[u16]) -> PathBuf {
    OsString::from_wide(&buf[..buf.iter().position(|c| *c == 0).unwrap_or(buf.len())]).into()
}

fn update_link(path: &Path, exe: &Path, icon: &Path) -> anyhow::Result<()> {
    let path = HSTRING::from(path.as_os_str());
    // The winit window thread has an initialized COM apartment. The ShellLink is local to
    // this call. Save only after verifying that the existing link targets this executable.
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        let file: IPersistFile = link.cast()?;
        // The shell property store inherits this mode. STGM_READ lets SetIconLocation
        // succeed in memory but refuses the AppUserModelID write with STG_E_ACCESSDENIED.
        file.Load(&path, STGM_READWRITE)?;
        let mut buf = vec![0; 32768];
        link.GetPath(&mut buf, std::ptr::null_mut(), 0)?;
        let target = path_from_buffer(&buf);
        if !target
            .file_name()
            .zip(exe.file_name())
            .is_some_and(|(a, b)| a.eq_ignore_ascii_case(b))
            || !target.is_absolute()
            || target.canonicalize().ok().as_deref() != Some(exe)
        {
            return Ok(());
        }
        let mut index = 0;
        link.GetIconLocation(&mut buf, &mut index)?;
        let store: IPropertyStore = link.cast()?;
        let id = store.GetValue(&PKEY_AppUserModel_ID)?;
        if index == 0 && path_from_buffer(&buf) == icon && id.to_string() == APP_ID {
            return Ok(());
        }
        link.SetIconLocation(&HSTRING::from(icon.as_os_str()), 0)?;
        set_string(&store, &PKEY_AppUserModel_ID, OsStr::new(APP_ID))?;
        store.Commit()?;
        file.Save(&path, true)?;
        SHChangeNotify(
            SHCNE_UPDATEITEM,
            SHCNF_PATHW,
            Some(path.as_ptr().cast()),
            None,
        );
    }
    Ok(())
}
