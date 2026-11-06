//! Explorer uses the shortcut's icon when a pinned app is closed.
use super::AppIcon;
use std::{
    ffi::OsString,
    os::windows::ffi::OsStringExt,
    path::{Path, PathBuf},
};
use windows::{
    Win32::{
        System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile, STGM_READ},
        UI::Shell::{IShellLinkW, SHCNE_UPDATEITEM, SHCNF_PATHW, SHChangeNotify, ShellLink},
    },
    core::{HSTRING, Interface},
};

impl AppIcon {
    pub(crate) fn refresh_pin(self) {
        if let Err(error) = refresh(self) {
            tracing::debug!(%error, "could not refresh pinned Moosik shortcut");
        }
    }
}

fn refresh(icon: AppIcon) -> anyhow::Result<()> {
    let exe = std::env::current_exe()?;
    let icon_file = exe
        .with_file_name("icons")
        .join(format!("{}.ico", icon.label().to_ascii_lowercase()));
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
        file.Load(&path, STGM_READ)?;
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
        if index == 0 && path_from_buffer(&buf) == icon {
            return Ok(());
        }
        link.SetIconLocation(&HSTRING::from(icon.as_os_str()), 0)?;
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
