//! Services the Windows shell provides: file dialogs, dropped files, the
//! clipboard, the title bar's color, and the system's light or dark setting
//! and contrast theme.

use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use tv_ui::theme::ContrastColors;
use tv_ui::Color;
use windows::core::{w, HSTRING, PWSTR};
use windows::Win32::Foundation::{GlobalFree, ERROR_SUCCESS, HANDLE, HGLOBAL, HWND};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_CAPTION_COLOR, DWMWA_COLOR_DEFAULT, DWMWA_TEXT_COLOR,
    DWMWA_USE_IMMERSIVE_DARK_MODE,
};
use windows::Win32::Graphics::Gdi::{
    GetSysColor, COLOR_GRAYTEXT, COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT, COLOR_HOTLIGHT,
    COLOR_WINDOW, COLOR_WINDOWTEXT, SYS_COLOR_INDEX,
};
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_INPROC_SERVER};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    DragFinish, DragQueryFileW, FileOpenDialog, FileSaveDialog, IFileOpenDialog, IFileSaveDialog,
    ShellExecuteW, FOS_ALLOWMULTISELECT, FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM,
    FOS_OVERWRITEPROMPT, HDROP, SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::{
    MessageBoxW, SystemParametersInfoW, MB_ICONERROR, MB_ICONINFORMATION, MB_OK,
    SPI_GETHIGHCONTRAST, SW_SHOWNORMAL, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
};

/// Shows a folder in File Explorer, making it first if it isn't there yet.
/// Returns false if it couldn't be shown.
pub fn show_folder(folder: &Path) -> bool {
    if std::fs::create_dir_all(folder).is_err() || !folder.is_dir() {
        return false;
    }
    // SAFETY: the strings outlive the call, and no parent window is needed.
    // Opening a folder only ever starts File Explorer on it.
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &HSTRING::from(folder.as_os_str()),
            None,
            None,
            SW_SHOWNORMAL,
        )
    };
    result.0 as isize > 32
}

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

