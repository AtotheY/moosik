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
        UI::Shell::{
            CSIDL_COMMON_PROGRAMS, IShellLinkW, SHCNE_ASSOCCHANGED, SHCNE_UPDATEITEM, SHCNF_FLUSH,
            SHCNF_IDLIST, SHCNF_PATHW, SHChangeNotify, SHGetFolderPathW, ShellLink,
        },
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
    // The installed Start-menu shortcut is also an icon source for a running,
    // unpinned app. Its icon path expands per user, so no shared file needs
    // writes or elevation when someone chooses different artwork.
    let launcher_icon = dirs.data_local_dir().join(r"Moosik\Taskbar\current.ico");
    let artwork = std::fs::read(&icon_file)?;
    let launcher_changed =
        std::fs::read(&launcher_icon).ok().as_deref() != Some(artwork.as_slice());
    if launcher_changed {
        std::fs::create_dir_all(launcher_icon.parent().unwrap())?;
        crate::persist::write_atomic(&launcher_icon, &artwork)?;
    }
    let pins = dirs
        .data_dir()
        .join(r"Microsoft\Internet Explorer\Quick Launch\User Pinned\TaskBar");
    let exe = exe.canonicalize()?;
    let mut rewritten = Vec::new();
    let entries = match std::fs::read_dir(pins) {
        Ok(entries) => Some(entries),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    for entry in entries.into_iter().flatten().flatten() {
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("lnk"))
        {
            match update_link(&path, &exe, &icon_file) {
                Ok(true) => rewritten.push(path),
                Ok(false) => {}
                Err(error) => {
                    tracing::debug!(%error, "could not read or refresh a taskbar shortcut")
                }
            }
        }
    }
    if launcher_changed || !rewritten.is_empty() {
        // A real taskbar pin keeps its cached artwork even after the .lnk is saved.
        // Invalidate first, then tell Explorer exactly which updated items to reread.
        // Do this once per change, never on a launch/exit with an unchanged pin.
        unsafe {
            SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST | SHCNF_FLUSH, None, None);
        }
        let mut affected = vec![icon_file, launcher_icon];
        let mut programs = [0; 260];
        if unsafe { SHGetFolderPathW(None, CSIDL_COMMON_PROGRAMS as i32, None, 0, &mut programs) }
            .is_ok()
        {
            affected.push(path_from_buffer(&programs).join(r"Moosik\Moosik.lnk"));
        }
        for path in affected.into_iter().chain(rewritten) {
            let path = HSTRING::from(path.as_os_str());
            unsafe {
                SHChangeNotify(
                    SHCNE_UPDATEITEM,
                    SHCNF_PATHW | SHCNF_FLUSH,
                    Some(path.as_ptr().cast()),
                    None,
                );
            }
        }
    }
    Ok(())
}

fn path_from_buffer(buf: &[u16]) -> PathBuf {
    OsString::from_wide(&buf[..buf.iter().position(|c| *c == 0).unwrap_or(buf.len())]).into()
}

fn update_link(path: &Path, exe: &Path, icon: &Path) -> anyhow::Result<bool> {
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
            return Ok(false);
        }
        let mut index = 0;
        link.GetIconLocation(&mut buf, &mut index)?;
        let store: IPropertyStore = link.cast()?;
        let id = store.GetValue(&PKEY_AppUserModel_ID)?;
        if index == 0 && path_from_buffer(&buf) == icon && id.to_string() == APP_ID {
            return Ok(false);
        }
        link.SetIconLocation(&HSTRING::from(icon.as_os_str()), 0)?;
        set_string(&store, &PKEY_AppUserModel_ID, OsStr::new(APP_ID))?;
        store.Commit()?;
        file.Save(&path, true)?;
    }
    Ok(true)
}
