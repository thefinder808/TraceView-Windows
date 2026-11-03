//! Registers TraceView, for this Windows user only, as a program that opens
//! `.log` and `.lo_` files, and takes that back. Windows doesn't let a program
//! make itself the default, so after registering, Windows' Default apps page
//! opens at TraceView for the person to pick it. Nothing here runs unless
//! they ask, and it needs no administrator rights: every key is under
//! `HKEY_CURRENT_USER`.

use std::io;
use std::path::{Path, PathBuf};
use windows::core::{w, HSTRING};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::System::Registry::{
    RegDeleteKeyValueW, RegDeleteKeyW, RegDeleteTreeW, RegGetValueW, RegSetKeyValueW,
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ,
};
use windows::Win32::UI::Shell::{SHChangeNotify, ShellExecuteW, SHCNE_ASSOCCHANGED, SHCNF_IDLIST};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

/// The file types TraceView offers to open. `.lo_` is how SCCM names a log
/// it has rotated.
pub const FILE_TYPES: [&str; 2] = [".log", ".lo_"];
const PROG_ID: &str = "TraceView.Log";
const APP_NAME: &str = "TraceView";
/// Where everything goes under `HKEY_CURRENT_USER`. Tests use a key of their own.
const ROOT: &str = "Software";

/// One value to write: its key under `HKEY_CURRENT_USER`, its name (empty for
/// the key's default value), and its text.
#[derive(Debug, PartialEq, Eq)]
struct Value {
    key: String,
    name: &'static str,
    data: String,
}

fn value(key: String, name: &'static str, data: &str) -> Value {
    Value {
        key,
        name,
        data: data.to_string(),
    }
}

fn classes(root: &str) -> String {
    format!(r"{root}\Classes")
}

fn capabilities(root: &str) -> String {
    format!(r"{root}\{APP_NAME}\Capabilities")
}

fn command_key(root: &str) -> String {
    format!(r"{}\{PROG_ID}\shell\open\command", classes(root))
}

/// Everything registering writes: a file type that opens with this EXE, the
/// extensions that list it under Open with, and the capabilities that put
/// TraceView on Windows' Default apps page.
fn values(root: &str, exe: &Path) -> Vec<Value> {
    let classes = classes(root);
    let capabilities = capabilities(root);
    let mut list = vec![
        value(format!(r"{classes}\{PROG_ID}"), "", "Log file"),
        value(
            command_key(root),
            "",
            &format!("\"{}\" \"%1\"", exe.display()),
        ),
        value(capabilities.clone(), "ApplicationName", APP_NAME),
        value(
            capabilities.clone(),
            "ApplicationDescription",
            "A fast log viewer for Windows, in the spirit of CMTrace",
        ),
        value(
            format!(r"{root}\RegisteredApplications"),
            APP_NAME,
            &capabilities,
        ),
    ];
    for extension in FILE_TYPES {
        list.push(value(
            format!(r"{classes}\{extension}\OpenWithProgids"),
            PROG_ID,
            "",
        ));
        list.push(value(
            format!(r"{capabilities}\FileAssociations"),
            extension,
            PROG_ID,
        ));
    }
    list
}

/// The program a registration's command opens: the quoted path at its start.
fn program_of(command: &str) -> Option<PathBuf> {
    let rest = command.trim().strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(PathBuf::from(&rest[..end]))
}

fn check(status: WIN32_ERROR) -> io::Result<()> {
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(status.0 as i32))
    }
}

/// Like `check`, but something already gone is fine.
fn check_removed(status: WIN32_ERROR) -> io::Result<()> {
    if status == ERROR_FILE_NOT_FOUND {
        Ok(())
    } else {
        check(status)
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Writes a value, making its key, and the keys above it, if they aren't there.
fn write(value: &Value) -> io::Result<()> {
    let data = wide(&value.data);
    // SAFETY: the strings and data outlive the call, and the size given is the
    // data's, in bytes, ending zero included.
    check(unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(value.key.as_str()),
            &HSTRING::from(value.name),
            REG_SZ.0,
            Some(data.as_ptr().cast()),
            (data.len() * 2) as u32,
        )
    })
}

fn register_under(root: &str, exe: &Path) -> io::Result<()> {
    values(root, exe).iter().try_for_each(write)
}