/// Shows information in a standard message box, which keeps the window
/// waiting until it's closed.
pub(crate) fn show_message(hwnd: HWND, title: &str, text: &str) {
    // SAFETY: the handle is the caller's window, and the strings outlive the call.
    unsafe {
        MessageBoxW(
            Some(hwnd),
            &HSTRING::from(text),
            &HSTRING::from(title),
            MB_OK | MB_ICONINFORMATION,
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

/// The colors of the Windows contrast theme in use, or `None` when none is.
pub(crate) fn contrast_colors() -> Option<ContrastColors> {
    let mut contrast = HIGHCONTRASTW {
        cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32,
        ..HIGHCONTRASTW::default()
    };
    // SAFETY: the out parameter is a local of the size it states, as this
    // setting needs.
    unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            contrast.cbSize,
            Some(std::ptr::from_mut(&mut contrast).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    }
    .ok()?;
    if contrast.dwFlags.0 & HCF_HIGHCONTRASTON.0 == 0 {
        return None;
    }
    let color = |index: SYS_COLOR_INDEX| {
        // SAFETY: reading a system color has no requirements.
        from_color_ref(unsafe { GetSysColor(index) })
    };
    Some(ContrastColors {
        background: color(COLOR_WINDOW),
        text: color(COLOR_WINDOWTEXT),
        link: color(COLOR_HOTLIGHT),
        disabled: color(COLOR_GRAYTEXT),
        selected: color(COLOR_HIGHLIGHT),
        selected_text: color(COLOR_HIGHLIGHTTEXT),
    })
}

fn from_color_ref(value: u32) -> Color {
    Color {
        r: (value & 0xFF) as u8,
        g: ((value >> 8) & 0xFF) as u8,
        b: ((value >> 16) & 0xFF) as u8,
    }
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

/// Gives the title bar back to Windows, as a contrast theme needs, since
/// Windows draws it in that theme's colors itself.
pub(crate) fn default_caption_colors(hwnd: HWND) {
    let default = DWMWA_COLOR_DEFAULT;
    // SAFETY: each pointer is to a local of the size passed with it.
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_CAPTION_COLOR,
            std::ptr::from_ref(&default).cast(),
            4,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_TEXT_COLOR,
            std::ptr::from_ref(&default).cast(),
            4,
        );
    }
}

/// Shows the open dialog, and returns the chosen files, of which there can be
/// several. `.lo_` is how SCCM names rotated logs. Must run on the window's
/// thread, with no state borrowed.
pub(crate) fn choose_files(hwnd: HWND) -> Vec<PathBuf> {
    choose(hwnd).unwrap_or_default()
}

fn choose(hwnd: HWND) -> Option<Vec<PathBuf>> {
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
            .SetOptions(options | FOS_FILEMUSTEXIST | FOS_FORCEFILESYSTEM | FOS_ALLOWMULTISELECT)
            .ok()?;
        dialog.SetTitle(&HSTRING::from("Open logs")).ok()?;
        dialog.Show(Some(hwnd)).ok()?;
        let items = dialog.GetResults().ok()?;
        let mut chosen = Vec::new();
        for index in 0..items.GetCount().ok()? {
            let path: PWSTR = items
                .GetItemAt(index)
                .ok()?
                .GetDisplayName(SIGDN_FILESYSPATH)
                .ok()?;
            chosen.push(wide_to_path(path.as_wide()));
            CoTaskMemFree(Some(path.0.cast()));
        }
        Some(chosen)
    }
}

/// Shows the save dialog for a CSV file, starting with `suggested_name`, and
/// returns the chosen path. Must run on the window's thread, with no state borrowed.
pub(crate) fn choose_save_path(hwnd: HWND, suggested_name: &str) -> Option<PathBuf> {
    // SAFETY: COM was initialized on this thread by `run`. All pointers are live
    // for their call, and the returned path is freed after being copied.
    unsafe {
        let dialog: IFileSaveDialog =
            CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let filters = [COMDLG_FILTERSPEC {
            pszName: w!("CSV files (*.csv)"),
            pszSpec: w!("*.csv"),
        }];
        dialog.SetFileTypes(&filters).ok()?;
        dialog.SetDefaultExtension(w!("csv")).ok()?;
        let options = dialog.GetOptions().ok()?;
        dialog
            .SetOptions(options | FOS_FORCEFILESYSTEM | FOS_OVERWRITEPROMPT)
            .ok()?;
        dialog.SetFileName(&HSTRING::from(suggested_name)).ok()?;
        dialog.SetTitle(&HSTRING::from("Export rows")).ok()?;
        dialog.Show(Some(hwnd)).ok()?;
        let path: PWSTR = dialog
            .GetResult()
            .ok()?
            .GetDisplayName(SIGDN_FILESYSPATH)
            .ok()?;
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

/// Reads text from the clipboard, if it holds any.
pub fn clipboard_text(hwnd: isize) -> Option<String> {
    // SAFETY: the clipboard is opened and closed here. The data is only read
    // while locked, and never past the size Windows reports for it.
    unsafe {
        OpenClipboard(Some(HWND(hwnd as *mut _))).ok()?;
        let text = (|| {
            let handle = GetClipboardData(u32::from(CF_UNICODETEXT.0)).ok()?;
            let memory = HGLOBAL(handle.0);
            let locked = GlobalLock(memory).cast::<u16>();
            if locked.is_null() {
                return None;
            }
            let capacity = GlobalSize(memory) / 2;
            let units = std::slice::from_raw_parts(locked, capacity);
            let length = units.iter().position(|unit| *unit == 0).unwrap_or(capacity);
            let text = String::from_utf16_lossy(&units[..length]);
            let _ = GlobalUnlock(memory);
            Some(text)
        })();
        let _ = CloseClipboard();
        text
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
        assert_eq!(from_color_ref(0x0033_2211), Color::rgb(0x11_22_33));
    }

    #[test]
    fn reading_the_contrast_theme_does_not_fail() {
        let _ = contrast_colors();
    }

    #[test]
    fn reading_the_system_theme_does_not_fail() {
        let _ = system_is_dark();
    }
}
