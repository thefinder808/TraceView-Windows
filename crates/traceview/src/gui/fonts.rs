//! Fonts and row spacing: one font for log text and one for the rest of the
//! window, each with its own size, and how much room each row gets.
//!
//! Rows are sized from the font's real line height, so they always fit their
//! text. The window's text stays within sizes its bars have room for. Windows'
//! display scale makes everything larger, if that's what's wanted.

use super::settings::FontSettings;
use std::ops::RangeInclusive;
use tv_render::Fonts;

pub const LOG_SIZES: RangeInclusive<f32> = 8.0..=28.0;
pub const WINDOW_SIZES: RangeInclusive<f32> = 10.0..=15.0;

/// Which font a choice is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Log,
    Window,
}

impl Target {
    pub fn label(self) -> &'static str {
        match self {
            Target::Log => "Log font",
            Target::Window => "Window font",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spacing {
    Compact,
    Normal,
    Roomy,
}

impl Spacing {
    const ALL: [Spacing; 3] = [Spacing::Compact, Spacing::Normal, Spacing::Roomy];

    pub fn name(self) -> &'static str {
        match self {
            Spacing::Compact => "compact",
            Spacing::Normal => "normal",
            Spacing::Roomy => "roomy",
        }
    }

    /// Anything else, perhaps from a newer version, is normal.
    pub fn from_name(name: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|spacing| spacing.name().eq_ignore_ascii_case(name.trim()))
            .unwrap_or(Spacing::Normal)
    }

    /// Room added to a line of text to make a row.
    pub fn padding(self) -> f32 {
        match self {
            Spacing::Compact => 1.0,
            Spacing::Normal => 3.0,
            Spacing::Roomy => 8.0,
        }
    }

    /// The next spacing up or down, staying at the ends.
    pub fn step(self, roomier: bool) -> Self {
        let index = Self::ALL
            .iter()
            .position(|spacing| *spacing == self)
            .unwrap_or(1);
        let next = if roomier {
            (index + 1).min(Self::ALL.len() - 1)
        } else {
            index.saturating_sub(1)
        };
        Self::ALL[next]
    }
}

fn clamp(size: f32, sizes: &RangeInclusive<f32>, default: f32) -> f32 {
    if size.is_finite() {
        size.round().clamp(*sizes.start(), *sizes.end())
    } else {
        default
    }
}

/// A size one step larger or smaller, within `sizes`.
pub fn step_size(size: f32, sizes: &RangeInclusive<f32>, larger: bool) -> f32 {
    let step = if larger { 1.0 } else { -1.0 };
    (size.round() + step).clamp(*sizes.start(), *sizes.end())
}

/// What the renderer draws with. Sizes out of range are brought into it, and
/// an empty name means the default font.
pub fn render_fonts(settings: &FontSettings) -> Fonts {
    let defaults = Fonts::default();
    let family = |name: &str, default: String| {
        let name = name.trim();
        if name.is_empty() {
            default
        } else {
            name.to_string()
        }
    };
    Fonts {
        ui_family: family(&settings.window_font, defaults.ui_family),
        ui_size: clamp(settings.window_size, &WINDOW_SIZES, defaults.ui_size),
        grid_family: family(&settings.log_font, defaults.grid_family),
        grid_size: clamp(settings.log_size, &LOG_SIZES, defaults.grid_size),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_match_the_renderer() {
        assert_eq!(render_fonts(&FontSettings::default()), Fonts::default());
    }

    #[test]
    fn odd_values_become_usable_ones() {
        let settings = FontSettings {
            log_font: "  ".to_string(),
            log_size: 400.0,
            window_font: " Cascadia Mono ".to_string(),
            window_size: f32::NAN,
            row_spacing: "airy".to_string(),
        };
        let fonts = render_fonts(&settings);
        assert_eq!(fonts.grid_family, "Consolas");
        assert_eq!(fonts.grid_size, *LOG_SIZES.end());
        assert_eq!(fonts.ui_family, "Cascadia Mono");
        assert_eq!(fonts.ui_size, 12.0);
        assert_eq!(Spacing::from_name(&settings.row_spacing), Spacing::Normal);
    }

    #[test]
    fn sizes_and_spacing_step_and_stop_at_the_ends() {
        assert_eq!(step_size(13.0, &LOG_SIZES, true), 14.0);
        assert_eq!(step_size(8.0, &LOG_SIZES, false), 8.0);
        assert_eq!(step_size(15.0, &WINDOW_SIZES, true), 15.0);
        assert_eq!(Spacing::Normal.step(true), Spacing::Roomy);
        assert_eq!(Spacing::Roomy.step(true), Spacing::Roomy);
        assert_eq!(Spacing::Compact.step(false), Spacing::Compact);
    }
}
