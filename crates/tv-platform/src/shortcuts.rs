//! Start menu shortcuts, so typing "TraceView" in Start opens the copy
//! that's installed, rather than whichever `traceview.exe` search finds.

use std::io;
use std::path::{Path, PathBuf};
use windows::core::{Interface, HSTRING};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED, STGM_READ,
};
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};

/// COM on this thread while it lives, for the command line, which hasn't
/// started it. On the window's thread, it's already started.
struct Com(bool);

impl Com {
    fn start() -> Self {
        // SAFETY: no reserved argument is passed, and a successful start is
        // matched by one uninitialize when this is dropped.
        Self(unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok())
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: matches the successful start above, on the same thread.
            unsafe { CoUninitialize() };
        }
    }
}

/// Makes a shortcut at `shortcut` that opens `target`, with its icon and a
/// description, making its folder if it isn't there yet.
pub fn create_shortcut(shortcut: &Path, target: &Path, description: &str) -> io::Result<()> {
    if let Some(folder) = shortcut.parent() {
        std::fs::create_dir_all(folder)?;
    }
    let _com = Com::start();
    // SAFETY: COM is started on this thread, every string outlives the call
    // that reads it, and the interfaces are released when dropped.
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(&HSTRING::from(target.as_os_str()))?;
        if let Some(folder) = target.parent() {
            link.SetWorkingDirectory(&HSTRING::from(folder.as_os_str()))?;
        }
        link.SetDescription(&HSTRING::from(description))?;
        link.SetIconLocation(&HSTRING::from(target.as_os_str()), 0)?;
        link.cast::<IPersistFile>()?
            .Save(&HSTRING::from(shortcut.as_os_str()), true)?;
    }
    Ok(())
}

/// What a shortcut opens, if there's a shortcut there.
pub fn shortcut_target(shortcut: &Path) -> Option<PathBuf> {
    if !shortcut.is_file() {
        return None;
    }
    let _com = Com::start();
    // SAFETY: COM is started on this thread, the path outlives the call, and
    // the buffer is passed with its length.
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        link.cast::<IPersistFile>()
            .ok()?
            .Load(&HSTRING::from(shortcut.as_os_str()), STGM_READ)
            .ok()?;
        let mut buffer = [0u16; 1024];
        link.GetPath(&mut buffer, std::ptr::null_mut(), 0).ok()?;
        let end = buffer
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(buffer.len());
        (end > 0).then(|| PathBuf::from(String::from_utf16_lossy(&buffer[..end])))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shortcut_opens_what_it_was_made_for() {
        let dir = tempfile::tempdir().unwrap();
        let target = std::env::current_exe().unwrap();
        let shortcut = dir.path().join("Start Menu").join("TraceView.lnk");
        assert_eq!(shortcut_target(&shortcut), None);
        create_shortcut(&shortcut, &target, "A log viewer").unwrap();
        let found = shortcut_target(&shortcut).unwrap();
        assert!(
            found
                .to_string_lossy()
                .eq_ignore_ascii_case(&target.to_string_lossy()),
            "{} is not {}",
            found.display(),
            target.display()
        );
    }
}
