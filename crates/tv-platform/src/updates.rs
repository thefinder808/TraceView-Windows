//! The Windows parts of auto-updates: downloading over HTTPS, SHA-256, and
//! reading who signed an EXE and its version. Whether to update, and what
//! to check, is the app's to decide.

use std::ffi::c_void;
use std::mem::size_of;
use std::path::Path;
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{ERROR_SUCCESS, HANDLE, HWND, INVALID_HANDLE_VALUE};
use windows::Win32::Networking::WinHttp::{
    WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders,
    WinHttpQueryOption, WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest,
    WinHttpSetOption, WinHttpSetTimeouts, INTERNET_DEFAULT_HTTPS_PORT,
    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE, WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2,
    WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_3, WINHTTP_OPTION_REDIRECT_POLICY,
    WINHTTP_OPTION_REDIRECT_POLICY_DISALLOW_HTTPS_TO_HTTP, WINHTTP_OPTION_SECURE_PROTOCOLS,
    WINHTTP_OPTION_URL, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE,
};
use windows::Win32::Security::Cryptography::{
    BCryptHash, CertNameToStrW, BCRYPT_SHA256_ALG_HANDLE, CERT_X500_NAME_STR, CRYPT_INTEGER_BLOB,
    X509_ASN_ENCODING,
};
use windows::Win32::Security::WinTrust::{
    WTHelperGetProvSignerFromChain, WTHelperProvDataFromStateData, WinVerifyTrust,
    CRYPT_PROVIDER_CERT, WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0,
    WINTRUST_FILE_INFO, WTD_CHOICE_FILE, WTD_REVOCATION_CHECK_CHAIN, WTD_REVOKE_WHOLECHAIN,
    WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE,
};
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW, VS_FIXEDFILEINFO,
};
use windows::Win32::System::Registry::{
    RegGetValueW, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD,
};

/// How long a download may go without progress before it gives up.
const TIMEOUT_MS: i32 = 30_000;

/// A WinHTTP handle, closed when dropped.
struct Internet(*mut c_void);

impl Internet {
    fn new(handle: *mut c_void) -> Result<Self, String> {
        if handle.is_null() {
            Err(network_error(&windows::core::Error::from_thread()))
        } else {
            Ok(Self(handle))
        }
    }
}

impl Drop for Internet {
    fn drop(&mut self) {
        // SAFETY: the handle came from WinHTTP and is closed only here.
        unsafe {
            let _ = WinHttpCloseHandle(self.0);
        }
    }
}

/// What a download gave.
pub struct Download {
    pub bytes: Vec<u8>,
    /// Where it finally came from, after any redirects.
    pub final_url: String,
}

/// Downloads `https://{host}{path}` with Windows' own networking, which uses
/// this PC's proxy settings and TLS 1.2 or later. Redirects are followed
/// only to other HTTPS addresses. Fails on any status but 200, past
/// `max_bytes`, or after 30 seconds without progress.
pub fn https_get(
    host: &str,
    path: &str,
    user_agent: &str,
    max_bytes: usize,
) -> Result<Download, String> {
    let error = |error: windows::core::Error| network_error(&error);
    // SAFETY: every handle is checked before use and closed by `Internet`,
    // strings outlive the calls that read them, and each buffer pointer is
    // given with its true length.
    unsafe {
        let session = Internet::new(WinHttpOpen(
            &HSTRING::from(user_agent),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        ))?;
        WinHttpSetTimeouts(session.0, TIMEOUT_MS, TIMEOUT_MS, TIMEOUT_MS, TIMEOUT_MS)
            .map_err(error)?;
        let set = |option: u32, value: u32| {
            WinHttpSetOption(
                Some(session.0 as *const c_void),
                option,
                Some(&value.to_ne_bytes()),
            )
        };
        // Windows without TLS 1.3 refuses that flag, so 1.2 alone comes next.
        if set(
            WINHTTP_OPTION_SECURE_PROTOCOLS,
            WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2 | WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_3,
        )
        .is_err()
        {
            set(
                WINHTTP_OPTION_SECURE_PROTOCOLS,
                WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2,
            )
            .map_err(error)?;
        }
        set(
            WINHTTP_OPTION_REDIRECT_POLICY,
            WINHTTP_OPTION_REDIRECT_POLICY_DISALLOW_HTTPS_TO_HTTP,
        )
        .map_err(error)?;
        let connection = Internet::new(WinHttpConnect(
            session.0,
            &HSTRING::from(host),
            INTERNET_DEFAULT_HTTPS_PORT,
            0,
        ))?;
        let request = Internet::new(WinHttpOpenRequest(
            connection.0,
            w!("GET"),
            &HSTRING::from(path),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        ))?;
        WinHttpSendRequest(request.0, None, None, 0, 0, 0).map_err(error)?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut()).map_err(error)?;
        let (mut status, mut size) = (0u32, size_of::<u32>() as u32);
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some((&mut status as *mut u32).cast()),
            &mut size,
            std::ptr::null_mut(),
        )
        .map_err(error)?;
        if status != 200 {
            return Err(format!("the server answered {status}"));
        }
        let final_url = final_url(request.0);
        let mut bytes = Vec::new();
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            let mut read = 0u32;
            WinHttpReadData(
                request.0,
                chunk.as_mut_ptr().cast(),
                chunk.len() as u32,
                &mut read,
            )
            .map_err(error)?;
            if read == 0 {
                break;
            }
            if bytes.len() + read as usize > max_bytes {
                return Err(format!("the download is larger than {max_bytes} bytes"));
            }
            bytes.extend_from_slice(&chunk[..read as usize]);
        }
        Ok(Download { bytes, final_url })
    }
}

