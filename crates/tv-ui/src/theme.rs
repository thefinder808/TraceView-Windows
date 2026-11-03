//! Theme tokens. Every color and metric the UI uses comes from here, so user
//! themes can replace all of them later. Nothing draws with a hard-coded color.

use crate::geometry::Color;

/// The colors a highlight rule can pick, in the order `Theme::rule_colors` holds them.
pub const RULE_COLOR_NAMES: [&str; 6] = ["Red", "Orange", "Yellow", "Green", "Blue", "Purple"];

/// One color a highlight rule can pick: a row background and the text drawn on
/// it, and a stronger shade for markers, which can sit on any row.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RuleColor {
    pub row: Color,
    pub text: Color,
    pub marker: Color,
}

const fn rule(row: u32, text: u32, marker: u32) -> RuleColor {
    RuleColor {
        row: Color::rgb(row),
        text: Color::rgb(text),
        marker: Color::rgb(marker),
    }
}

/// Pale rows with black text, and markers dark enough for CMTrace's red rows.
const LIGHT_RULES: [RuleColor; 6] = [
    rule(0xFFD2CC, 0x000000, 0x9B1C12),
    rule(0xFFDDB8, 0x000000, 0x7A3800),
    rule(0xFFF4A8, 0x000000, 0x5C4600),
    rule(0xD3F0CF, 0x000000, 0x135C26),
    rule(0xD2E4FF, 0x000000, 0x0B4CA3),
    rule(0xE9DAFF, 0x000000, 0x63309F),
];

