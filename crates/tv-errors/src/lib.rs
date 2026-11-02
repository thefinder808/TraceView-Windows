//! Turns error codes found in Windows logs into readable explanations, for the
//! Error Lookup feature.
//!
//! Every message comes from Windows itself at run time, through `FormatMessageW`.
//! Nothing here stores message text, code names or code tables, so messages are
//! in the user's own language and match the Windows they're running on. The only
//! table in this crate maps HRESULT facility numbers to short names.
//!
//! Where the text comes from, and what was proven on Windows 11 build 26300:
//!
//! | Kind | Message table | Status |
//! |---|---|---|
//! | Win32 | the system table | Works |
//! | HRESULT | the system table, or the Win32 code it wraps | Works |
//! | NTSTATUS | `ntdll.dll` | Works |
//! | WinHTTP, 12001-12199 | `winhttp.dll` | Works |
//! | BITS, 0x8020xxxx, and the HTTP statuses it reports, 0x8019xxxx | `qmgr.dll` | Works |
//! | Windows Update, 0x8024xxxx | `wuaueng.dll` | Tried, but its table only holds event messages, so these don't decode |
//! | ConfigMgr, 0x87D0xxxx and 0x87D1xxxx | `srsresources.dll` from an installed ConfigMgr console | Untested, as no console was installed. Never bundled |
//!
//! DLLs are loaded as data, so none of their code runs, and from full paths, so
//! Windows never searches for them.

#![deny(unsafe_code)]

mod decode;
mod find;
#[allow(unsafe_code)]
mod messages;
mod parse;

pub use decode::{lookup, lookup_all};
pub use find::find_codes;

/// What kind of code a reading treats the value as.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CodeKind {
    /// A Win32 error code, as from `GetLastError`.
    Win32,
    /// A COM-style result code.
    HResult,
    /// A kernel and native API status code.
    NtStatus,
    /// A WinHTTP error code, which is a Win32 code with its own message table.
    WinHttp,
    /// A Windows Update Agent result code.
    WindowsUpdate,
    /// A Background Intelligent Transfer Service result code.
    Bits,
    /// A Configuration Manager result code.
    ConfigMgr,
}

impl CodeKind {
    /// A short label for showing to people.
    pub fn name(self) -> &'static str {
        match self {
            Self::Win32 => "Win32",
            Self::HResult => "HRESULT",
            Self::NtStatus => "NTSTATUS",
            Self::WinHttp => "WinHTTP",
            Self::WindowsUpdate => "Windows Update",
            Self::Bits => "BITS",
            Self::ConfigMgr => "ConfigMgr",
        }
    }
}

/// One explanation of a code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decoded {
    pub kind: CodeKind,
    /// The value as an unsigned 32-bit code. When an HRESULT wraps a Win32
    /// code, its Win32 or WinHTTP reading holds the inner 16-bit code.
    pub code: u32,
    /// The code in hex, such as `0x80070005`.
    pub hex: String,
    /// The code as a signed number, such as `-2147024891`, which is how logs
    /// often print it.
    pub signed: i32,
    /// For HRESULT-shaped codes, the facility name and number, such as
    /// `WIN32 (7)`. Just the number when the facility isn't one this crate names.
    pub facility: Option<String>,
    /// The text from Windows on one line, with runs of whitespace made single
    /// spaces. Insert placeholders such as `%1` become `…`, since there's
    /// nothing to fill them with.
    pub message: String,
    /// Where the text came from: `system`, or a DLL's file name such as
    /// `ntdll.dll`.
    pub source: String,
}

/// A code-like token found in a log message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundCode {
    /// The token's byte range in the message.
    pub range: std::ops::Range<usize>,
    /// The token itself, ready to pass to [`lookup`].
    pub text: String,
}