fn unregister_under(root: &str) -> io::Result<()> {
    let classes = classes(root);
    // SAFETY: each string outlives its call.
    unsafe {
        check_removed(RegDeleteTreeW(
            HKEY_CURRENT_USER,
            &HSTRING::from(format!(r"{classes}\{PROG_ID}")),
        ))?;
        // Only TraceView's value goes: other programs list themselves here too.
        for extension in FILE_TYPES {
            check_removed(RegDeleteKeyValueW(
                HKEY_CURRENT_USER,
                &HSTRING::from(format!(r"{classes}\{extension}\OpenWithProgids")),
                &HSTRING::from(PROG_ID),
            ))?;
        }
        check_removed(RegDeleteKeyValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(format!(r"{root}\RegisteredApplications")),
            &HSTRING::from(APP_NAME),
        ))?;
        check_removed(RegDeleteTreeW(
            HKEY_CURRENT_USER,
            &HSTRING::from(capabilities(root)),
        ))?;
        // The key above the capabilities goes only if nothing else is in it.
        let _ = RegDeleteKeyW(
            HKEY_CURRENT_USER,
            &HSTRING::from(format!(r"{root}\{APP_NAME}")),
        );
    }
    Ok(())
}

fn registered_under(root: &str) -> Option<PathBuf> {
    let mut buffer = vec![0u16; 2048];
    let mut size = (buffer.len() * 2) as u32;
    // SAFETY: the buffer and its size in bytes are passed together, and the
    // strings outlive the call.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(command_key(root)),
            None,
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let length = (size as usize / 2).saturating_sub(1).min(buffer.len());
    program_of(&String::from_utf16_lossy(&buffer[..length]))
}

/// Tells Explorer that file types changed, so icons and Open with catch up.
fn announce() {
    // SAFETY: no item lists are passed with this event.
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
}

/// Registers this EXE for [`FILE_TYPES`], then opens Default apps at
/// TraceView, where the person can make it the default.
pub fn register_file_types() -> io::Result<()> {
    let exe = std::env::current_exe()?;
    register_under(ROOT, &exe)?;
    announce();
    // SAFETY: the strings outlive the call, and no parent window is needed.
    // Windows 10 opens Default apps itself, without going to TraceView.
    unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            w!("ms-settings:defaultapps?registeredAppUser=TraceView"),
            None,
            None,
            SW_SHOWNORMAL,
        );
    }
    Ok(())
}

/// Takes the registration back. A file type whose default was TraceView
/// asks again which program to use.
pub fn unregister_file_types() -> io::Result<()> {
    unregister_under(ROOT)?;
    announce();
    Ok(())
}

/// The EXE this user's registration opens, if TraceView is registered.
pub fn registered_file_types() -> Option<PathBuf> {
    registered_under(ROOT)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A key of its own under `HKEY_CURRENT_USER\Software`, removed afterwards,
    /// so the real registration is never touched.
    struct Scratch(String);

    impl Scratch {
        fn new(name: &str) -> Self {
            Self(format!(
                r"Software\TraceView tests\{name} {}",
                std::process::id()
            ))
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            // SAFETY: the string outlives the call.
            unsafe {
                let _ = RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(self.0.as_str()));
                let _ = RegDeleteKeyW(HKEY_CURRENT_USER, w!(r"Software\TraceView tests"));
            }
        }
    }

    #[test]
    fn the_registration_opens_this_exe_for_each_type() {
        let exe = Path::new(r"C:\Tools\TraceView\traceview.exe");
        let values = values("Software", exe);
        let command = values
            .iter()
            .find(|value| value.key.ends_with(r"shell\open\command"))
            .unwrap();
        assert_eq!(command.data, r#""C:\Tools\TraceView\traceview.exe" "%1""#);
        assert_eq!(
            command.key,
            r"Software\Classes\TraceView.Log\shell\open\command"
        );
        for extension in FILE_TYPES {
            assert!(values.iter().any(|value| {
                value.key == format!(r"Software\Classes\{extension}\OpenWithProgids")
                    && value.name == PROG_ID
            }));
        }
        let registered = values.iter().find(|value| value.name == APP_NAME).unwrap();
        assert_eq!(registered.data, r"Software\TraceView\Capabilities");
        assert_eq!(program_of(&command.data).as_deref(), Some(exe));
        assert_eq!(program_of("traceview.exe %1"), None);
    }

    #[test]
    fn registering_and_taking_it_back_round_trips() {
        let scratch = Scratch::new("round trip");
        let exe = Path::new(r"C:\Tools\Trace View\traceview.exe");
        assert_eq!(registered_under(&scratch.0), None);
        register_under(&scratch.0, exe).unwrap();
        assert_eq!(registered_under(&scratch.0).as_deref(), Some(exe));
        unregister_under(&scratch.0).unwrap();
        assert_eq!(registered_under(&scratch.0), None);
        unregister_under(&scratch.0).unwrap();
    }
}