/// Dim rows with bright text, as dark themes draw errors and warnings.
const DARK_RULES: [RuleColor; 6] = [
    rule(0x47231F, 0xFF9C94, 0xFF6B61),
    rule(0x452E16, 0xFFBE7A, 0xFF9F43),
    rule(0x403A14, 0xFFE57A, 0xFFD60A),
    rule(0x1B3C26, 0x86E39A, 0x3DD65C),
    rule(0x1B3150, 0x96C8FF, 0x5AABFF),
    rule(0x33264F, 0xD2B6FF, 0xB78CFF),
];

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

    pub sidebar_background: Color,
    /// Section headings in the sidebar, such as "OPEN" and "RECENT".
    pub section_text: Color,
    /// Severity chips above the grid. An active chip filters to its level.
    pub chip_background: Color,
    pub chip_active_background: Color,
    pub chip_active_text: Color,
    /// The event histogram: its background, a bar color per level, and the
    /// band showing which part of the log is on screen.
    pub histogram_background: Color,
    pub histogram_info: Color,
    pub histogram_warning: Color,
    pub histogram_error: Color,
    pub histogram_window: Color,
    /// The marker drawn on bookmarked rows.
    pub bookmark: Color,
    /// Highlight rule colors, in [`RULE_COLOR_NAMES`] order.
    pub rule_colors: [RuleColor; 6],

    /// The least height of the grid's header and the status bar. Both grow to
    /// fit a larger font. Font sizes and row spacing are settings of their own.
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
        sidebar_background: Color::rgb(0xF3F3F3),
        section_text: Color::rgb(0x555555),
        chip_background: Color::rgb(0xFFFFFF),
        chip_active_background: Color::rgb(0x0A64C8),
        chip_active_text: Color::rgb(0xFFFFFF),
        histogram_background: Color::rgb(0xF7F7F7),
        histogram_info: Color::rgb(0x8A8A8A),
        histogram_warning: Color::rgb(0x9E6B00),
        histogram_error: Color::rgb(0xD13438),
        histogram_window: Color::rgb(0xDCE8F5),
        bookmark: Color::rgb(0x0A64C8),
        rule_colors: LIGHT_RULES,
        header_height: 22.0,
        status_height: 22.0,
    };

    /// The Mac app's default, a light theme, from its ConsoleTheme.swift. Its
    /// light grays are darkened to meet contrast.
    pub const CONSOLE: Theme = Theme {
        name: "Console",
        is_dark: false,
        window_background: Color::rgb(0xFFFFFF),
        grid_background: Color::rgb(0xFFFFFF),
        header_background: Color::rgb(0xF4F4F4),
        status_background: Color::rgb(0xF4F4F4),
        scrollbar_track: Color::rgb(0xF6F6F6),
        scrollbar_thumb: Color::rgb(0x9B9B9B),
        border: Color::rgb(0xDCDCDC),
        primary_text: Color::rgb(0x161616),
        secondary_text: Color::rgb(0x5A5A5A),
        tertiary_text: Color::rgb(0x767676),
        timestamp_text: Color::rgb(0x4A4A4A),
        component_text: Color::rgb(0x5A5A5A),
        placeholder_text: Color::rgb(0x9B9B9B),
        selected_row: Color::rgb(0xCCE0F5),
        selected_text: Color::rgb(0x161616),
        warning_row: Color::rgb(0xF5F0E6),
        warning_text: Color::rgb(0x7A5400),
        error_row: Color::rgb(0xF9E9E8),
        error_text: Color::rgb(0x9B1D14),
        critical_row: Color::rgb(0xEFDEDE),
        critical_text: Color::rgb(0x9B1D14),
        notice_text: Color::rgb(0x0066CC),
        debug_text: Color::rgb(0x767676),
        accent: Color::rgb(0x0066CC),
        input_background: Color::rgb(0xFFFFFF),
        panel_background: Color::rgb(0xF6F6F6),
        hover_background: Color::rgb(0xECECEC),
        highlight_row: Color::rgb(0xE4D7FF),
        text_selection: Color::rgb(0xA6C9ED),
        sidebar_background: Color::rgb(0xFAFAFA),
        section_text: Color::rgb(0x737373),
        chip_background: Color::rgb(0xFFFFFF),
        chip_active_background: Color::rgb(0xCCE0F5),
        chip_active_text: Color::rgb(0x161616),
        histogram_background: Color::rgb(0xEBEBEB),
        histogram_info: Color::rgb(0x878787),
        histogram_warning: Color::rgb(0x7A5400),
        histogram_error: Color::rgb(0x9B1D14),
        histogram_window: Color::rgb(0xC1D3E5),
        bookmark: Color::rgb(0x0066CC),
        rule_colors: LIGHT_RULES,
        header_height: 22.0,
        status_height: 22.0,
    };

    /// The Mac app's Dark theme, from its DarkTheme.swift. Its dim grays are
    /// lightened to meet contrast.
    pub const DARK: Theme = Theme {
        name: "Dark",
        is_dark: true,
        window_background: Color::rgb(0x1A1A1E),
        grid_background: Color::rgb(0x1E1E22),
        header_background: Color::rgb(0x19191D),
        status_background: Color::rgb(0x19191D),
        scrollbar_track: Color::rgb(0x232328),
        scrollbar_thumb: Color::rgb(0x636366),
        border: Color::rgb(0x3A3A3F),
        primary_text: Color::rgb(0xE5E5EA),
        secondary_text: Color::rgb(0x8E8E93),
        tertiary_text: Color::rgb(0x858587),
        timestamp_text: Color::rgb(0x7EB6FF),
        component_text: Color::rgb(0xA9A9AE),
        placeholder_text: Color::rgb(0x636366),
        selected_row: Color::rgb(0x27384E),
        selected_text: Color::rgb(0xE5E5EA),
        warning_row: Color::rgb(0x33301A),
        warning_text: Color::rgb(0xFFD60A),
        error_row: Color::rgb(0x351E1C),
        error_text: Color::rgb(0xFF6961),
        critical_row: Color::rgb(0x3D1F1D),
        critical_text: Color::rgb(0xFF6961),
        notice_text: Color::rgb(0x7EB6FF),
        debug_text: Color::rgb(0x858587),
        accent: Color::rgb(0x4A9EFF),
        input_background: Color::rgb(0x2C2C31),
        panel_background: Color::rgb(0x232328),
        hover_background: Color::rgb(0x2C2C31),
        highlight_row: Color::rgb(0x2E2850),
        text_selection: Color::rgb(0x375479),
        sidebar_background: Color::rgb(0x232328),
        section_text: Color::rgb(0x8A8A8C),
        chip_background: Color::rgb(0x1E1E22),
        chip_active_background: Color::rgb(0x27384E),
        chip_active_text: Color::rgb(0xE5E5EA),
        histogram_background: Color::rgb(0x2E2E33),
        histogram_info: Color::rgb(0x777779),
        histogram_warning: Color::rgb(0xFFD60A),
        histogram_error: Color::rgb(0xFF6961),
        histogram_window: Color::rgb(0x334258),
        bookmark: Color::rgb(0x4A9EFF),
        rule_colors: DARK_RULES,
        header_height: 22.0,
        status_height: 22.0,
    };

    /// The Mac app's Light theme, from its LightTheme.swift. Its light grays
    /// are darkened to meet contrast.
    pub const LIGHT: Theme = Theme {
        name: "Light",
        is_dark: false,
        window_background: Color::rgb(0xF5F5F7),
        grid_background: Color::rgb(0xFFFFFF),
        header_background: Color::rgb(0xE8E8ED),
        status_background: Color::rgb(0xE8E8ED),
        scrollbar_track: Color::rgb(0xEEEFF1),
        scrollbar_thumb: Color::rgb(0xAEAEB2),
        border: Color::rgb(0xD1D1D6),
        primary_text: Color::rgb(0x1D1D1F),
        secondary_text: Color::rgb(0x68686C),
        tertiary_text: Color::rgb(0x767679),
        timestamp_text: Color::rgb(0x0060CC),
        component_text: Color::rgb(0x6E6E73),
        placeholder_text: Color::rgb(0xAEAEB2),
        selected_row: Color::rgb(0xCCE4FF),
        selected_text: Color::rgb(0x1D1D1F),
        warning_row: Color::rgb(0xFFF3E0),
        warning_text: Color::rgb(0x996300),
        error_row: Color::rgb(0xFFEDEC),
        error_text: Color::rgb(0xB82828),
        critical_row: Color::rgb(0xFFE4E2),
        critical_text: Color::rgb(0xC5221F),
        notice_text: Color::rgb(0x0072EF),
        debug_text: Color::rgb(0x767679),
        accent: Color::rgb(0x007AFF),
        input_background: Color::rgb(0xFFFFFF),
        panel_background: Color::rgb(0xEEEFF1),
        hover_background: Color::rgb(0xE5E5EA),
        highlight_row: Color::rgb(0xE4D7FF),
        text_selection: Color::rgb(0xA6D0FF),
        sidebar_background: Color::rgb(0xEEEFF1),
        section_text: Color::rgb(0x6C6C6F),
        chip_background: Color::rgb(0xFFFFFF),
        chip_active_background: Color::rgb(0xCCE4FF),
        chip_active_text: Color::rgb(0x1D1D1F),
        histogram_background: Color::rgb(0xE5E5EA),
        histogram_info: Color::rgb(0x838386),
        histogram_warning: Color::rgb(0x996300),
        histogram_error: Color::rgb(0xD32F2F),
        histogram_window: Color::rgb(0xBCD2EE),
        bookmark: Color::rgb(0x007AFF),
        rule_colors: LIGHT_RULES,
        header_height: 22.0,
        status_height: 22.0,
    };

    /// The Mac app's Neon theme, from its NeonTheme.swift: cyan on near-black.
    /// Its dim grays are lightened to meet contrast.
    pub const NEON: Theme = Theme {
        name: "Neon",
        is_dark: true,
        window_background: Color::rgb(0x0D0D12),
        grid_background: Color::rgb(0x0F0F16),
        header_background: Color::rgb(0x0A0A10),
        status_background: Color::rgb(0x0A0A10),
        scrollbar_track: Color::rgb(0x12121A),
        scrollbar_thumb: Color::rgb(0x4A4A6A),
        border: Color::rgb(0x2A2A40),
        primary_text: Color::rgb(0xE0E0F0),
        secondary_text: Color::rgb(0x7A7AA1),
        tertiary_text: Color::rgb(0x797991),
        timestamp_text: Color::rgb(0x00D4FF),
        component_text: Color::rgb(0x9898C0),
        placeholder_text: Color::rgb(0x4A4A6A),
        selected_row: Color::rgb(0x0C3645),
        selected_text: Color::rgb(0xE0E0F0),
        warning_row: Color::rgb(0x2C2813),
        warning_text: Color::rgb(0xFFE000),
        error_row: Color::rgb(0x310D1C),
        error_text: Color::rgb(0xFF4070),
        critical_row: Color::rgb(0x3F0C1E),
        critical_text: Color::rgb(0xFF4070),
        notice_text: Color::rgb(0x00D4FF),
        debug_text: Color::rgb(0x797991),
        accent: Color::rgb(0x00D4FF),
        input_background: Color::rgb(0x1A1A2E),
        panel_background: Color::rgb(0x12121A),
        hover_background: Color::rgb(0x1A1A2E),
        highlight_row: Color::rgb(0x2B0B50),
        text_selection: Color::rgb(0x115B77),
        sidebar_background: Color::rgb(0x12121A),
        section_text: Color::rgb(0x7B7B92),
        chip_background: Color::rgb(0x0F0F16),
        chip_active_background: Color::rgb(0x0C3645),
        chip_active_text: Color::rgb(0xE0E0F0),
        histogram_background: Color::rgb(0x1E1E30),
        histogram_info: Color::rgb(0x696979),
        histogram_warning: Color::rgb(0xFFE000),
        histogram_error: Color::rgb(0xFF4070),
        histogram_window: Color::rgb(0x193F55),
        bookmark: Color::rgb(0x00D4FF),
        rule_colors: DARK_RULES,
        header_height: 22.0,
        status_height: 22.0,
    };

    /// Every built-in theme, in the order a theme picker lists them.
    pub const ALL: [&'static Theme; 5] = [
        &Self::CLASSIC,
        &Self::CONSOLE,
        &Self::DARK,
        &Self::LIGHT,
        &Self::NEON,
    ];

    /// The default theme for the system's light or dark setting.
    pub fn for_system(dark: bool) -> &'static Theme {
        if dark {
            &Self::DARK
        } else {
            &Self::CLASSIC
        }
    }

    /// Finds a built-in theme by name, ignoring case, as saved in settings.
    pub fn by_name(name: &str) -> Option<&'static Theme> {
        Self::ALL
            .into_iter()
            .find(|theme| theme.name.eq_ignore_ascii_case(name))
    }

    /// Every pair whose contrast falls short: text needs 4.5:1 against what
    /// it's drawn on, as WCAG asks, and bars and markers need 3:1. Each says
    /// which pair it is and by how much, for a theme file's author.
    pub fn contrast_problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        let mut check = |name: String, first: Color, second: Color, needed: f64| {
            let ratio = contrast(first, second);
            if ratio < needed {
                problems.push(format!("{name}: {ratio:.1} to 1, below {needed}"));
            }
        };
        for (name, text, background) in self.text_pairs() {
            check(format!("{name} text"), text, background, 4.5);
        }
        for (name, mark, background) in [
            ("Info bar", self.histogram_info, self.histogram_background),
            (
                "Warning bar",
                self.histogram_warning,
                self.histogram_background,
            ),
            ("Error bar", self.histogram_error, self.histogram_background),
            ("Bookmark", self.bookmark, self.grid_background),
        ] {
            check(name.to_string(), mark, background, 3.0);
        }
        // A rule's row is read like any other, its marker can land on a
        // severity row, and the rules editor writes each color's name in its
        // marker shade.
        for (name, color) in RULE_COLOR_NAMES.iter().zip(self.rule_colors) {
            for (part, first, second, needed) in [
                ("row text", color.text, color.row, 4.5),
                (
                    "name in the editor",
                    color.marker,
                    self.chip_background,
                    4.5,
                ),
                ("marker", color.marker, self.grid_background, 4.5),
                ("marker on a warning", color.marker, self.warning_row, 3.0),
                ("marker on an error", color.marker, self.error_row, 3.0),
                (
                    "marker on a critical row",
                    color.marker,
                    self.critical_row,
                    3.0,
                ),
            ] {
                check(format!("{name} rule {part}"), first, second, needed);
            }
        }
        problems
    }

    fn text_pairs(&self) -> [(&'static str, Color, Color); 28] {
        [
            ("Primary", self.primary_text, self.grid_background),
            ("Secondary", self.secondary_text, self.grid_background),
            ("Row number", self.tertiary_text, self.grid_background),
            ("Timestamp", self.timestamp_text, self.grid_background),
            ("Component", self.component_text, self.grid_background),
            ("Notice", self.notice_text, self.grid_background),
            ("Debug", self.debug_text, self.grid_background),
            ("Warning", self.warning_text, self.warning_row),
            ("Error", self.error_text, self.error_row),
            ("Critical", self.critical_text, self.critical_row),
            ("Selected", self.selected_text, self.selected_row),
            ("Header", self.primary_text, self.header_background),
            ("Status", self.secondary_text, self.status_background),
            ("Input", self.primary_text, self.input_background),
            ("Panel", self.primary_text, self.panel_background),
            ("Hover", self.primary_text, self.hover_background),
            ("Find match", self.primary_text, self.highlight_row),
            ("Error find match", self.error_text, self.highlight_row),
            ("Error chip", self.histogram_error, self.chip_background),
            ("Warning chip", self.histogram_warning, self.chip_background),
            ("Notice chip", self.notice_text, self.chip_background),
            ("Debug chip", self.debug_text, self.chip_background),
            ("Text selection", self.primary_text, self.text_selection),
            ("Menu label", self.secondary_text, self.panel_background),
            ("Section", self.section_text, self.sidebar_background),
            ("Sidebar", self.primary_text, self.sidebar_background),
            ("Chip", self.primary_text, self.chip_background),
            (
                "Active chip",
                self.chip_active_text,
                self.chip_active_background,
            ),
        ]
    }
}

