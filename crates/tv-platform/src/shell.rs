//! Services the Windows shell provides: file dialogs, dropped files, the
//! clipboard, the title bar's color, and the system's light or dark setting.

use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use tv_ui::Color;
use windows::core::{w, HSTRING, PWSTR};
use windows::Win32::Foundation::{GlobalFree, ERROR_SUCCESS, HANDLE, HGLOBAL, HWND};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE,
};
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_INPROC_SERVER};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    DragFinish, DragQueryFileW, FileOpenDialog, IFileOpenDialog, FOS_FILEMUSTEXIST,
    FOS_FORCEFILESYSTEM, HDROP, SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};

/// Reports a problem when there may be no console to report it on.
pub fn show_error(message: &str) {
    // SAFETY: both strings outlive the call, and no parent window is needed.
    unsafe {
        MessageBoxW(
            None,
            &HSTRING::from(message),
            w!("TraceView"),
            MB_OK | MB_ICONERROR,
        );
    }
}

pub(crate) fn system_is_dark() -> bool {
    let mut value = 1u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: the value and size are locals of the type this flag writes.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(std::ptr::from_mut(&mut value).cast()),
            Some(&mut size),
        )
    };
    status == ERROR_SUCCESS && value == 0
}

fn color_ref(color: Color) -> u32 {
    u32::from(color.r) | u32::from(color.g) << 8 | u32::from(color.b) << 16
}

/// Colors the native title bar. Caption colors need Windows 11, and Windows 10
/// ignores them and keeps its own, which is harmless.
pub(crate) fn set_caption_colors(hwnd: HWND, background: Color, text: Color, dark: bool) {
    let dark_mode = i32::from(dark);
    let (background, text) = (color_ref(background), color_ref(text));
    // SAFETY: each pointer is to a local of the size passed with it.
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            std::ptr::from_ref(&dark_mode).cast(),
            4,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_CAPTION_COLOR,
            std::ptr::from_ref(&background).cast(),
            4,
        );
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_TEXT_COLOR, std::ptr::from_ref(&text).cast(), 4);
    }
}

/// Shows the open dialog, and returns the chosen file. `.lo_` is how SCCM names
/// rotated logs. Must run on the window's thread, with no state borrowed.
pub(crate) fn choose_file(hwnd: HWND) -> Option<PathBuf> {
    // SAFETY: COM was initialized on this thread by `run`. All pointers are live
    // for their call, and the returned path is freed after being copied.
    unsafe {
        let dialog: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let filters = [
            COMDLG_FILTERSPEC {
                pszName: w!("Log files (*.log, *.lo_, *.txt)"),
                pszSpec: w!("*.log;*.lo_;*.txt"),
            },
            COMDLG_FILTERSPEC {
                pszName: w!("All files (*.*)"),
                pszSpec: w!("*.*"),
            },
        ];
        dialog.SetFileTypes(&filters).ok()?;
        let options = dialog.GetOptions().ok()?;
        dialog
            .SetOptions(options | FOS_FILEMUSTEXIST | FOS_FORCEFILESYSTEM)
            .ok()?;
        dialog.SetTitle(&HSTRING::from("Open log file")).ok()?;
        dialog.Show(Some(hwnd)).ok()?;
        let item = dialog.GetResult().ok()?;
        let path: PWSTR = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let chosen = wide_to_path(path.as_wide());
        CoTaskMemFree(Some(path.0.cast()));
        Some(chosen)
    }
}

fn wide_to_path(wide: &[u16]) -> PathBuf {
    PathBuf::from(std::ffi::OsString::from_wide(wide))
}

/// Reads the paths from a file-drop message and releases its handle.
pub(crate) fn dropped_files(drop: HDROP) -> Vec<PathBuf> {
    // SAFETY: the handle came from WM_DROPFILES, and buffers are sized by the
    // preceding query. DragFinish is called exactly once.
    unsafe {
        let count = DragQueryFileW(drop, u32::MAX, None);
        let mut paths = Vec::with_capacity(count as usize);
        for index in 0..count {
            let length = DragQueryFileW(drop, index, None) as usize;
            let mut buffer = vec![0u16; length + 1];
            let copied = DragQueryFileW(drop, index, Some(&mut buffer)) as usize;
            paths.push(wide_to_path(&buffer[..copied]));
        }
        DragFinish(drop);
        paths
    }
}

/// Puts text on the clipboard. Fails quietly if another app has it locked,
/// because there's nothing useful to do about that.
pub(crate) fn set_clipboard_text(hwnd: HWND, text: &str) {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = wide.len() * 2;
    // SAFETY: the allocation holds `bytes` and is copied into while locked. After
    // a successful SetClipboardData, the system owns it. Otherwise it's freed.
    unsafe {
        if OpenClipboard(Some(hwnd)).is_err() {
            return;
        }
        let _ = EmptyClipboard();
        if let Ok(memory) = GlobalAlloc(GMEM_MOVEABLE, bytes) {
            let locked = GlobalLock(memory);
            if locked.is_null() {
                let _ = GlobalFree(Some(memory));
            } else {
                std::ptr::copy_nonoverlapping(wide.as_ptr(), locked.cast::<u16>(), wide.len());
                let _ = GlobalUnlock(memory);
                let handle = HANDLE(memory.0);
                if SetClipboardData(u32::from(CF_UNICODETEXT.0), Some(handle)).is_err() {
                    let _ = GlobalFree(Some(HGLOBAL(memory.0)));
                }
            }
        }
        let _ = CloseClipboard();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_use_windows_byte_order() {
        assert_eq!(color_ref(Color::rgb(0x11_22_33)), 0x0033_2211);
    }

    #[test]
    fn reading_the_system_theme_does_not_fail() {
        let _ = system_is_dark();
    }
}
