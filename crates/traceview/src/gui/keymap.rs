//! Keyboard shortcuts. Every command has a stable name and can have one
//! shortcut, and settings keep only the ones changed from the defaults, as in
//! `find = "Ctrl+J"`, or `""` for none.
//!
//! A shortcut needs Ctrl or a function key, so typing in a box never runs a
//! command. The keys Windows and the app use for editing, switching tabs and
//! the menus are kept for that.

use super::workspace::command;
use std::collections::BTreeMap;
use tv_ui::{Key, Modifiers, Theme};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shortcut {
    pub key: Key,
    pub ctrl: bool,
    pub shift: bool,
}

/// Keys whose symbols need Shift on many keyboards, so Shift is ignored for them.
fn ignores_shift(key: Key) -> bool {
    matches!(key, Key::Char('=' | '-'))
}

fn key_name(key: Key) -> Option<String> {
    Some(match key {
        Key::Char('=') => "Plus".to_string(),
        Key::Char('-') => "Minus".to_string(),
        Key::Char('\\') => "\\".to_string(),
        Key::Char(character) if character.is_ascii_uppercase() || character.is_ascii_digit() => {
            character.to_string()
        }
        Key::F(number) => format!("F{number}"),
        Key::Tab => "Tab".to_string(),
        _ => return None,
    })
}

fn key_from_name(name: &str) -> Option<Key> {
    let upper = name.to_ascii_uppercase();
    Some(match upper.as_str() {
        "PLUS" | "=" => Key::Char('='),
        "MINUS" | "-" => Key::Char('-'),
        "\\" | "BACKSLASH" => Key::Char('\\'),
        "TAB" => Key::Tab,
        _ if upper.len() == 1 => {
            let character = upper.chars().next()?;
            if !character.is_ascii_alphanumeric() {
                return None;
            }
            Key::Char(character)
        }
        _ => {
            let number: u8 = upper.strip_prefix('F')?.parse().ok()?;
            if !(1..=24).contains(&number) {
                return None;
            }
            Key::F(number)
        }
    })
}

impl Shortcut {
    /// The shortcut for a key press, if the key can be part of one.
    pub fn from_press(key: Key, modifiers: Modifiers) -> Option<Self> {
        if modifiers.alt {
            return None;
        }
        key_name(key)?;
        Some(Self {
            key,
            ctrl: modifiers.ctrl,
            shift: modifiers.shift && !ignores_shift(key),
        })
    }

    /// Reads text such as `Ctrl+Shift+F`, `F3` or `Ctrl+Plus`, in any case.
    /// Empty text is no shortcut.
    pub fn parse(text: &str) -> Result<Option<Self>, String> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(None);
        }
        let parts: Vec<&str> = text.split('+').map(str::trim).collect();
        let (key, modifiers) = parts.split_last().ok_or("no key")?;
        let mut shortcut = Self {
            key: key_from_name(key).ok_or_else(|| format!("\"{key}\" isn't a key name"))?,
            ctrl: false,
            shift: false,
        };
        for modifier in modifiers {
            match modifier.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => shortcut.ctrl = true,
                "shift" => shortcut.shift = !ignores_shift(shortcut.key),
                "alt" => {
                    return Err("Alt opens the menus, so it can't be in a shortcut".to_string())
                }
                _ => return Err(format!("\"{modifier}\" isn't Ctrl or Shift")),
            }
        }
        shortcut.check()?;
        Ok(Some(shortcut))
    }

    pub fn label(&self) -> String {
        let mut label = String::new();
        if self.ctrl {
            label.push_str("Ctrl+");
        }
        if self.shift {
            label.push_str("Shift+");
        }
        label.push_str(&key_name(self.key).unwrap_or_default());
        label
    }

    /// Says why this can't be a shortcut, if it can't.
    pub fn check(&self) -> Result<(), String> {
        let label = self.label();
        match (self.key, self.ctrl, self.shift) {
            (Key::F(10), ..) => Err("F10 opens the menus".to_string()),
            (Key::F(_), ..) => Ok(()),
            (_, false, _) => Err(format!(
                "{label} would get in the way of typing. Use Ctrl or a function key"
            )),
            (Key::Char('A' | 'C' | 'V' | 'X' | 'Y' | 'Z'), true, false) => {
                Err(format!("{label} is kept for editing"))
            }
            (Key::Char('1'..='9'), true, false) => {
                Err(format!("{label} is kept for switching tabs"))
            }
            _ => Ok(()),
        }
    }

    fn matches(&self, key: Key, modifiers: Modifiers) -> bool {
        Self::from_press(key, modifiers).is_some_and(|pressed| pressed == *self)
    }
}