/// WCAG relative luminance contrast between two colors, from 1 to 21.
pub fn contrast(first: Color, second: Color) -> f64 {
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

/// Names each color field once, for theme files: [`COLOR_TOKENS`] lists the
/// names, and `color` and `set_color` reach a field by its name.
macro_rules! color_tokens {
    ($($field:ident),* $(,)?) => {
        /// Every color a theme file can set, by the name it uses. The highlight
        /// rule colors are set separately, by color name.
        pub const COLOR_TOKENS: &[&str] = &[$(stringify!($field)),*];

        impl Theme {
            pub fn color(&self, token: &str) -> Option<Color> {
                match token {
                    $(stringify!($field) => Some(self.$field),)*
                    _ => None,
                }
            }

            /// Returns false for a name that isn't a color token.
            pub fn set_color(&mut self, token: &str, color: Color) -> bool {
                match token {
                    $(stringify!($field) => {
                        self.$field = color;
                        true
                    })*
                    _ => false,
                }
            }
        }
    };
}

// Every `Color` field of `Theme` belongs here, so theme files can set it.
color_tokens!(
    window_background,
    grid_background,
    header_background,
    status_background,
    scrollbar_track,
    scrollbar_thumb,
    border,
    primary_text,
    secondary_text,
    tertiary_text,
    timestamp_text,
    component_text,
    placeholder_text,
    selected_row,
    selected_text,
    warning_row,
    warning_text,
    error_row,
    error_text,
    critical_row,
    critical_text,
    notice_text,
    debug_text,
    accent,
    input_background,
    panel_background,
    hover_background,
    highlight_row,
    text_selection,
    sidebar_background,
    section_text,
    chip_background,
    chip_active_background,
    chip_active_text,
    histogram_background,
    histogram_info,
    histogram_warning,
    histogram_error,
    histogram_window,
    bookmark,
);

/// A token's name as people read it: `grid_background` is "Grid background".
pub fn token_label(token: &str) -> String {
    let spaced = token.replace('_', " ");
    let mut characters = spaced.chars();
    characters.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(characters).collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_theme_meets_its_contrast_limits() {
        for theme in Theme::ALL {
            assert_eq!(
                theme.contrast_problems(),
                Vec::<String>::new(),
                "{} theme",
                theme.name
            );
        }
    }

    #[test]
    fn a_weak_pair_is_named_with_its_ratio() {
        let mut theme = Theme::CLASSIC;
        theme.primary_text = Color::rgb(0xBBBBBB);
        let problems = theme.contrast_problems();
        assert!(
            problems[0].starts_with("Primary text: 1.9 to 1, below 4.5"),
            "{problems:?}"
        );
    }

    #[test]
    fn every_color_is_a_token() {
        // Each field set to a different color, then read back by its token.
        let mut theme = Theme::CLASSIC;
        for (index, token) in COLOR_TOKENS.iter().enumerate() {
            assert!(
                theme.set_color(token, Color::rgb(index as u32 + 1)),
                "{token}"
            );
        }
        for (index, token) in COLOR_TOKENS.iter().enumerate() {
            assert_eq!(
                theme.color(token),
                Some(Color::rgb(index as u32 + 1)),
                "{token}"
            );
        }
        assert!(!theme.set_color("sparkle", Color::rgb(0)));
        assert_eq!(theme.color("name"), None);
        assert_eq!(COLOR_TOKENS.len(), 40);
        assert_eq!(token_label("grid_background"), "Grid background");
    }

    #[test]
    fn system_setting_picks_the_theme() {
        assert_eq!(Theme::for_system(true).name, "Dark");
        assert_eq!(Theme::for_system(false).name, "Classic");
    }

    #[test]
    fn themes_are_found_by_name_in_any_case() {
        assert_eq!(Theme::ALL[0].name, "Classic");
        for theme in Theme::ALL {
            assert_eq!(Theme::by_name(theme.name), Some(theme));
            assert_eq!(Theme::by_name(&theme.name.to_uppercase()), Some(theme));
        }
        assert_eq!(Theme::by_name("neon").map(|theme| theme.name), Some("Neon"));
        assert_eq!(Theme::by_name("Solarized"), None);
    }
}
