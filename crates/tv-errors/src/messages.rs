//! Reads message text from Windows: the system message table, and the message
//! tables inside a few DLLs. This is the crate's only `unsafe` code.
//!
//! DLLs are loaded as data files and image resources, so none of their code runs,
//! and only by full path, so Windows never searches for them and can't be tricked
//! into loading a planted copy. Each is loaded at most once and then kept for the
//! life of the process, which costs a little address space and saves reloading.

use regex::Regex;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, OnceLock};
use windows::core::{HSTRING, PWSTR};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::System::Diagnostics::Debug::{
    FormatMessageW, FORMAT_MESSAGE_FROM_HMODULE, FORMAT_MESSAGE_FROM_SYSTEM,
    FORMAT_MESSAGE_IGNORE_INSERTS,
};
use windows::Win32::System::LibraryLoader::{
    LoadLibraryExW, LOAD_LIBRARY_AS_DATAFILE, LOAD_LIBRARY_AS_IMAGE_RESOURCE,
};
use windows::Win32::System::SystemInformation::GetSystemDirectoryW;

/// A message table to look codes up in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Table {
    /// The system's own table, which holds Win32 codes and many HRESULTs.
    System,
    Ntdll,
    WinHttp,
    WindowsUpdate,
    Bits,
    ConfigMgr,
}

/// The DLL tables, in the order of their slots in the handle cache.
const DLL_TABLES: [Table; 5] = [
    Table::Ntdll,
    Table::WinHttp,
    Table::WindowsUpdate,
    Table::Bits,
    Table::ConfigMgr,
];

impl Table {
    /// Where the text comes from, as shown to people.
    pub(crate) fn source(self) -> &'static str {
        self.file_name().unwrap_or("system")
    }

    fn file_name(self) -> Option<&'static str> {
        match self {
            Self::System => None,
            Self::Ntdll => Some("ntdll.dll"),
            Self::WinHttp => Some("winhttp.dll"),
            Self::WindowsUpdate => Some("wuaueng.dll"),
            Self::Bits => Some("qmgr.dll"),
            Self::ConfigMgr => Some("srsresources.dll"),
        }
    }

    /// The full paths to try, in order.
    fn candidate_paths(self) -> Vec<PathBuf> {
        let Some(file_name) = self.file_name() else {
            return Vec::new();
        };
        if self == Self::ConfigMgr {
            return configmgr_console_folders()
                .into_iter()
                .map(|folder| folder.join(file_name))
                .collect();
        }
        system_directory()
            .map(|folder| vec![folder.join(file_name)])
            .unwrap_or_default()
    }
}

/// The longest message read, in UTF-16 units. FormatMessageW can't return more
/// than 64 KB, and real messages are far shorter.
const BUFFER_LENGTH: u32 = 16 * 1024;

/// Looks a code up in one table, in the user's language. None if the table
/// isn't available or has no readable message for the code.
pub(crate) fn message(table: Table, code: u32) -> Option<String> {
    let (flags, module) = match table {
        Table::System => (FORMAT_MESSAGE_FROM_SYSTEM, None),
        _ => (FORMAT_MESSAGE_FROM_HMODULE, Some(module(table)?)),
    };
    let mut buffer = vec![0u16; BUFFER_LENGTH as usize];
    // Language 0 picks the user's language, falling back as Windows does.
    // SAFETY: IGNORE_INSERTS means no arguments are read. The buffer's length
    // is passed with it, and a cached module handle is never freed.
    let length = unsafe {
        FormatMessageW(
            flags | FORMAT_MESSAGE_IGNORE_INSERTS,
            module.map(|handle| handle.0.cast_const()),
            code,
            0,
            PWSTR(buffer.as_mut_ptr()),
            BUFFER_LENGTH,
            None,
        )
    };
    let text = buffer.get(..length as usize)?;
    clean_message(&String::from_utf16_lossy(text))
}

/// Returns a DLL's handle, loading it on first use. The handle is kept as an
/// address, because raw handles can't be shared between threads directly.
fn module(table: Table) -> Option<HMODULE> {
    static LOADED: [OnceLock<Option<usize>>; DLL_TABLES.len()] =
        [const { OnceLock::new() }; DLL_TABLES.len()];
    let slot = DLL_TABLES.iter().position(|&dll| dll == table)?;
    let address = LOADED[slot].get_or_init(|| {
        table
            .candidate_paths()
            .iter()
            .find_map(|path| load_as_data(path))
    });
    address.map(|address| HMODULE(std::ptr::with_exposed_provenance_mut(address)))
}

/// Says whether a table's DLL could be loaded, for tests and diagnostics.
#[cfg(test)]
pub(crate) fn is_available(table: Table) -> bool {
    table == Table::System || module(table).is_some()
}

fn load_as_data(path: &Path) -> Option<usize> {
    // A relative path would make Windows search for the file.
    if !path.is_absolute() {
        return None;
    }
    // SAFETY: the path string outlives the call. Loaded this way, the DLL's code
    // never runs, and only its resources are read.
    let module = unsafe {
        LoadLibraryExW(
            &HSTRING::from(path),
            None,
            LOAD_LIBRARY_AS_DATAFILE | LOAD_LIBRARY_AS_IMAGE_RESOURCE,
        )
    }
    .ok()?;
    Some(module.0.expose_provenance())
}

