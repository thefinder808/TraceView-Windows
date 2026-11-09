//! Works out what a 32-bit value could mean, looks each reading up, and puts the
//! most likely first.
//!
//! The top two bits do most of the sorting. HRESULT failures set only the top
//! bit, so values starting 0x8 to 0xB read as HRESULTs first. NTSTATUS errors set
//! both, so values starting 0xC to 0xF read as NTSTATUS first. Values that fit in
//! 16 bits are Win32 codes. Every other reading that Windows has text for is kept
//! too, because the same value often means different things in different places.

use crate::messages::{self, Table};
use crate::parse::parse_values;
use crate::{find_codes, CodeKind, Decoded};
use std::ops::RangeInclusive;

/// The bit `HRESULT_FROM_NT` sets to mark an NTSTATUS wrapped in an HRESULT.
const WRAPPED_NTSTATUS: u32 = 0x1000_0000;
/// The largest Win32 code. Wrapping one in an HRESULT keeps only these 16 bits.
const LARGEST_WIN32_CODE: u32 = 0xFFFF;

/// COM interfaces' own codes. Many components share it, so only WMI's range
/// is looked up in WMI's table.
const FACILITY_ITF: u32 = 4;
const WMI_CODES: RangeInclusive<u32> = 0x8004_1000..=0x8004_2FFF;
const FACILITY_WIN32: u32 = 7;
const FACILITY_HTTP: u32 = 25;
const FACILITY_BACKGROUND_COPY: u32 = 32;
const FACILITY_WINDOWS_UPDATE: u32 = 36;
/// ConfigMgr's codes start 0x87D0 or 0x87D1.
const CONFIGMGR_FACILITIES: RangeInclusive<u32> = 0x7D0..=0x7D1;
/// The Win32 codes whose text lives in `winhttp.dll`.
const WINHTTP_CODES: RangeInclusive<u32> = 12001..=12199;

/// Most codes pulled out of pasted text that get decoded, so a huge paste can't
/// cause thousands of lookups.
const MOST_CODES_FROM_TEXT: usize = 16;

/// Decodes text a person typed or a code found in a log: "0x80070005",
/// "80070005", "-2147024891", "5", "0xC0000005". Returns every interpretation
/// that produced a message, most likely first. Empty if nothing decodes.
///
/// Text that isn't a code by itself, such as a pasted log line, is searched with
/// [`find_codes`], and the first few codes found are decoded in order.
pub fn lookup(text: &str) -> Vec<Decoded> {
    collect(text, false)
}

/// Like [`lookup`], but a value Windows has no text for still gets one entry:
/// its most likely reading with an empty message and source, so its hex, signed
/// and facility forms can be shown. Windows Update codes need this.
pub fn lookup_all(text: &str) -> Vec<Decoded> {
    collect(text, true)
}

fn collect(text: &str, keep_unexplained: bool) -> Vec<Decoded> {
    let mut values = parse_values(text);
    if values.is_empty() {
        for found in find_codes(text).iter().take(MOST_CODES_FROM_TEXT) {
            for value in parse_values(&found.text) {
                if !values.contains(&value) {
                    values.push(value);
                }
            }
        }
    }
    let mut results: Vec<Decoded> = Vec::new();
    for value in values {
        let mut decoded = decode_value(value);
        if decoded.is_empty() && keep_unexplained {
            if let Some(reading) = readings(value).first() {
                decoded.push(describe(reading.kind, reading.code, String::new(), ""));
            }
        }
        for decoded in decoded {
            let already_listed = results
                .iter()
                .any(|listed| listed.kind == decoded.kind && listed.code == decoded.code);
            if !already_listed {
                results.push(decoded);
            }
        }
    }
    results
}

/// One way to read a value: its kind, the code to look up, and which table to
/// look in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Reading {
    pub kind: CodeKind,
    pub code: u32,
    pub table: Table,
}

impl Reading {
    fn new(kind: CodeKind, code: u32, table: Table) -> Self {
        Self { kind, code, table }
    }
}

/// The HRESULT facility field. Eleven bits, so the flag bits above it, such as
/// the one marking a wrapped NTSTATUS, aren't counted.
pub(crate) fn facility(code: u32) -> u32 {
    (code >> 16) & 0x7FF
}

/// Names a facility as "NAME (number)", or just the number if it isn't one of
/// the common ones named here.
pub(crate) fn facility_name(facility: u32) -> String {
    match facility_label(facility) {
        Some(label) => format!("{label} ({facility})"),
        None => facility.to_string(),
    }
}