/// Every command a shortcut can run: its id, its name in settings, and its
/// default shortcut.
pub fn commands() -> Vec<(u32, String, &'static str)> {
    let mut list: Vec<(u32, String, &'static str)> = [
        (command::OPEN, "open", "Ctrl+O"),
        (command::MERGE, "merge", "Ctrl+Shift+M"),
        (command::EXPORT, "export", ""),
        (command::CLOSE_TAB, "close_tab", "Ctrl+W"),
        (command::NEXT_TAB, "next_tab", "Ctrl+Tab"),
        (command::PREVIOUS_TAB, "previous_tab", "Ctrl+Shift+Tab"),
        (command::FIND, "find", "Ctrl+F"),
        (command::FIND_NEXT, "find_next", "F3"),
        (command::FIND_PREVIOUS, "find_previous", "Shift+F3"),
        (command::GO_TO_ROW, "go_to_row", "Ctrl+G"),
        (command::NEXT_ERROR, "next_error", "Ctrl+E"),
        (command::PREVIOUS_ERROR, "previous_error", "Ctrl+Shift+E"),
        (command::BOOKMARK, "bookmark", "Ctrl+D"),
        (command::NEXT_BOOKMARK, "next_bookmark", "F2"),
        (command::PREVIOUS_BOOKMARK, "previous_bookmark", "Shift+F2"),
        (command::FILTER, "filter", "Ctrl+Shift+F"),
        (command::CLEAR_FILTER, "clear_filter", ""),
        (command::SAVE_PRESET, "save_preset", ""),
        (command::PALETTE, "palette", "Ctrl+K"),
        (command::SIDEBAR, "sidebar", "Ctrl+B"),
        (command::SPLIT, "split", "Ctrl+\\"),
        (command::SYNC, "sync", "Ctrl+Shift+S"),
        (command::CHIPS, "chips", ""),
        (command::HISTOGRAM, "histogram", ""),
        (command::DETAILS, "details", ""),
        (command::PAUSE, "pause", "Ctrl+U"),
        (command::HIGHLIGHTING, "highlighting", ""),
        (command::COLUMNS, "columns", ""),
        (command::FONTS, "fonts", ""),
        (command::KEYS, "keys", ""),
        (command::LARGER_TEXT, "larger_text", "Ctrl+Plus"),
        (command::SMALLER_TEXT, "smaller_text", "Ctrl+Minus"),
        (command::DEFAULT_TEXT, "default_text", "Ctrl+0"),
        (command::THEME_SYSTEM, "theme_system", ""),
        (command::ERROR_LOOKUP, "error_lookup", "Ctrl+L"),
        (command::RULES, "rules", "Ctrl+H"),
    ]
    .into_iter()
    .map(|(id, name, default)| (id, name.to_string(), default))
    .collect();
    for (id, theme) in (command::THEMES..).zip(Theme::ALL) {
        list.push((id, format!("theme_{}", theme.name.to_lowercase()), ""));
    }
    list
}

/// Which command each shortcut runs.
#[derive(Clone, Debug, Default)]
pub struct Keymap {
    bindings: Vec<(Shortcut, u32)>,
}

