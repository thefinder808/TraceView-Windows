//! Colors live in one struct so user-defined themes can replace them later.

use eframe::egui::Color32;
use tv_engine::Level;

pub struct Theme {
    pub background: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub header: Color32,
    pub selected: Color32,
    pub warning: Color32,
    pub error: Color32,
    pub critical: Color32,
    pub scrollbar: Color32,
    pub thumb: Color32,
}

impl Theme {
    pub fn console() -> Self {
        Self {
            background: Color32::from_rgb(0x12, 0x14, 0x18),
            text: Color32::from_rgb(0xDC, 0xDF, 0xE4),
            muted: Color32::from_rgb(0x8A, 0x90, 0x9A),
            header: Color32::from_rgb(0x20, 0x23, 0x2A),
            selected: Color32::from_rgb(0x1F, 0x3A, 0x5F),
            warning: Color32::from_rgb(0x4A, 0x3C, 0x10),
            error: Color32::from_rgb(0x55, 0x1E, 0x22),
            critical: Color32::from_rgb(0x7A, 0x14, 0x1C),
            scrollbar: Color32::from_rgb(0x1A, 0x1C, 0x21),
            thumb: Color32::from_rgb(0x55, 0x5B, 0x66),
        }
    }

    /// Mirrors CMTrace's colors, which are what its users are used to scanning.
    pub fn light() -> Self {
        Self {
            background: Color32::WHITE,
            text: Color32::BLACK,
            muted: Color32::from_rgb(0x60, 0x60, 0x60),
            header: Color32::from_rgb(0xEC, 0xEC, 0xEC),
            selected: Color32::from_rgb(0xBF, 0xDB, 0xFF),
            warning: Color32::from_rgb(0xFF, 0xF2, 0x8C),
            error: Color32::from_rgb(0xFF, 0x9C, 0x9C),
            critical: Color32::from_rgb(0xFF, 0x70, 0x70),
            scrollbar: Color32::from_rgb(0xF2, 0xF2, 0xF2),
            thumb: Color32::from_rgb(0xB0, 0xB0, 0xB0),
        }
    }

    pub fn row_background(&self, level: Level) -> Option<Color32> {
        match level {
            Level::Info => None,
            Level::Warning => Some(self.warning),
            Level::Error => Some(self.error),
            Level::Critical => Some(self.critical),
        }
    }
}