/// Short names for the facilities most often seen in logs.
fn facility_label(facility: u32) -> Option<&'static str> {
    let label = match facility {
        0 => "NULL",
        1 => "RPC",
        2 => "DISPATCH",
        3 => "STORAGE",
        4 => "ITF",
        7 => "WIN32",
        8 => "WINDOWS",
        9 => "SECURITY",
        10 => "CONTROL",
        11 => "CERT",
        12 => "INTERNET",
        13 => "MEDIASERVER",
        14 => "MSMQ",
        15 => "SETUPAPI",
        16 => "SCARD",
        17 => "COMPLUS",
        19 => "URT",
        23 => "SXS",
        25 => "HTTP",
        32 => "BACKGROUNDCOPY",
        33 => "CONFIGURATION",
        34 => "STATE_MANAGEMENT",
        36 => "WINDOWSUPDATE",
        37 => "DIRECTORYSERVICE",
        38 => "GRAPHICS",
        39 => "SHELL",
        40 => "TPM_SERVICES",
        41 => "TPM_SOFTWARE",
        48 => "PLA",
        49 => "FVE",
        50 => "FWP",
        51 => "WINRM",
        52 => "NDIS",
        53 => "USERMODE_HYPERVISOR",
        55 => "USERMODE_VIRTUALIZATION",
        56 => "USERMODE_VOLMGR",
        57 => "BCD",
        58 => "USERMODE_VHD",
        60 => "SDIAG",
        61 => "WEBSERVICES",
        80 => "WINDOWS_DEFENDER",
        208 => "DELIVERY_OPTIMIZATION",
        _ => return None,
    };
    Some(label)
}

/// Every way to read a value, most likely first.
pub(crate) fn readings(code: u32) -> Vec<Reading> {
    let mut readings = Vec::new();
    if code <= LARGEST_WIN32_CODE {
        win32_readings(code, &mut readings);
        return readings;
    }
    let as_hresult = Reading::new(CodeKind::HResult, code, Table::System);
    let as_ntstatus = Reading::new(CodeKind::NtStatus, code, Table::Ntdll);
    let wrapped_ntstatus = (code & WRAPPED_NTSTATUS != 0)
        .then(|| Reading::new(CodeKind::NtStatus, code & !WRAPPED_NTSTATUS, Table::Ntdll));
    match code >> 30 {
        // An NTSTATUS error, or an HRESULT wrapping one.
        0b11 => {
            readings.extend(wrapped_ntstatus);
            readings.push(as_ntstatus);
            // NTSTATUS uses facility 7 to wrap Win32 codes too.
            if facility(code) == FACILITY_WIN32 {
                win32_readings(code & LARGEST_WIN32_CODE, &mut readings);
            }
            readings.push(as_hresult);
        }
        // A failure HRESULT, or an NTSTATUS warning.
        0b10 => {
            readings.push(as_hresult);
            facility_readings(code, &mut readings);
            readings.extend(wrapped_ntstatus);
            readings.push(as_ntstatus);
        }
        // An informational NTSTATUS.
        0b01 => {
            readings.push(as_ntstatus);
            readings.extend(wrapped_ntstatus);
            readings.push(as_hresult);
        }
        // A success HRESULT that isn't plain zero or one.
        _ => {
            readings.push(as_hresult);
            readings.extend(wrapped_ntstatus);
        }
    }
    readings
}

/// Readings for a Win32 code. WinHTTP's codes have their own table.
fn win32_readings(code: u32, readings: &mut Vec<Reading>) {
    if WINHTTP_CODES.contains(&code) {
        readings.push(Reading::new(CodeKind::WinHttp, code, Table::WinHttp));
    }
    readings.push(Reading::new(CodeKind::Win32, code, Table::System));
}

/// Readings for a failure HRESULT whose facility has its own message table.
fn facility_readings(code: u32, readings: &mut Vec<Reading>) {
    match facility(code) {
        FACILITY_WIN32 => win32_readings(code & LARGEST_WIN32_CODE, readings),
        // Windows has no text for these, but CMTrace has some, in English.
        FACILITY_WINDOWS_UPDATE => {
            readings.push(Reading::new(
                CodeKind::WindowsUpdate,
                code,
                Table::WindowsUpdate,
            ));
            readings.push(Reading::new(CodeKind::WindowsUpdate, code, Table::CmTrace));
        }
        // BITS also reports the HTTP status codes it gets from servers.
        FACILITY_BACKGROUND_COPY | FACILITY_HTTP => {
            readings.push(Reading::new(CodeKind::Bits, code, Table::Bits));
        }
        // A console's text first, as it may be in the user's language, then
        // CMTrace's, which every ConfigMgr client has.
        number if CONFIGMGR_FACILITIES.contains(&number) => {
            readings.push(Reading::new(CodeKind::ConfigMgr, code, Table::ConfigMgr));
            readings.push(Reading::new(CodeKind::ConfigMgr, code, Table::CmTrace));
        }
        FACILITY_ITF if WMI_CODES.contains(&code) => {
            readings.push(Reading::new(CodeKind::Wmi, code, Table::Wmi));
        }
        _ => {}
    }
}