impl Keymap {
    /// The defaults, with the changes in `overrides` applied. A changed
    /// shortcut wins over a default that uses the same keys. Names, keys and
    /// repeats that make no sense are skipped.
    pub fn new(overrides: &BTreeMap<String, String>) -> Self {
        let mut chosen: Vec<(Shortcut, u32)> = Vec::new();
        let mut defaults = Vec::new();
        for (id, name, default) in commands() {
            match overrides.get(&name) {
                Some(text) => {
                    if let Ok(Some(shortcut)) = Shortcut::parse(text) {
                        if chosen.iter().all(|(taken, _)| *taken != shortcut) {
                            chosen.push((shortcut, id));
                        }
                    }
                }
                None => {
                    if let Ok(Some(shortcut)) = Shortcut::parse(default) {
                        defaults.push((shortcut, id));
                    }
                }
            }
        }
        for (shortcut, id) in defaults {
            if chosen.iter().all(|(taken, _)| *taken != shortcut) {
                chosen.push((shortcut, id));
            }
        }
        Self { bindings: chosen }
    }

    /// The command a key press runs, if any.
    pub fn command_for(&self, key: Key, modifiers: Modifiers) -> Option<u32> {
        self.bindings
            .iter()
            .find(|(shortcut, _)| shortcut.matches(key, modifiers))
            .map(|(_, id)| *id)
    }

    pub fn shortcut_for(&self, id: u32) -> Option<Shortcut> {
        self.bindings
            .iter()
            .find(|(_, command)| *command == id)
            .map(|(shortcut, _)| *shortcut)
    }

    /// The shortcut as menus show it, or empty for none.
    pub fn label_for(&self, id: u32) -> String {
        self.shortcut_for(id)
            .map(|shortcut| shortcut.label())
            .unwrap_or_default()
    }
}

/// A command's name in settings.
pub fn name_of(id: u32) -> Option<String> {
    commands()
        .into_iter()
        .find(|(command, ..)| *command == id)
        .map(|(_, name, _)| name)
}

pub fn default_of(id: u32) -> Option<Shortcut> {
    commands()
        .into_iter()
        .find(|(command, ..)| *command == id)
        .and_then(|(.., default)| Shortcut::parse(default).ok().flatten())
}

/// Gives a command a shortcut, or none, in `overrides`. Another command using
/// the same keys loses them, and its name comes back so the change can be
/// reported.
pub fn assign(
    overrides: &mut BTreeMap<String, String>,
    id: u32,
    shortcut: Option<Shortcut>,
) -> Option<u32> {
    let name = name_of(id)?;
    let mut taken_from = None;
    if let Some(shortcut) = shortcut {
        let current = Keymap::new(overrides);
        if let Some(other) = current.command_for(shortcut.key, modifiers_of(shortcut)) {
            if other != id {
                if let Some(other_name) = name_of(other) {
                    overrides.insert(other_name, String::new());
                    taken_from = Some(other);
                }
            }
        }
    }
    if shortcut == default_of(id) {
        overrides.remove(&name);
    } else {
        overrides.insert(
            name,
            shortcut
                .map(|shortcut| shortcut.label())
                .unwrap_or_default(),
        );
    }
    taken_from
}

