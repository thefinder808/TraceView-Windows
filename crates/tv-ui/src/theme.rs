//! Theme tokens. Every color and metric the UI uses comes from here, so user
//! themes can replace all of them later. Nothing draws with a hard-coded color.

use crate::geometry::Color;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub name: &'static str,
    pub is_dark: bool,

    pub window_background: Color,
    pub grid_background: Color,
    pub header_background: Color,
    pub status_background: Color,
    pub scrollbar_track: Color,
    pub scrollbar_thumb: Color,
    pub border: Color,

    pub primary_text: Color,
    pub secondary_text: Color,
    pub tertiary_text: Color,
    pub timestamp_text: Color,
    pub component_text: Color,
    pub placeholder_text: Color,

    pub selected_row: Color,
    pub selected_text: Color,

    pub warning_row: Color,
    pub warning_text: Color,
    pub error_row: Color,
    pub error_text: Color,
    pub critical_row: Color,
    pub critical_text: Color,
    pub notice_text: Color,
    pub debug_text: Color,

    pub accent: Color,
    pub input_background: Color,
    pub panel_background: Color,
    pub hover_background: Color,
    /// Rows matching the current find text, like CMTrace's highlight.
    pub highlight_row: Color,
    pub text_selection: Color,

    pub ui_font_size: f32,
    pub grid_font_size: f32,
    pub row_height: f32,
    pub header_height: f32,
    pub status_height: f32,
}

impl Theme {
    /// CMTrace's colors: black on white, with red errors and yellow warnings.
    pub const CLASSIC: Theme = Theme {
        name: "Classic",
        is_dark: false,
        window_background: Color::rgb(0xFFFFFF),
        grid_background: Color::rgb(0xFFFFFF),
        header_background: Color::rgb(0xECECEC),
        status_background: Color::rgb(0xECECEC),
        scrollbar_track: Color::rgb(0xF2F2F2),
        scrollbar_thumb: Color::rgb(0xB0B0B0),
        border: Color::rgb(0xC9C9C9),
        primary_text: Color::rgb(0x000000),
        secondary_text: Color::rgb(0x444444),
        tertiary_text: Color::rgb(0x666666),
        timestamp_text: Color::rgb(0x333333),
        component_text: Color::rgb(0x333333),
        placeholder_text: Color::rgb(0x999999),
        selected_row: Color::rgb(0x0A64C8),
        selected_text: Color::rgb(0xFFFFFF),
        warning_row: Color::rgb(0xFFF28C),
        warning_text: Color::rgb(0x000000),
        error_row: Color::rgb(0xFF9C9C),
        error_text: Color::rgb(0x000000),
        critical_row: Color::rgb(0xFF7070),
        critical_text: Color::rgb(0x000000),
        notice_text: Color::rgb(0x0A4FA0),
        debug_text: Color::rgb(0x666666),
        accent: Color::rgb(0x0A64C8),
        input_background: Color::rgb(0xFFFFFF),
        panel_background: Color::rgb(0xF7F7F7),
        hover_background: Color::rgb(0xE5F1FB),
        highlight_row: Color::rgb(0xE4D7FF),
        text_selection: Color::rgb(0x99C9FF),
        ui_font_size: 12.0,
        grid_font_size: 13.0,
        row_height: 18.0,
        header_height: 22.0,
        status_height: 22.0,
    };

    /// The Mac app's dark theme, from its theme specification.
    pub const CONSOLE: Theme = Theme {
        name: "Console",
        is_dark: true,
        window_background: Color::rgb(0x1A1A1E),
        grid_background: Color::rgb(0x1E1E22),
        header_background: Color::rgb(0x232328),
        status_background: Color::rgb(0x19191D),
        scrollbar_track: Color::rgb(0x232328),
        scrollbar_thumb: Color::rgb(0x55555C),
        border: Color::rgb(0x3A3A3F),
        primary_text: Color::rgb(0xE5E5EA),
        secondary_text: Color::rgb(0x8E8E93),
        tertiary_text: Color::rgb(0x8E8E94),
        timestamp_text: Color::rgb(0x7EB6FF),
        component_text: Color::rgb(0xA9A9AE),
        placeholder_text: Color::rgb(0x55555C),
        selected_row: Color::rgb(0x1F3A5F),
        selected_text: Color::rgb(0xFFFFFF),
        warning_row: Color::rgb(0x33301A),
        warning_text: Color::rgb(0xFFD60A),
        error_row: Color::rgb(0x351E1C),
        error_text: Color::rgb(0xFF6961),
        critical_row: Color::rgb(0x3D1F1D),
        critical_text: Color::rgb(0xFF6961),
        notice_text: Color::rgb(0x7EB6FF),
        debug_text: Color::rgb(0x8E8E94),
        accent: Color::rgb(0x4A9EFF),
        input_background: Color::rgb(0x2C2C31),
        panel_background: Color::rgb(0x232328),
        hover_background: Color::rgb(0x34343A),
        highlight_row: Color::rgb(0x2E2850),
        text_selection: Color::rgb(0x264F78),
        ui_font_size: 12.0,
        grid_font_size: 13.0,
        row_height: 18.0,
        header_height: 22.0,
        status_height: 22.0,
    };

    /// The default theme for the system's light or dark setting.
    pub fn for_system(dark: bool) -> &'static Theme {
        if dark {
            &Self::CONSOLE
        } else {
            &Self::CLASSIC
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG relative luminance contrast, which log text has to meet.
    fn contrast(first: Color, second: Color) -> f64 {
        let luminance = |color: Color| {
            let channel = |value: u8| {
                let value = f64::from(value) / 255.0;
                if value <= 0.03928 {
                    value / 12.92
                } else {
                    ((value + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * channel(color.r) + 0.7152 * channel(color.g) + 0.0722 * channel(color.b)
        };
        let (a, b) = (luminance(first), luminance(second));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn log_text_meets_contrast_requirements_in_every_theme() {
        for theme in [Theme::CLASSIC, Theme::CONSOLE] {
            for (name, text, background) in [
                ("primary", theme.primary_text, theme.grid_background),
                ("secondary", theme.secondary_text, theme.grid_background),
                ("row number", theme.tertiary_text, theme.grid_background),
                ("timestamp", theme.timestamp_text, theme.grid_background),
                ("component", theme.component_text, theme.grid_background),
                ("notice", theme.notice_text, theme.grid_background),
                ("debug", theme.debug_text, theme.grid_background),
                ("warning", theme.warning_text, theme.warning_row),
                ("error", theme.error_text, theme.error_row),
                ("critical", theme.critical_text, theme.critical_row),
                ("selected", theme.selected_text, theme.selected_row),
                ("header", theme.primary_text, theme.header_background),
                ("status", theme.secondary_text, theme.status_background),
                ("input", theme.primary_text, theme.input_background),
                ("panel", theme.primary_text, theme.panel_background),
                ("hover", theme.primary_text, theme.hover_background),
                ("find match", theme.primary_text, theme.highlight_row),
                ("text selection", theme.primary_text, theme.text_selection),
                ("menu label", theme.secondary_text, theme.panel_background),
            ] {
                let ratio = contrast(text, background);
                assert!(
                    ratio >= 4.5,
                    "{} theme, {name} text: {ratio:.2} is below 4.5",
                    theme.name
                );
            }
        }
    }

    #[test]
    fn system_setting_picks_the_theme() {
        assert_eq!(Theme::for_system(true).name, "Console");
        assert_eq!(Theme::for_system(false).name, "Classic");
    }
}
