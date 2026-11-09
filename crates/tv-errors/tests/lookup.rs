//! Decodes real codes against this machine's Windows. Message text depends on
//! the user's language, so these check that messages exist, never what they say.

use tv_errors::{find_codes, lookup, CodeKind, Decoded};

fn first(text: &str) -> Decoded {
    lookup(text)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("{text:?} should decode"))
}

fn find(results: &[Decoded], kind: CodeKind, code: u32) -> &Decoded {
    results
        .iter()
        .find(|decoded| decoded.kind == kind && decoded.code == code)
        .unwrap_or_else(|| panic!("no {kind:?} 0x{code:08X} in {results:#?}"))
}

fn assert_tidy(decoded: &Decoded) {
    let message = &decoded.message;
    assert!(!message.is_empty(), "{decoded:?}");
    assert_eq!(message.trim(), message, "{decoded:?}");
    assert!(!message.contains("  "), "{decoded:?}");
    assert!(!message.contains(['\r', '\n', '\t']), "{decoded:?}");
    assert!(!decoded.source.is_empty());
}

#[test]
fn win32_hresult_decodes_both_ways() {
    let results = lookup("0x80070005");
    let hresult = &results[0];
    assert_eq!(hresult.kind, CodeKind::HResult);
    assert_eq!(hresult.code, 0x8007_0005);
    assert_eq!(hresult.hex, "0x80070005");
    assert_eq!(hresult.signed, -2_147_024_891);
    assert_eq!(hresult.facility.as_deref(), Some("WIN32 (7)"));
    assert_eq!(hresult.source, "system");
    assert_tidy(hresult);

    let win32 = find(&results, CodeKind::Win32, 5);
    assert_eq!(win32.hex, "0x00000005");
    assert_eq!(win32.signed, 5);
    assert_eq!(win32.facility, None);
    assert_tidy(win32);
}

#[test]
fn every_written_form_of_a_code_agrees() {
    for text in ["0x80070005", "80070005", "-2147024891", "2147942405"] {
        let decoded = first(text);
        assert_eq!(decoded.code, 0x8007_0005, "{text:?}");
        assert_eq!(decoded.kind, CodeKind::HResult, "{text:?}");
    }
}

#[test]
fn small_decimal_is_win32() {
    let decoded = first("5");
    assert_eq!(decoded.kind, CodeKind::Win32);
    assert_eq!(decoded.code, 5);
    assert_eq!(decoded.source, "system");
    assert_tidy(&decoded);
}

#[test]
fn ntstatus_comes_from_ntdll() {
    let decoded = first("0xC0000005");
    assert_eq!(decoded.kind, CodeKind::NtStatus);
    assert_eq!(decoded.code, 0xC000_0005);
    assert_eq!(decoded.signed, -1_073_741_819);
    assert_eq!(decoded.source, "ntdll.dll");
    assert_eq!(decoded.facility, None);
    assert_tidy(&decoded);
}

#[test]
fn hresult_from_nt_decodes_the_inner_status() {
    let decoded = first("0xD0000022");
    assert_eq!(decoded.kind, CodeKind::NtStatus);
    assert_eq!(decoded.code, 0xC000_0022);
    assert_eq!(decoded.source, "ntdll.dll");
}

#[test]
fn a_value_with_two_meanings_returns_both() {
    // An HRESULT in the system table, and an NTSTATUS warning in ntdll.
    let results = lookup("0x80000005");
    assert_eq!(results[0].kind, CodeKind::HResult);
    assert_tidy(find(&results, CodeKind::NtStatus, 0x8000_0005));
}

#[test]
fn winhttp_codes_come_from_winhttp() {
    let decoded = first("12002");
    assert_eq!(decoded.kind, CodeKind::WinHttp);
    assert_eq!(decoded.source, "winhttp.dll");
    assert_tidy(&decoded);

    // The system table has no text for this HRESULT, so it borrows WinHTTP's.
    let results = lookup("0x80072EE2");
    assert_eq!(results[0].kind, CodeKind::HResult);
    assert_eq!(results[0].facility.as_deref(), Some("WIN32 (7)"));
    assert_eq!(results[0].source, "winhttp.dll");
    let inner = find(&results, CodeKind::WinHttp, 12002);
    assert_eq!(inner.message, results[0].message);
}