/// The URL a request ended up at, after redirects.
///
/// # Safety
///
/// `request` must be an open WinHTTP request handle.
unsafe fn final_url(request: *mut c_void) -> String {
    let mut size = 0u32;
    // SAFETY: the caller's handle is open, and the first call only asks for
    // the size, which the second call's buffer then has.
    unsafe {
        let _ = WinHttpQueryOption(request, WINHTTP_OPTION_URL, None, &mut size);
        let mut buffer = vec![0u16; (size as usize).div_ceil(2)];
        if WinHttpQueryOption(
            request,
            WINHTTP_OPTION_URL,
            Some(buffer.as_mut_ptr().cast()),
            &mut size,
        )
        .is_err()
        {
            return String::new();
        }
        let end = buffer
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(buffer.len());
        String::from_utf16_lossy(&buffer[..end])
    }
}

/// WinHTTP's own errors, which Windows has no text for, in plain words.
fn network_error(error: &windows::core::Error) -> String {
    let code = (error.code().0 as u32) & 0xFFFF;
    match code {
        12002 => "the connection timed out".to_string(),
        12007 => "the server's name couldn't be found, so this PC may be offline".to_string(),
        12029 | 12030 => "couldn't connect to the server".to_string(),
        12037 | 12038 | 12044 | 12045 | 12175 => "the secure connection failed".to_string(),
        _ => match error.message() {
            message if !message.trim().is_empty() => message.trim().to_string(),
            _ => format!("network error {code}"),
        },
    }
}

