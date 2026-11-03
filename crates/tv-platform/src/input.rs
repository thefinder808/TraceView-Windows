//! Translates Windows input messages into toolkit events.

use tv_ui::{Key, Modifiers};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, VK_ADD, VK_APPS, VK_BACK, VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE,
    VK_F1, VK_F24, VK_HOME, VK_LEFT, VK_MENU, VK_NEXT, VK_OEM_5, VK_OEM_MINUS, VK_OEM_PLUS,
    VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_SUBTRACT, VK_TAB, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    SystemParametersInfoW, SPI_GETWHEELSCROLLLINES, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
};

/// Used when the system setting is "one screen at a time".
const WHEEL_PAGE_ROWS: f32 = 20.0;
const WHEEL_DELTA: f32 = 120.0;

pub(crate) fn modifiers() -> Modifiers {
    // SAFETY: GetKeyState takes a key code and only reads thread input state.
    let down = |key: i32| unsafe { GetKeyState(key) } < 0;
    Modifiers {
        ctrl: down(i32::from(VK_CONTROL.0)),
        shift: down(i32::from(VK_SHIFT.0)),
        alt: down(i32::from(VK_MENU.0)),
    }
}

pub(crate) fn key_from_virtual(code: u16) -> Option<Key> {
    let key = windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(code);
    Some(match key {
        VK_UP => Key::Up,
        VK_DOWN => Key::Down,
        VK_LEFT => Key::Left,
        VK_RIGHT => Key::Right,
        VK_PRIOR => Key::PageUp,
        VK_NEXT => Key::PageDown,
        VK_HOME => Key::Home,
        VK_END => Key::End,
        VK_ESCAPE => Key::Escape,
        VK_RETURN => Key::Enter,
        VK_TAB => Key::Tab,
        VK_BACK => Key::Backspace,
        VK_DELETE => Key::Delete,
        VK_APPS => Key::Menu,
        // The backslash key on US keyboards, for Ctrl+\.
        VK_OEM_5 => Key::Char('\\'),
        // Plus and minus, for Ctrl+Plus and Ctrl+Minus, from either keyboard.
        VK_OEM_PLUS | VK_ADD => Key::Char('='),
        VK_OEM_MINUS | VK_SUBTRACT => Key::Char('-'),
        _ if (VK_F1.0..=VK_F24.0).contains(&code) => Key::F((code - VK_F1.0 + 1) as u8),
        // Virtual codes for letters and digits equal their uppercase ASCII values.
        // Codes in the lowercase range are other keys, such as the number pad's.
        _ if code.to_le_bytes()[1] == 0
            && ((code as u8).is_ascii_uppercase() || (code as u8).is_ascii_digit()) =>
        {
            Key::Char(code as u8 as char)
        }
        _ => return None,
    })
}

/// How many rows one wheel notch scrolls, from the user's system setting.
fn rows_per_notch() -> f32 {
    let mut lines = 3u32;
    // SAFETY: the out parameter is a local of the size this setting writes.
    let read = unsafe {
        SystemParametersInfoW(
            SPI_GETWHEELSCROLLLINES,
            0,
            Some(std::ptr::from_mut(&mut lines).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    match read {
        Ok(()) if lines == u32::MAX => WHEEL_PAGE_ROWS,
        Ok(()) => lines as f32,
        Err(_) => 3.0,
    }
}

/// Converts a raw wheel delta to rows, positive toward the start of the file.
pub(crate) fn wheel_rows(delta: i16) -> f32 {
    f32::from(delta) / WHEEL_DELTA * rows_per_notch()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_navigation_and_function_keys() {
        assert_eq!(key_from_virtual(VK_UP.0), Some(Key::Up));
        assert_eq!(key_from_virtual(VK_NEXT.0), Some(Key::PageDown));
        assert_eq!(key_from_virtual(VK_F1.0), Some(Key::F(1)));
        assert_eq!(key_from_virtual(VK_F1.0 + 2), Some(Key::F(3)));
    }

    #[test]
    fn maps_letters_and_digits_to_uppercase() {
        assert_eq!(key_from_virtual(u16::from(b'O')), Some(Key::Char('O')));
        assert_eq!(key_from_virtual(u16::from(b'7')), Some(Key::Char('7')));
    }

    #[test]
    fn ignores_keys_it_does_not_use() {
        assert_eq!(key_from_virtual(VK_SHIFT.0), None);
        assert_eq!(key_from_virtual(0x1234), None);
        // Number pad 1, whose code is a lowercase letter's.
        assert_eq!(key_from_virtual(0x61), None);
    }

    #[test]
    fn plus_and_minus_come_from_either_keyboard() {
        assert_eq!(key_from_virtual(VK_OEM_PLUS.0), Some(Key::Char('=')));
        assert_eq!(key_from_virtual(VK_ADD.0), Some(Key::Char('=')));
        assert_eq!(key_from_virtual(VK_OEM_MINUS.0), Some(Key::Char('-')));
        assert_eq!(key_from_virtual(VK_SUBTRACT.0), Some(Key::Char('-')));
    }

    #[test]
    fn wheel_direction_is_preserved() {
        assert!(wheel_rows(120) > 0.0);
        assert!(wheel_rows(-120) < 0.0);
    }
}