/// The real System32 folder, asked of Windows rather than read from an
/// environment variable that anything could change.
fn system_directory() -> Option<PathBuf> {
    let mut buffer = [0u16; 512];
    // SAFETY: the buffer is a local, and its length is passed with it.
    let length = unsafe { GetSystemDirectoryW(Some(&mut buffer)) } as usize;
    // Zero means failure, and a larger value means the buffer was too small.
    if length == 0 || length >= buffer.len() {
        return None;
    }
    Some(PathBuf::from(OsString::from_wide(&buffer[..length])))
}

/// Folders where an installed ConfigMgr console keeps `srsresources.dll`. The
/// console's setup records its `bin\i386` folder in `SMS_ADMIN_UI_PATH`, and
/// the DLL sits one level up. The product folder was renamed in later versions,
/// so both names are tried.
fn configmgr_console_folders() -> Vec<PathBuf> {
    let mut folders = Vec::new();
    if let Some(console) = std::env::var_os("SMS_ADMIN_UI_PATH").map(PathBuf::from) {
        if let Some(parent) = console.parent() {
            folders.push(parent.to_path_buf());
        }
        folders.push(console);
    }
    for program_files in ["ProgramFiles(x86)", "ProgramFiles"] {
        let Some(root) = std::env::var_os(program_files).map(PathBuf::from) else {
            continue;
        };
        for product in [
            "Microsoft Endpoint Manager",
            "Microsoft Configuration Manager",
        ] {
            folders.push(root.join(product).join("AdminConsole").join("bin"));
        }
    }
    folders
}

/// Finds insert placeholders and escapes that FormatMessageW leaves in text when
/// told to ignore inserts. Group 1 is an escaped character to keep. Anything else
/// is a placeholder: `%1` to `%99` with an optional `!format!`, or a C-style one
/// such as `%s` or `%p`, which some NTSTATUS messages use.
static PLACEHOLDER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"%(?:([%.!])|[1-9][0-9]?(?:![^!\s]{0,16}!)?|[-#+0]*[0-9]*(?:\.[0-9]+)?(?:hh|h|ll|l|I64|I32|w|z)?[cCdiouxXsSpZ])",
    )
    .expect("the placeholder pattern is valid")
});

/// Turns raw message-table text into one tidy line. Returns None if nothing
/// readable is left, as with a message that's only a placeholder.
pub(crate) fn clean_message(raw: &str) -> Option<String> {
    let replaced = PLACEHOLDER.replace_all(raw, |captures: &regex::Captures| {
        captures
            .get(1)
            .map_or_else(|| "…".to_string(), |escaped| escaped.as_str().to_string())
    });
    let message = replaced.split_whitespace().collect::<Vec<_>>().join(" ");
    message
        .chars()
        .any(char::is_alphanumeric)
        .then_some(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitespace_becomes_single_spaces() {
        assert_eq!(
            clean_message("  {Title}\r\nFirst  line.\r\n\tSecond.\r\n").as_deref(),
            Some("{Title} First line. Second.")
        );
    }

    #[test]
    fn placeholders_become_ellipses() {
        assert_eq!(
            clean_message("At 0x%p, file %1 and %2!s! failed: %s").as_deref(),
            Some("At 0x…, file … and … failed: …")
        );
    }

    #[test]
    fn escapes_keep_their_character() {
        assert_eq!(
            clean_message("100%% done%. Really%!").as_deref(),
            Some("100% done. Really!")
        );
    }

    #[test]
    fn a_lone_percent_sign_stays() {
        assert_eq!(clean_message("50% left").as_deref(), Some("50% left"));
    }

    #[test]
    fn nothing_readable_is_no_message() {
        assert_eq!(clean_message("%1"), None);
        assert_eq!(clean_message(" \r\n"), None);
        assert_eq!(clean_message(""), None);
    }

    #[test]
    fn the_system_directory_is_found() {
        let folder = system_directory().expect("Windows reports its System32 folder");
        assert!(folder.is_absolute());
        assert!(folder.join("ntdll.dll").is_file());
    }

    #[test]
    fn configmgr_paths_are_only_ever_full_paths_when_loaded() {
        // A relative path is refused before Windows can search for it.
        assert_eq!(load_as_data(Path::new("ntdll.dll")), None);
    }

    #[test]
    fn system_dlls_with_message_tables_load() {
        for table in [Table::Ntdll, Table::WinHttp, Table::Bits] {
            assert!(is_available(table), "{table:?} should load");
        }
    }

    #[test]
    fn loading_is_cached() {
        let first = module(Table::Ntdll).map(|handle| handle.0);
        let second = module(Table::Ntdll).map(|handle| handle.0);
        assert!(first.is_some());
        assert_eq!(first, second);
    }

    #[test]
    fn unknown_codes_have_no_message() {
        // Not a code any of these tables use.
        for table in [Table::System, Table::Ntdll, Table::WinHttp, Table::Bits] {
            assert_eq!(message(table, 0x2BAD_F00D), None, "{table:?}");
        }
    }
}