/// SHA-256 of `bytes`, as lowercase hex, from Windows' own cryptography.
pub fn sha256_hex(bytes: &[u8]) -> Result<String, String> {
    let mut digest = [0u8; 32];
    // SAFETY: the input and output are live slices, given with their lengths.
    unsafe { BCryptHash(BCRYPT_SHA256_ALG_HANDLE, None, bytes, &mut digest) }
        .ok()
        .map_err(|error| format!("couldn't compute SHA-256: {}", error.message()))?;
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Who signed a file, from a signature Windows trusts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signer {
    /// The signing certificate's subject, such as `CN=…, O=…, C=US`.
    pub subject: String,
    /// The subject of the root certificate its chain ends at.
    pub root: String,
}

/// Checks the signature inside a file the way Windows does before running
/// it, including whether any certificate in its chain was revoked, and says
/// who signed it. A file whose signature Windows doesn't trust, or that has
/// none, is an error. Signatures kept in Windows' catalogs, as many of
/// Windows' own files have, aren't read.
pub fn signer(path: &Path) -> Result<Signer, String> {
    let path = HSTRING::from(path.as_os_str());
    let mut file = WINTRUST_FILE_INFO {
        cbStruct: size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: PCWSTR(path.as_ptr()),
        ..Default::default()
    };
    let mut data = WINTRUST_DATA {
        cbStruct: size_of::<WINTRUST_DATA>() as u32,
        dwUIChoice: WTD_UI_NONE,
        fdwRevocationChecks: WTD_REVOKE_WHOLECHAIN,
        dwUnionChoice: WTD_CHOICE_FILE,
        Anonymous: WINTRUST_DATA_0 { pFile: &mut file },
        dwStateAction: WTD_STATEACTION_VERIFY,
        dwProvFlags: WTD_REVOCATION_CHECK_CHAIN,
        ..Default::default()
    };
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    let no_window = HWND(INVALID_HANDLE_VALUE.0);
    // SAFETY: `data` and `file` point at live locals and the path's string,
    // which outlive both calls. The state the first call opens is read only
    // between them and released by the second.
    unsafe {
        let result = WinVerifyTrust(
            no_window,
            &mut action,
            (&mut data as *mut WINTRUST_DATA).cast(),
        );
        let signer = if result == 0 {
            read_signer(data.hWVTStateData)
        } else {
            Err(trust_error(result))
        };
        data.dwStateAction = WTD_STATEACTION_CLOSE;
        let _ = WinVerifyTrust(
            no_window,
            &mut action,
            (&mut data as *mut WINTRUST_DATA).cast(),
        );
        signer
    }
}

/// The signer's certificate chain, from a successful check's state.
///
/// # Safety
///
/// `state` must be the state of a check that succeeded and isn't closed yet.
unsafe fn read_signer(state: HANDLE) -> Result<Signer, String> {
    let unreadable = || "its signer couldn't be read".to_string();
    // SAFETY: the caller's state is open, and each pointer the helpers give
    // is checked for null before it's read. The chain lives with the state.
    unsafe {
        let provider = WTHelperProvDataFromStateData(state);
        if provider.is_null() {
            return Err(unreadable());
        }
        let signer = WTHelperGetProvSignerFromChain(provider, 0, false, 0);
        let Some(signer) = signer.as_ref() else {
            return Err(unreadable());
        };
        if signer.pasCertChain.is_null() || signer.csCertChain == 0 {
            return Err(unreadable());
        }
        let chain = std::slice::from_raw_parts(signer.pasCertChain, signer.csCertChain as usize);
        let subject = |cert: &CRYPT_PROVIDER_CERT| -> Option<String> {
            let context = cert.pCert.as_ref()?;
            let info = context.pCertInfo.as_ref()?;
            Some(name(&info.Subject))
        };
        Ok(Signer {
            subject: subject(&chain[0]).ok_or_else(unreadable)?,
            root: subject(&chain[chain.len() - 1]).ok_or_else(unreadable)?,
        })
    }
}

/// A certificate name as text, such as `CN=…, O=…, C=US`.
///
/// # Safety
///
/// `blob` must be an encoded name from a live certificate.
unsafe fn name(blob: &CRYPT_INTEGER_BLOB) -> String {
    // SAFETY: the first call only measures, and the buffer then fits.
    unsafe {
        let length = CertNameToStrW(X509_ASN_ENCODING, blob, CERT_X500_NAME_STR, None);
        let mut buffer = vec![0u16; length as usize];
        let written = CertNameToStrW(
            X509_ASN_ENCODING,
            blob,
            CERT_X500_NAME_STR,
            Some(&mut buffer),
        );
        String::from_utf16_lossy(&buffer[..(written as usize).saturating_sub(1)])
    }
}

/// Why Windows doesn't trust a signature, in plain words.
fn trust_error(result: i32) -> String {
    match result as u32 {
        0x800B_0100 => "it isn't signed".to_string(),
        0x800B_0003 => "it isn't a program Windows can check".to_string(),
        0x8009_6010 => "its signature doesn't match its contents".to_string(),
        0x800B_010C => "its signing certificate was revoked".to_string(),
        0x8009_2013 => "Windows couldn't check whether its certificate was revoked".to_string(),
        0x800B_0101 => "its signing certificate has expired".to_string(),
        0x800B_0109 => "its signature doesn't lead to a root Windows trusts".to_string(),
        code => format!("Windows doesn't trust its signature (0x{code:08X})"),
    }
}

/// Where an administrator can turn updates off, for the PC or for one user,
/// as Group Policy and Intune set policies.
pub const POLICY_KEY: &str = r"SOFTWARE\Policies\TraceView";
/// A DWORD under [`POLICY_KEY`], which turns updates off when it isn't 0.
pub const POLICY_VALUE: &str = "DisableUpdates";

/// Whether an administrator turned updates off, under `HKEY_LOCAL_MACHINE`
/// or `HKEY_CURRENT_USER`.
pub fn turned_off_by_policy() -> bool {
    [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER]
        .into_iter()
        .any(|root| {
            let (mut value, mut size) = (0u32, size_of::<u32>() as u32);
            // SAFETY: the strings outlive the call, and the buffer is a live u32
            // whose size is given.
            let status = unsafe {
                RegGetValueW(
                    root,
                    &HSTRING::from(POLICY_KEY),
                    &HSTRING::from(POLICY_VALUE),
                    RRF_RT_REG_DWORD,
                    None,
                    Some((&mut value as *mut u32).cast()),
                    Some(&mut size),
                )
            };
            status == ERROR_SUCCESS && value != 0
        })
}

/// The major, minor and patch numbers in a file's version resource.
pub fn file_version(path: &Path) -> Option<[u16; 3]> {
    let path = HSTRING::from(path.as_os_str());
    // SAFETY: the buffer is the size Windows asked for, and the fixed info
    // it points into is checked for length and read without assuming
    // alignment.
    unsafe {
        let size = GetFileVersionInfoSizeW(&path, None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(&path, None, size, data.as_mut_ptr().cast()).ok()?;
        let (mut info, mut length) = (std::ptr::null_mut::<c_void>(), 0u32);
        if !VerQueryValueW(data.as_ptr().cast(), w!("\\"), &mut info, &mut length).as_bool()
            || info.is_null()
            || (length as usize) < size_of::<VS_FIXEDFILEINFO>()
        {
            return None;
        }
        let info = std::ptr::read_unaligned(info as *const VS_FIXEDFILEINFO);
        if info.dwSignature != 0xFEEF_04BD {
            return None;
        }
        Some([
            (info.dwFileVersionMS >> 16) as u16,
            info.dwFileVersionMS as u16,
            (info.dwFileVersionLS >> 16) as u16,
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn system32() -> PathBuf {
        PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into()))
            .join("System32")
    }

    #[test]
    fn sha256_gives_the_standard_digests() {
        assert_eq!(
            sha256_hex(b"").unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc").unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn a_signed_exe_names_its_signer_and_a_changed_copy_is_refused() {
        // Most of Windows' files are signed in catalogs, but some EXEs carry
        // their signature inside, as TraceView's do.
        let mut exes: Vec<PathBuf> = std::fs::read_dir(system32())
            .unwrap()
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
            })
            .collect();
        exes.sort();
        let (path, signed_by) = exes
            .iter()
            .find_map(|path| signer(path).ok().map(|signed_by| (path, signed_by)))
            .expect("System32 has an EXE signed inside the file");
        assert!(
            signed_by.subject.contains("O=Microsoft Corporation"),
            "{signed_by:?}"
        );
        assert!(signed_by.root.contains("Microsoft"), "{signed_by:?}");

        let dir = tempfile::tempdir().unwrap();
        let mut bytes = std::fs::read(path).unwrap();
        let middle = bytes.len() / 2;
        bytes[middle] ^= 0xFF;
        let changed = dir.path().join("changed.exe");
        std::fs::write(&changed, &bytes).unwrap();
        assert!(signer(&changed).is_err());

        // This test's own program is a real, unsigned EXE.
        let unsigned = std::env::current_exe().unwrap();
        assert_eq!(signer(&unsigned), Err("it isn't signed".to_string()));
        let text = dir.path().join("text.exe");
        std::fs::write(&text, b"not really a program").unwrap();
        assert!(signer(&text).is_err());
    }

    #[test]
    fn a_system_exe_has_a_version() {
        let version = file_version(&system32().join("cmd.exe")).expect("cmd.exe has a version");
        assert!(version[0] >= 6, "{version:?}");
        let dir = tempfile::tempdir().unwrap();
        let text = dir.path().join("text.exe");
        std::fs::write(&text, b"not a program").unwrap();
        assert_eq!(file_version(&text), None);
    }

    /// Reaches GitHub, so it only runs when asked for, with `--ignored`.
    #[test]
    #[ignore]
    fn downloads_the_latest_releases_checksums() {
        let download = https_get(
            "github.com",
            "/thefinder808/TraceView-Windows/releases/latest/download/SHA256SUMS.txt",
            "TraceView-test",
            64 * 1024,
        )
        .unwrap();
        let text = String::from_utf8(download.bytes).unwrap();
        assert!(text.contains("TraceView-"), "{text}");
        assert!(
            download.final_url.starts_with("https://"),
            "{}",
            download.final_url
        );
        assert!(https_get(
            "github.com",
            "/thefinder808/TraceView-Windows/no-such-page",
            "TraceView-test",
            1024
        )
        .is_err());
    }
}
