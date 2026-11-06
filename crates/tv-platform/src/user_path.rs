//! Adds a folder to this Windows user's PATH, and takes it back. Only the
//! user's own PATH changes, in `HKEY_CURRENT_USER\Environment`, so no
//! administrator rights are needed, and a folder goes at the end, where it
//! can't take over a command Windows or another program already has.
//!
//! The value is read and written as it's stored, so entries such as
//! `%USERPROFILE%\.dotnet\tools` keep their variables, and it stays the type
//! it was. Afterwards Windows is told the environment changed, so programs
//! started from then on see it. Programs already running keep their old PATH.

use std::io;
use std::path::Path;
use windows::core::{w, HSTRING};
use windows::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_SUCCESS, LPARAM, WIN32_ERROR, WPARAM,
};
use windows::Win32::System::Registry::{
    RegDeleteKeyW, RegDeleteTreeW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_EXPAND_SZ,
    REG_SZ, REG_VALUE_TYPE, RRF_NOEXPAND, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ,
};
use windows::Win32::UI::WindowsAndMessaging::{
    SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
};

/// Where Windows keeps the user's environment, under `HKEY_CURRENT_USER`.
const ENVIRONMENT: &str = "Environment";
/// The parent of the keys tests use instead.
const TESTS: &str = r"Software\TraceView tests";

/// The user's PATH, or a stand-in for tests that's deleted when dropped.
pub struct UserPath {
    key: String,
}

impl UserPath {
    /// The real one, which programs read.
    pub fn current() -> Self {
        Self {
            key: ENVIRONMENT.to_string(),
        }
    }

    /// A PATH of its own under `HKEY_CURRENT_USER\Software`, removed when
    /// dropped, so tests never touch the real one.
    #[doc(hidden)]
    pub fn for_tests(name: &str) -> Self {
        Self {
            key: format!(r"{TESTS}\{name} {}", std::process::id()),
        }
    }

    /// Whether the folder is on it, however it's written there.
    pub fn contains(&self, folder: &Path) -> bool {
        read(&self.key).is_ok_and(|(path, _)| {
            path.split(';')
                .any(|entry| same_folder(entry, &folder.to_string_lossy()))
        })
    }

    /// Adds the folder at the end. Returns false if it was already there.
    pub fn add(&self, folder: &Path) -> io::Result<bool> {
        let (path, kind) = read(&self.key)?;
        let Some(changed) = with_folder(&path, &folder.to_string_lossy()) else {
            return Ok(false);
        };
        write(&self.key, &changed, kind)?;
        self.announce();
        Ok(true)
    }

    /// Takes the folder off, however it's written. Returns false if it wasn't
    /// there.
    pub fn remove(&self, folder: &Path) -> io::Result<bool> {
        let (path, kind) = read(&self.key)?;
        let Some(changed) = without_folder(&path, &folder.to_string_lossy()) else {
            return Ok(false);
        };
        write(&self.key, &changed, kind)?;
        self.announce();
        Ok(true)
    }

    /// Tells running programs, Explorer above all, that the environment
    /// changed, so what they start from now on gets the new PATH.
    fn announce(&self) {
        if self.key != ENVIRONMENT {
            return;
        }
        // SAFETY: the string is static and outlives the call. Hung windows are
        // skipped, and the wait for the rest is capped at five seconds.
        unsafe {
            SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SETTINGCHANGE,
                WPARAM(0),
                LPARAM(w!("Environment").as_ptr() as isize),
                SMTO_ABORTIFHUNG,
                5000,
                None,
            );
        }
    }
}

impl Drop for UserPath {
    fn drop(&mut self) {
        if !self.key.starts_with(TESTS) {
            return;
        }
        // SAFETY: the strings outlive the calls. The parent goes only if no
        // other test's key is left in it.
        unsafe {
            let _ = RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(self.key.as_str()));
            let _ = RegDeleteKeyW(HKEY_CURRENT_USER, &HSTRING::from(TESTS));
        }
    }
}

fn check(status: WIN32_ERROR) -> io::Result<()> {
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(status.0 as i32))
    }
}

/// The PATH as stored, with its variables unexpanded, and its type. A user
/// with no PATH of their own has an empty one, which gets the usual type.
fn read(key: &str) -> io::Result<(String, REG_VALUE_TYPE)> {
    let key = HSTRING::from(key);
    let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND;
    let mut buffer: Vec<u16> = Vec::new();
    loop {
        let mut kind = REG_VALUE_TYPE::default();
        let mut size = (buffer.len() * 2) as u32;
        // SAFETY: the buffer and its size in bytes are passed together, or no
        // buffer at all to ask the size, and the strings outlive the call.
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                &key,
                w!("Path"),
                flags,
                Some(&mut kind),
                (!buffer.is_empty()).then(|| buffer.as_mut_ptr().cast()),
                Some(&mut size),
            )
        };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok((String::new(), REG_EXPAND_SZ));
        }
        let fits = !buffer.is_empty() && status == ERROR_SUCCESS;
        if !fits && (status == ERROR_MORE_DATA || status == ERROR_SUCCESS) {
            // The value can grow between asking its size and reading it.
            buffer = vec![0; size as usize / 2 + 64];
            continue;
        }
        check(status)?;
        let units = &buffer[..(size as usize / 2).min(buffer.len())];
        let length = units
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(units.len());
        return Ok((String::from_utf16_lossy(&units[..length]), kind));
    }
}

