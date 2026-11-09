//! Reads message text from Windows: the system message table, and the message
//! tables inside a few DLLs. This is the crate's only `unsafe` code. It also
//! reads CMTrace's message table, which needs no `unsafe`.
//!
//! DLLs are loaded as data files and image resources, so none of their code runs,
//! and only by full path, so Windows never searches for them and can't be tricked
//! into loading a planted copy. Each is loaded at most once and then kept for the
//! life of the process, which costs a little address space and saves reloading.
//! CMTrace isn't loaded at all: its file is read as bytes, also by full path.

use crate::resources;
use regex::Regex;
use std::ffi::OsString;
use std::fs::File;
use std::io::Read;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, OnceLock};
use windows::core::{HSTRING, PWSTR};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::System::Diagnostics::Debug::{
    FormatMessageW, FORMAT_MESSAGE_FROM_HMODULE, FORMAT_MESSAGE_FROM_SYSTEM,
    FORMAT_MESSAGE_IGNORE_INSERTS, FORMAT_MESSAGE_OPTIONS,
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
    Wmi,
    /// The table inside `CMTrace.exe`, which explains ConfigMgr and Windows
    /// Update codes. It's in English only.
    CmTrace,
}

/// The DLL tables, in the order of their slots in the handle cache.
const DLL_TABLES: [Table; 6] = [
    Table::Ntdll,
    Table::WinHttp,
    Table::WindowsUpdate,
    Table::Bits,
    Table::ConfigMgr,
    Table::Wmi,
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
            Self::Wmi => Some("wmiutils.dll"),
            Self::CmTrace => Some("CMTrace.exe"),
        }
    }

    /// The full paths to try, in order.
    fn candidate_paths(self) -> Vec<PathBuf> {
        let Some(file_name) = self.file_name() else {
            return Vec::new();
        };
        match self {
            Self::ConfigMgr => configmgr_console_folders()
                .into_iter()
                .map(|folder| folder.join(file_name))
                .collect(),
            Self::CmTrace => cmtrace_folders()
                .into_iter()
                .map(|folder| folder.join(file_name))
                .collect(),
            // WMI keeps its files in System32's wbem folder.
            Self::Wmi => system_directory()
                .map(|folder| vec![folder.join("wbem").join(file_name)])
                .unwrap_or_default(),
            _ => system_directory()
                .map(|folder| vec![folder.join(file_name)])
                .unwrap_or_default(),
        }
    }
}

/// The longest message read, in UTF-16 units. FormatMessageW can't return more
/// than 64 KB, and real messages are far shorter.
const BUFFER_LENGTH: u32 = 16 * 1024;

/// Looks a code up in one table, in the user's language, or in English for
/// CMTrace's. None if the table isn't available or has no readable message for
/// the code.
pub(crate) fn message(table: Table, code: u32) -> Option<String> {
    let (flags, module) = match table {
        Table::System => (FORMAT_MESSAGE_FROM_SYSTEM, None),
        Table::CmTrace => {
            return clean_message(&resources::find_message(cmtrace_table()?, code)?);
        }
        _ => (FORMAT_MESSAGE_FROM_HMODULE, Some(module(table)?)),
    };
    // Language 0 picks the user's language, falling back as Windows does.
    format_message(flags, module, code, 0)
}