fn modifiers_of(shortcut: Shortcut) -> Modifiers {
    Modifiers {
        ctrl: shortcut.ctrl,
        shift: shortcut.shift,
        alt: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CTRL: Modifiers = Modifiers {
        ctrl: true,
        shift: false,
        alt: false,
    };
    const CTRL_SHIFT: Modifiers = Modifiers {
        ctrl: true,
        shift: true,
        alt: false,
    };

    fn shortcut(text: &str) -> Shortcut {
        Shortcut::parse(text).unwrap().unwrap()
    }

    #[test]
    fn shortcuts_read_and_write_as_text() {
        for text in [
            "Ctrl+O",
            "Ctrl+Shift+F",
            "F3",
            "Shift+F2",
            "Ctrl+\\",
            "Ctrl+Plus",
            "Ctrl+Tab",
            "Ctrl+0",
        ] {
            assert_eq!(shortcut(text).label(), text);
        }
        assert_eq!(shortcut(" ctrl + shift + f ").label(), "Ctrl+Shift+F");
        assert_eq!(
            shortcut("Ctrl+Shift+=").label(),
            "Ctrl+Plus",
            "Shift is part of typing +"
        );
        assert_eq!(Shortcut::parse(""), Ok(None));
    }

    #[test]
    fn keys_kept_for_typing_editing_tabs_and_menus_are_refused() {
        for text in [
            "J",
            "Shift+J",
            "Ctrl+C",
            "Ctrl+V",
            "Ctrl+3",
            "Alt+F",
            "F10",
            "Ctrl+Sparkle",
            "Hyper+J",
        ] {
            assert!(Shortcut::parse(text).is_err(), "{text}");
        }
        assert!(Shortcut::parse("Ctrl+Shift+C").is_ok());
        assert!(Shortcut::parse("Ctrl+Shift+3").is_ok());
        assert!(Shortcut::parse("F1").is_ok());
    }

    #[test]
    fn the_defaults_match_what_the_menus_have_always_said() {
        let keymap = Keymap::new(&BTreeMap::new());
        assert_eq!(
            keymap.command_for(Key::Char('F'), CTRL),
            Some(command::FIND)
        );
        assert_eq!(
            keymap.command_for(Key::Char('F'), CTRL_SHIFT),
            Some(command::FILTER)
        );
        assert_eq!(
            keymap.command_for(Key::Char('='), CTRL_SHIFT),
            Some(command::LARGER_TEXT)
        );
        assert_eq!(keymap.label_for(command::SPLIT), "Ctrl+\\");
        assert_eq!(keymap.label_for(command::EXPORT), "");
        let shortcuts: Vec<Shortcut> = commands()
            .iter()
            .filter_map(|(id, ..)| keymap.shortcut_for(*id))
            .collect();
        for (index, first) in shortcuts.iter().enumerate() {
            assert!(
                !shortcuts[index + 1..].contains(first),
                "{} twice",
                first.label()
            );
        }
    }

    #[test]
    fn changes_win_over_defaults_and_odd_entries_are_skipped() {
        let overrides: BTreeMap<String, String> = [
            ("next_error", "Ctrl+F"),
            ("bookmark", ""),
            ("sparkle", "Ctrl+J"),
            ("open", "Ctrl+C"),
        ]
        .into_iter()
        .map(|(name, text)| (name.to_string(), text.to_string()))
        .collect();
        let keymap = Keymap::new(&overrides);
        assert_eq!(
            keymap.command_for(Key::Char('F'), CTRL),
            Some(command::NEXT_ERROR)
        );
        assert_eq!(
            keymap.shortcut_for(command::FIND),
            None,
            "its default was taken"
        );
        assert_eq!(keymap.shortcut_for(command::BOOKMARK), None);
        assert_eq!(
            keymap.shortcut_for(command::OPEN),
            None,
            "Ctrl+C isn't allowed"
        );
    }

    #[test]
    fn assigning_moves_keys_and_reports_who_lost_them() {
        let mut overrides = BTreeMap::new();
        let lost = assign(&mut overrides, command::EXPORT, Some(shortcut("Ctrl+K")));
        assert_eq!(lost, Some(command::PALETTE));
        let keymap = Keymap::new(&overrides);
        assert_eq!(
            keymap.command_for(Key::Char('K'), CTRL),
            Some(command::EXPORT)
        );
        assert_eq!(keymap.shortcut_for(command::PALETTE), None);
        // Going back to the default keeps settings tidy.
        assign(&mut overrides, command::EXPORT, None);
        assign(&mut overrides, command::PALETTE, Some(shortcut("Ctrl+K")));
        assert_eq!(overrides.get("palette"), None);
        assert_eq!(overrides.get("export"), None);
        assert_eq!(
            Keymap::new(&overrides).label_for(command::PALETTE),
            "Ctrl+K"
        );
    }

    #[test]
    fn every_command_has_its_own_name() {
        let names: Vec<String> = commands().into_iter().map(|(_, name, _)| name).collect();
        for (index, name) in names.iter().enumerate() {
            assert!(!names[index + 1..].contains(name), "{name} twice");
        }
        assert!(names.contains(&"theme_neon".to_string()));
    }
}