fn write(key: &str, path: &str, kind: REG_VALUE_TYPE) -> io::Result<()> {
    let kind = if kind == REG_SZ {
        REG_SZ
    } else {
        REG_EXPAND_SZ
    };
    let data: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: the strings and data outlive the call, and the size given is the
    // data's, in bytes, ending zero included.
    check(unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(key),
            w!("Path"),
            kind.0,
            Some(data.as_ptr().cast()),
            (data.len() * 2) as u32,
        )
    })
}

/// Fills in `%NAME%` variables from this process's environment, leaving any it
/// doesn't know as they are.
fn expand(entry: &str) -> String {
    let mut expanded = String::new();
    let mut rest = entry;
    while let Some(start) = rest.find('%') {
        expanded.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('%') else {
            rest = &rest[start..];
            break;
        };
        let name = &after[..end];
        match std::env::var(name) {
            Ok(value) if !name.is_empty() => expanded.push_str(&value),
            _ => {
                expanded.push('%');
                expanded.push_str(name);
                expanded.push('%');
            }
        }
        rest = &after[end + 1..];
    }
    expanded.push_str(rest);
    expanded
}

/// Whether a PATH entry names the folder: in any case, with or without quotes,
/// a trailing backslash, or variables.
fn same_folder(entry: &str, folder: &str) -> bool {
    let normal = |text: &str| {
        expand(text.trim().trim_matches('"'))
            .trim_end_matches(['\\', '/'])
            .to_lowercase()
    };
    let entry = normal(entry);
    !entry.is_empty() && entry == normal(folder)
}

/// The PATH with the folder added at the end, or None if it's already there.
fn with_folder(path: &str, folder: &str) -> Option<String> {
    if path.split(';').any(|entry| same_folder(entry, folder)) {
        return None;
    }
    let kept = path.trim_end_matches(';');
    Some(if kept.is_empty() {
        folder.to_string()
    } else {
        format!("{kept};{folder}")
    })
}

/// The PATH without the folder, or None if it isn't there. Every other entry
/// stays as it was written.
fn without_folder(path: &str, folder: &str) -> Option<String> {
    let entries: Vec<&str> = path.split(';').collect();
    let kept: Vec<&str> = entries
        .iter()
        .copied()
        .filter(|entry| !same_folder(entry, folder))
        .collect();
    (kept.len() != entries.len()).then(|| kept.join(";"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FOLDER: &str = r"C:\Users\Pat\AppData\Local\Programs\TraceView";

    #[test]
    fn a_folder_goes_at_the_end() {
        assert_eq!(
            with_folder(r"%USERPROFILE%\bin;C:\Tools;", FOLDER).as_deref(),
            Some(r"%USERPROFILE%\bin;C:\Tools;C:\Users\Pat\AppData\Local\Programs\TraceView")
        );
        assert_eq!(with_folder("", FOLDER).as_deref(), Some(FOLDER));
    }

    #[test]
    fn a_folder_already_there_is_left_alone() {
        let written_differently = [
            r"c:\users\pat\appdata\local\programs\traceview",
            r"C:\Users\Pat\AppData\Local\Programs\TraceView\",
            r#""C:\Users\Pat\AppData\Local\Programs\TraceView""#,
        ];
        for entry in written_differently {
            assert_eq!(with_folder(&format!(r"C:\Tools;{entry}"), FOLDER), None);
        }
    }

    #[test]
    fn variables_are_filled_in_to_compare() {
        let local = std::env::var("LOCALAPPDATA").unwrap();
        let folder = format!(r"{local}\Programs\TraceView");
        assert!(same_folder(r"%LOCALAPPDATA%\Programs\TraceView", &folder));
        assert!(!same_folder(r"%NO_SUCH_VARIABLE_HERE%\TraceView", &folder));
        assert_eq!(expand("100% sure %"), "100% sure %");
    }

    #[test]
    fn removing_keeps_every_other_entry_as_written() {
        let path = format!(r"%USERPROFILE%\bin;{FOLDER};;C:\Tools");
        assert_eq!(
            without_folder(&path, FOLDER).as_deref(),
            Some(r"%USERPROFILE%\bin;;C:\Tools")
        );
        assert_eq!(without_folder(r"C:\Tools", FOLDER), None);
    }

    #[test]
    fn adding_and_removing_round_trips_in_the_registry() {
        let path = UserPath::for_tests("round trip");
        let folder = Path::new(FOLDER);
        assert!(!path.contains(folder));
        write(&path.key, r"%USERPROFILE%\bin", REG_EXPAND_SZ).unwrap();
        assert!(path.add(folder).unwrap());
        assert!(!path.add(folder).unwrap(), "added once only");
        assert!(path.contains(folder));
        let (stored, kind) = read(&path.key).unwrap();
        assert_eq!(stored, format!(r"%USERPROFILE%\bin;{FOLDER}"));
        assert_eq!(kind, REG_EXPAND_SZ, "variables stay unexpanded");
        assert!(path.remove(folder).unwrap());
        assert!(!path.remove(folder).unwrap());
        assert_eq!(read(&path.key).unwrap().0, r"%USERPROFILE%\bin");
    }

    #[test]
    fn a_plain_path_keeps_its_type() {
        let path = UserPath::for_tests("plain");
        write(&path.key, r"C:\Tools", REG_SZ).unwrap();
        path.add(Path::new(FOLDER)).unwrap();
        assert_eq!(read(&path.key).unwrap().1, REG_SZ);
    }

    #[test]
    fn a_user_with_no_path_gets_one() {
        let path = UserPath::for_tests("none");
        assert_eq!(read(&path.key).unwrap(), (String::new(), REG_EXPAND_SZ));
        path.add(Path::new(FOLDER)).unwrap();
        assert_eq!(read(&path.key).unwrap().0, FOLDER);
    }
}