/// Looks up every reading of a value, keeping those Windows has text for.
fn decode_value(code: u32) -> Vec<Decoded> {
    let mut decoded: Vec<Decoded> = readings(code)
        .into_iter()
        .filter_map(|reading| {
            let message = messages::message(reading.table, reading.code)?;
            Some(describe(
                reading.kind,
                reading.code,
                message,
                reading.table.source(),
            ))
        })
        .collect();
    // An HRESULT wrapping a Win32 code means what that code means. The system
    // table knows most of them by their HRESULT too, but not codes whose text
    // lives in a DLL, such as WinHTTP's, so borrow the inner code's text.
    let wraps_win32 = code >> 30 == 0b10 && facility(code) == FACILITY_WIN32;
    let has_hresult = decoded.iter().any(|entry| entry.kind == CodeKind::HResult);
    if wraps_win32 && !has_hresult {
        let inner = decoded
            .iter()
            .find(|entry| entry.code == code & LARGEST_WIN32_CODE)
            .map(|entry| (entry.message.clone(), entry.source.clone()));
        if let Some((message, source)) = inner {
            decoded.insert(0, describe(CodeKind::HResult, code, message, &source));
        }
    }
    // A named code is explained by its name, even with no text: on its reading
    // with text, or else on its most specific reading.
    if let Some(name) = name_of(code) {
        match decoded.iter_mut().find(|entry| entry.code == code) {
            Some(entry) => entry.name = Some(name),
            None => {
                let all = readings(code);
                let best = all
                    .iter()
                    .find(|reading| reading.kind == CodeKind::WindowsUpdate)
                    .or(all.first());
                if let Some(reading) = best {
                    let mut named = describe(reading.kind, code, String::new(), "");
                    named.name = Some(name);
                    decoded.insert(0, named);
                }
            }
        }
    }
    decoded
}

/// The name of a Windows Update or Delivery Optimization code.
fn name_of(code: u32) -> Option<&'static str> {
    crate::names::NAMES
        .binary_search_by_key(&code, |(known, _)| *known)
        .ok()
        .map(|index| crate::names::NAMES[index].1)
}

