//! Network drives mapped to letters, so a log opened from `Z:\` can be named
//! by the machine it's on.

use windows::core::{HSTRING, PWSTR};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::NetworkManagement::WNet::WNetGetConnectionW;

/// The share a drive letter is mapped to, such as `\\SRV01\c$` for `Z`, or
/// none for a local drive, a letter that isn't mapped, or a mapping that
/// isn't connected. Windows answers from its own list of connections,
/// without reaching the server.
pub fn mapped_drive(letter: u8) -> Option<String> {
    if !letter.is_ascii_alphabetic() {
        return None;
    }
    let device = HSTRING::from(format!("{}:", char::from(letter.to_ascii_uppercase())));
    let mut buffer = vec![0u16; 1024];
    let mut length = buffer.len() as u32;
    // SAFETY: the name outlives the call, and the buffer's length is given in
    // characters, as the call expects.
    let status =
        unsafe { WNetGetConnectionW(&device, Some(PWSTR(buffer.as_mut_ptr())), &mut length) };
    if status != ERROR_SUCCESS {
        return None;
    }
    let end = buffer
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(buffer.len());
    let share = String::from_utf16_lossy(&buffer[..end]);
    (!share.is_empty()).then_some(share)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_drives_and_other_characters_map_to_nothing() {
        let system = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_string());
        assert_eq!(mapped_drive(system.as_bytes()[0]), None);
        assert_eq!(mapped_drive(b'1'), None);
        assert_eq!(mapped_drive(b'\\'), None);
    }
}