/// Asks Windows for a message's text in a language, cleaned up.
fn format_message(
    flags: FORMAT_MESSAGE_OPTIONS,
    module: Option<HMODULE>,
    code: u32,
    language: u32,
) -> Option<String> {
    let mut buffer = vec![0u16; BUFFER_LENGTH as usize];
    // SAFETY: IGNORE_INSERTS means no arguments are read. The buffer's length
    // is passed with it, and a cached module handle is never freed.
    let length = unsafe {
        FormatMessageW(
            flags | FORMAT_MESSAGE_IGNORE_INSERTS,
            module.map(|handle| handle.0.cast_const()),
            code,
            language,
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
    match table {
        Table::System => true,
        Table::CmTrace => cmtrace_table().is_some(),
        _ => module(table).is_some(),
    }
}

/// The largest `CMTrace.exe` read. Real ones are under 1 MB.
const LARGEST_CMTRACE: u64 = 16 * 1024 * 1024;

/// CMTrace's message table, read on first use from the first CMTrace found,
/// then kept for the life of the process. It's about 100 KB.
fn cmtrace_table() -> Option<&'static [u8]> {
    static TABLE: OnceLock<Option<Vec<u8>>> = OnceLock::new();
    TABLE
        .get_or_init(|| {
            Table::CmTrace.candidate_paths().iter().find_map(|path| {
                let file = read_file(path, LARGEST_CMTRACE)?;
                resources::cmtrace_messages(&file).map(<[u8]>::to_vec)
            })
        })
        .as_deref()
}

/// Reads a whole file by full path, if it's no larger than `limit`.
fn read_file(path: &Path, limit: u64) -> Option<Vec<u8>> {
    if !path.is_absolute() {
        return None;
    }
    let mut bytes = Vec::new();
    File::open(path)
        .ok()?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() as u64 <= limit).then_some(bytes)
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

/// Folders where ConfigMgr keeps `CMTrace.exe`: the client's folder in the
/// Windows folder, on every managed PC, and a site server's `tools` folder.
fn cmtrace_folders() -> Vec<PathBuf> {
    let mut folders = Vec::new();
    // System32's parent is the Windows folder.
    if let Some(windows) = system_directory().as_deref().and_then(Path::parent) {
        folders.push(windows.join("CCM"));
    }
    if let Some(root) = std::env::var_os("ProgramFiles").map(PathBuf::from) {
        folders.push(root.join("Microsoft Configuration Manager").join("tools"));
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
        for table in [Table::Ntdll, Table::WinHttp, Table::Bits, Table::Wmi] {
            assert!(is_available(table), "{table:?} should load");
        }
    }

    #[test]
    fn wmi_codes_have_text_in_wmiutils() {
        // WBEM_E_INVALID_CLASS and WBEM_E_PROVIDER_LOAD_FAILURE.
        for code in [0x8004_1010, 0x8004_1013] {
            assert!(message(Table::Wmi, code).is_some(), "{code:#x}");
        }
    }

    #[test]
    fn the_resource_reader_agrees_with_windows() {
        // ntdll.dll carries its English message table itself, so reading the
        // file and asking Windows must give the same text, in English.
        const ENGLISH: u32 = 0x409;
        let path = system_directory().unwrap().join("ntdll.dll");
        let file = read_file(&path, 64 * 1024 * 1024).expect("ntdll.dll can be read");
        let table = resources::cmtrace_messages(&file).expect("ntdll.dll has a message table");
        let ntdll = module(Table::Ntdll).expect("ntdll.dll loads");
        let mut compared = 0;
        let codes = (0xC000_0000..0xC000_0400)
            .chain(0x8000_0001..0x8000_0040)
            .chain(0x4000_0000..0x4000_0040);
        for code in codes {
            // FormatMessageW stops at "%0", even within "%08lx", so do the same.
            let ours = resources::find_message(table, code)
                .and_then(|raw| clean_message(raw.split("%0").next().unwrap_or_default()));
            let windows = format_message(FORMAT_MESSAGE_FROM_HMODULE, Some(ntdll), code, ENGLISH);
            assert_eq!(ours, windows, "{code:#x}");
            compared += usize::from(ours.is_some());
        }
        assert!(compared > 500, "only {compared} messages were compared");
    }

    #[test]
    fn cmtrace_is_looked_for_where_configmgr_keeps_it() {
        let windows = system_directory().unwrap().parent().unwrap().to_path_buf();
        let paths = Table::CmTrace.candidate_paths();
        assert_eq!(paths[0], windows.join("CCM").join("CMTrace.exe"));
        assert!(paths
            .iter()
            .any(|path| path.ends_with(r"Microsoft Configuration Manager\tools\CMTrace.exe")));
        // Never a relative path, which would be read from the current folder.
        assert_eq!(read_file(Path::new("CMTrace.exe"), LARGEST_CMTRACE), None);
    }

    #[test]
    fn a_real_cmtrace_has_configmgr_text() {
        // ConfigMgr isn't on most PCs that run the tests, so this runs only
        // when TRACEVIEW_TEST_CMTRACE is the full path to a CMTrace.exe.
        let Some(path) = std::env::var_os("TRACEVIEW_TEST_CMTRACE") else {
            return;
        };
        let file = read_file(Path::new(&path), LARGEST_CMTRACE)
            .expect("TRACEVIEW_TEST_CMTRACE is the full path to a readable file");
        let table = resources::cmtrace_messages(&file).expect("CMTrace has a message table");
        let text = |code| resources::find_message(table, code).and_then(|raw| clean_message(&raw));
        assert_eq!(text(0x87D0_0215).as_deref(), Some("Item not found"));
        assert_eq!(
            text(0x87D0_0324).as_deref(),
            Some("The application was not detected after installation completed.")
        );
        assert_eq!(
            text(0x8024_0022).as_deref(),
            Some("Operation failed for all the updates.")
        );
        assert_eq!(text(0x8007_0002), None);
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