fn describe(kind: CodeKind, code: u32, message: String, source: &str) -> Decoded {
    let has_facility = matches!(
        kind,
        CodeKind::HResult
            | CodeKind::WindowsUpdate
            | CodeKind::Bits
            | CodeKind::ConfigMgr
            | CodeKind::Wmi
    );
    Decoded {
        kind,
        code,
        hex: format!("0x{code:08X}"),
        // The same bits, read as signed.
        signed: code as i32,
        facility: has_facility.then(|| facility_name(facility(code))),
        message,
        source: source.to_string(),
        name: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(code: u32) -> Vec<(CodeKind, u32, Table)> {
        readings(code)
            .into_iter()
            .map(|reading| (reading.kind, reading.code, reading.table))
            .collect()
    }

    #[test]
    fn facilities_are_named_with_their_number() {
        assert_eq!(facility_name(7), "WIN32 (7)");
        assert_eq!(facility_name(0x24), "WINDOWSUPDATE (36)");
        assert_eq!(facility_name(2000), "2000");
        assert_eq!(facility(0x8007_0005), 7);
        assert_eq!(facility(0x8024_402C), 0x24);
        assert_eq!(facility(0x87D0_0324), 0x7D0);
        // The wrapped-NTSTATUS flag isn't part of the facility.
        assert_eq!(facility(0xD000_0022), 0);
    }

    #[test]
    fn small_values_read_as_win32() {
        assert_eq!(kinds(5), vec![(CodeKind::Win32, 5, Table::System)]);
        assert_eq!(
            kinds(12002),
            vec![
                (CodeKind::WinHttp, 12002, Table::WinHttp),
                (CodeKind::Win32, 12002, Table::System),
            ]
        );
    }

    #[test]
    fn win32_hresults_also_read_as_their_win32_code() {
        assert_eq!(
            kinds(0x8007_0005),
            vec![
                (CodeKind::HResult, 0x8007_0005, Table::System),
                (CodeKind::Win32, 5, Table::System),
                (CodeKind::NtStatus, 0x8007_0005, Table::Ntdll),
            ]
        );
        assert!(kinds(0x8007_2EE2).contains(&(CodeKind::WinHttp, 12002, Table::WinHttp)));
    }

    #[test]
    fn ntstatus_errors_read_as_ntstatus_first() {
        assert_eq!(
            kinds(0xC000_0005)[0],
            (CodeKind::NtStatus, 0xC000_0005, Table::Ntdll)
        );
    }

    #[test]
    fn wrapped_ntstatus_reads_as_the_inner_status() {
        assert_eq!(
            kinds(0xD000_0022)[0],
            (CodeKind::NtStatus, 0xC000_0022, Table::Ntdll)
        );
    }

    #[test]
    fn facilities_with_their_own_tables_are_routed() {
        assert!(kinds(0x8024_402C).contains(&(
            CodeKind::WindowsUpdate,
            0x8024_402C,
            Table::WindowsUpdate
        )));
        assert!(kinds(0x8020_0010).contains(&(CodeKind::Bits, 0x8020_0010, Table::Bits)));
        assert!(kinds(0x8019_0194).contains(&(CodeKind::Bits, 0x8019_0194, Table::Bits)));
        assert!(kinds(0x87D0_0324).contains(&(CodeKind::ConfigMgr, 0x87D0_0324, Table::ConfigMgr)));
        assert!(kinds(0x87D1_041C).contains(&(CodeKind::ConfigMgr, 0x87D1_041C, Table::ConfigMgr)));
        // CMTrace's text comes after a console's, and after Windows Update's own.
        for (code, kind, first) in [
            (0x87D0_0215, CodeKind::ConfigMgr, Table::ConfigMgr),
            (0x8024_0022, CodeKind::WindowsUpdate, Table::WindowsUpdate),
        ] {
            let all = kinds(code);
            let at = |table| {
                all.iter()
                    .position(|reading| *reading == (kind, code, table))
            };
            assert!(
                at(first).unwrap() < at(Table::CmTrace).unwrap(),
                "{code:#x}"
            );
        }
        assert!(kinds(0x8004_1010).contains(&(CodeKind::Wmi, 0x8004_1010, Table::Wmi)));
        // Other components' codes in the same facility aren't WMI's.
        assert!(!kinds(0x8004_0154)
            .iter()
            .any(|(kind, _, _)| *kind == CodeKind::Wmi));
    }

    #[test]
    fn the_names_table_is_sorted_with_one_name_per_code() {
        let names = crate::names::NAMES;
        assert!(names.len() > 500);
        assert!(names.windows(2).all(|pair| pair[0].0 < pair[1].0));
    }

    #[test]
    fn codes_windows_has_no_text_for_are_named() {
        for (code, kind, name) in [
            (
                "0x80240022",
                CodeKind::WindowsUpdate,
                "WU_E_ALL_UPDATES_FAILED",
            ),
            (
                "0x8024402C",
                CodeKind::WindowsUpdate,
                "WU_E_PT_WINHTTP_NAME_NOT_RESOLVED",
            ),
            ("0x80D02002", CodeKind::HResult, "DO_E_DOWNLOAD_NO_PROGRESS"),
        ] {
            let found = lookup(code);
            assert_eq!(found[0].kind, kind, "{code}");
            assert_eq!(found[0].name, Some(name), "{code}");
            assert!(found[0].message.is_empty());
        }
        // A code Windows explains has no name.
        assert!(lookup("0x80070005")
            .iter()
            .all(|found| found.name.is_none()));
    }

    #[test]
    fn decoded_forms_are_consistent() {
        let decoded = describe(CodeKind::HResult, 0x8007_0005, "x".into(), "system");
        assert_eq!(decoded.hex, "0x80070005");
        assert_eq!(decoded.signed, -2_147_024_891);
        assert_eq!(decoded.facility.as_deref(), Some("WIN32 (7)"));
        let decoded = describe(CodeKind::Win32, 5, "x".into(), "system");
        assert_eq!(decoded.hex, "0x00000005");
        assert_eq!(decoded.signed, 5);
        assert_eq!(decoded.facility, None);
    }
}