#[test]
fn bits_codes_come_from_qmgr() {
    let results = lookup("0x80200010");
    let bits = find(&results, CodeKind::Bits, 0x8020_0010);
    assert_eq!(bits.source, "qmgr.dll");
    assert_eq!(bits.facility.as_deref(), Some("BACKGROUNDCOPY (32)"));
    assert_tidy(bits);

    // BITS also explains the HTTP statuses servers send it.
    let results = lookup("0x80190194");
    let bits = find(&results, CodeKind::Bits, 0x8019_0194);
    assert_eq!(bits.facility.as_deref(), Some("HTTP (25)"));
}

#[test]
fn windows_update_and_configmgr_codes_never_fail() {
    // Their tables may be missing or lack these codes, but any result must
    // still be well formed: tidy text, or else a name in its place.
    for text in ["0x8024402C", "0x80240022", "0x87D00324", "0x87D1041C"] {
        for decoded in lookup(text) {
            if decoded.message.is_empty() {
                assert!(decoded.name.is_some(), "{decoded:?}");
            } else {
                assert_tidy(&decoded);
            }
        }
    }
    assert_eq!(
        first("0x8024402C").name,
        Some("WU_E_PT_WINHTTP_NAME_NOT_RESOLVED")
    );
}

#[test]
fn configmgr_clients_explain_configmgr_codes() {
    // Every ConfigMgr client has CMTrace, whose text explains these. Most PCs
    // that run the tests aren't clients, so there's nothing to check there.
    let windows = std::env::var_os("SystemRoot").expect("Windows sets SystemRoot");
    if !std::path::Path::new(&windows)
        .join(r"CCM\CMTrace.exe")
        .is_file()
    {
        return;
    }
    for (text, code) in [("0x87D00215", 0x87D0_0215), ("0x87D00324", 0x87D0_0324)] {
        assert_tidy(find(&lookup(text), CodeKind::ConfigMgr, code));
    }
}

#[test]
fn wmi_codes_decode_from_wmi_text() {
    let decoded = first("0x80041010");
    assert_eq!(decoded.kind, CodeKind::Wmi);
    assert_eq!(decoded.source, "wmiutils.dll");
    assert_eq!(decoded.facility.as_deref(), Some("ITF (4)"));
    assert_tidy(&decoded);
}

#[test]
fn pasted_log_text_is_searched() {
    let decoded = first("Download failed, hr=0x80070005 after 3 tries");
    assert_eq!(decoded.code, 0x8007_0005);
}

#[test]
fn non_codes_decode_to_nothing() {
    for text in [
        "",
        "   ",
        "hello",
        "0x",
        "0x123456789",
        "4294967296",
        "KB5034441",
    ] {
        assert!(lookup(text).is_empty(), "{text:?}");
    }
}

#[test]
fn found_codes_decode() {
    let message = "Failed (0x80070005), error 2, status 0xC0000022";
    let found = find_codes(message);
    let texts: Vec<&str> = found.iter().map(|code| code.text.as_str()).collect();
    assert_eq!(texts, ["0x80070005", "2", "0xC0000022"]);
    for code in &found {
        assert!(!lookup(&code.text).is_empty(), "{code:?}");
    }
}

#[test]
fn lookups_from_many_threads_share_the_cache() {
    let threads: Vec<_> = (0..8)
        .map(|_| std::thread::spawn(|| first("0xC0000005").source))
        .collect();
    for thread in threads {
        assert_eq!(thread.join().expect("no panic"), "ntdll.dll");
    }
}

#[test]
fn lookup_all_still_gives_the_forms_of_codes_without_text() {
    // Facility 255 isn't assigned, so no Windows has text for this.
    let text = "0x80FFFFFF";
    assert!(lookup(text).is_empty());
    let results = tv_errors::lookup_all(text);
    assert_eq!(results.len(), 1);
    let only = &results[0];
    assert_eq!(only.kind, CodeKind::HResult);
    assert_eq!(only.hex, "0x80FFFFFF");
    assert_eq!(only.signed, -2_130_706_433);
    assert_eq!(only.facility.as_deref(), Some("255"));
    assert!(only.message.is_empty() && only.source.is_empty());
}

#[test]
fn lookup_all_matches_lookup_when_windows_has_text() {
    assert_eq!(tv_errors::lookup_all("0x80070005"), lookup("0x80070005"));
    assert!(tv_errors::lookup_all("not a code").is_empty());
}
