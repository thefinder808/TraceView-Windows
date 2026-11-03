//! Themes in files: one TOML file each, in a `themes` folder beside the
//! settings file. A file can start from a built-in theme and change only some
//! colors, so a small file is easy to share:
//!
//! ```toml
//! name = "Midnight"
//! based_on = "Dark"
//!
//! [colors]
//! grid_background = "#0B1020"
//!
//! [rule_colors.blue]
//! row = "#1B3150"
//! ```
//!
//! The renderer and every view take themes as `&'static`, so a theme read
//! from a file is kept for as long as the app runs. Each distinct theme is
//! kept once, so editing one costs a few hundred bytes per change.

use super::settings::write_atomically;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use tv_ui::theme::{RuleColor, COLOR_TOKENS, RULE_COLOR_NAMES};
use tv_ui::{Color, Theme};

pub const FOLDER: &str = "themes";
/// Limits, so a folder of odd files can't make the menus unusable or take long to read.
const MOST_FILES: usize = 50;
const MOST_FILE_BYTES: u64 = 64 * 1024;
const LEAST_BAR: f32 = 16.0;
const MOST_BAR: f32 = 48.0;
const HEADER: &str = "# A TraceView theme. Any color left out comes from based_on, or from\n# Classic or Dark. Colors are #RRGGBB.\n\n";

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ThemeFile {
    name: String,
    /// A built-in theme to start from.
    #[serde(skip_serializing_if = "String::is_empty")]
    based_on: String,
    dark: Option<bool>,
    header_height: Option<f32>,
    status_height: Option<f32>,
    colors: BTreeMap<String, String>,
    /// By rule color name, such as `blue`.
    rule_colors: BTreeMap<String, RuleFile>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RuleFile {
    #[serde(skip_serializing_if = "String::is_empty")]
    row: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    text: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    marker: String,
}

/// A theme read from a file, and the file.
#[derive(Clone, Debug)]
pub struct FileTheme {
    pub theme: &'static Theme,
    pub path: PathBuf,
}

/// Reads `#RRGGBB`, with or without the `#`.
pub fn parse_color(text: &str) -> Option<Color> {
    let digits = text.trim().trim_start_matches('#');
    if digits.len() != 6
        || !digits
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return None;
    }
    u32::from_str_radix(digits, 16).ok().map(Color::rgb)
}

pub fn color_text(color: Color) -> String {
    format!("#{:02X}{:02X}{:02X}", color.r, color.g, color.b)
}

fn color_or_error(token: &str, text: &str) -> Result<Color, String> {
    parse_color(text).ok_or_else(|| format!("{token} is \"{text}\", which isn't #RRGGBB"))
}

/// Reads a theme file. Its name comes back apart from the theme, because a
/// theme's name has to be kept for good, which [`keep`] does.
pub fn parse(text: &str) -> Result<(String, Theme), String> {
    let file: ThemeFile = toml::from_str(text).map_err(|error| {
        let line = error.span().map_or(0, |span| {
            text[..span.start.min(text.len())].lines().count().max(1)
        });
        format!("line {line}: {}", error.message().trim())
    })?;
    let name = file.name.trim().to_string();
    if name.is_empty() {
        return Err("it needs a name".to_string());
    }
    if Theme::by_name(&name).is_some() {
        return Err(format!("{name} is a built-in theme's name"));
    }
    let mut theme = match file.based_on.trim() {
        "" if file.dark == Some(true) => Theme::DARK,
        "" => Theme::CLASSIC,
        base => *Theme::by_name(base)
            .ok_or_else(|| format!("based_on is \"{base}\", which isn't a built-in theme"))?,
    };
    if let Some(dark) = file.dark {
        theme.is_dark = dark;
    }
    for (token, value) in &file.colors {
        let color = color_or_error(token, value)?;
        if !theme.set_color(token, color) {
            return Err(format!("\"{token}\" isn't a color name"));
        }
    }
    for (name, parts) in &file.rule_colors {
        let index = RULE_COLOR_NAMES
            .iter()
            .position(|known| known.eq_ignore_ascii_case(name))
            .ok_or_else(|| format!("\"{name}\" isn't a rule color"))?;
        let rule = &mut theme.rule_colors[index];
        for (part, value, slot) in [
            ("row", &parts.row, &mut rule.row),
            ("text", &parts.text, &mut rule.text),
            ("marker", &parts.marker, &mut rule.marker),
        ] {
            if !value.trim().is_empty() {
                *slot = color_or_error(&format!("{name} {part}"), value)?;
            }
        }
    }
    let bar = |height: Option<f32>, default: f32| match height {
        Some(height) if height.is_finite() => height.clamp(LEAST_BAR, MOST_BAR),
        _ => default,
    };
    theme.header_height = bar(file.header_height, theme.header_height);
    theme.status_height = bar(file.status_height, theme.status_height);
    Ok((name, theme))
}

/// Writes every part of a theme, so its file stands on its own.
pub fn render(name: &str, theme: &Theme) -> String {
    let file = ThemeFile {
        name: name.to_string(),
        based_on: String::new(),
        dark: Some(theme.is_dark),
        header_height: Some(theme.header_height),
        status_height: Some(theme.status_height),
        colors: COLOR_TOKENS
            .iter()
            .filter_map(|token| Some(((*token).to_string(), color_text(theme.color(token)?))))
            .collect(),
        rule_colors: RULE_COLOR_NAMES
            .iter()
            .zip(theme.rule_colors)
            .map(|(name, RuleColor { row, text, marker })| {
                (
                    name.to_lowercase(),
                    RuleFile {
                        row: color_text(row),
                        text: color_text(text),
                        marker: color_text(marker),
                    },
                )
            })
            .collect(),
    };
    format!(
        "{HEADER}{}",
        toml::to_string_pretty(&file).unwrap_or_default()
    )
}

static KEPT: Mutex<Vec<&'static Theme>> = Mutex::new(Vec::new());

/// Keeps a theme for as long as the app runs, under `name`. Asking again for
/// the same theme gives back the one already kept.
pub fn keep(name: &str, theme: Theme) -> &'static Theme {
    let mut kept = KEPT.lock().unwrap_or_else(PoisonError::into_inner);
    let same = |existing: &&'static Theme| {
        let mut probe = theme;
        probe.name = existing.name;
        existing.name == name && probe == **existing
    };
    if let Some(existing) = kept.iter().find(|existing| same(existing)) {
        return existing;
    }
    let mut owned = theme;
    owned.name = Box::leak(name.to_string().into_boxed_str());
    let leaked: &'static Theme = Box::leak(Box::new(owned));
    kept.push(leaked);
    leaked
}

/// A file name for a theme, from the characters of its name that are safe in one.
pub fn file_name(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || matches!(character, ' ' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect();
    let safe = safe.trim();
    format!("{}.toml", if safe.is_empty() { "Theme" } else { safe })
}

/// Whether a theme can't be called `name`, because a built-in theme is, or
/// one of `taken` is or would have the same file.
pub fn name_taken(name: &str, taken: &[String]) -> bool {
    let file = file_name(name);
    Theme::by_name(name).is_some()
        || taken.iter().any(|taken| {
            taken.eq_ignore_ascii_case(name) || file_name(taken).eq_ignore_ascii_case(&file)
        })
}

/// What a theme being edited can't be renamed to: the other themes' names,
/// and the names of every theme file in the folder, even ones with mistakes,
/// so none is written over. `own` is the edited theme's file.
pub fn taken_names(themes: &[FileTheme], folder: Option<&Path>, own: Option<&Path>) -> Vec<String> {
    let others = |path: &Path| Some(path) != own;
    let mut taken: Vec<String> = themes
        .iter()
        .filter(|file| others(&file.path))
        .map(|file| file.theme.name.to_string())
        .collect();
    if let Some(entries) = folder.and_then(|folder| std::fs::read_dir(folder).ok()) {
        taken.extend(
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| others(path))
                .filter(|path| {
                    path.extension()
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("toml"))
                })
                .filter_map(|path| Some(path.file_stem()?.to_string_lossy().into_owned())),
        );
    }
    taken
}

/// Reads every theme in a folder, in file name order. A file with a mistake,
/// or a name another file already took, is left out, and says why.
pub fn load(folder: &Path) -> (Vec<FileTheme>, Vec<String>) {
    let (mut themes, mut problems) = (Vec::<FileTheme>::new(), Vec::new());
    let Ok(entries) = std::fs::read_dir(folder) else {
        return (themes, problems);
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("toml"))
        })
        .collect();
    paths.sort();
    for path in paths.into_iter().take(MOST_FILES) {
        let label = path
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        let read = std::fs::metadata(&path)
            .ok()
            .filter(|metadata| metadata.len() <= MOST_FILE_BYTES)
            .ok_or_else(|| "it's too large for a theme".to_string())
            .and_then(|_| std::fs::read_to_string(&path).map_err(|error| error.to_string()))
            .and_then(|text| parse(&text));
        match read {
            Ok((name, _))
                if themes
                    .iter()
                    .any(|known| known.theme.name.eq_ignore_ascii_case(&name)) =>
            {
                problems.push(format!(
                    "Theme file {label} wasn't used: another file is already called {name}"
                ));
            }
            Ok((name, theme)) => themes.push(FileTheme {
                theme: keep(&name, theme),
                path,
            }),
            Err(problem) => problems.push(format!("Theme file {label} wasn't used: {problem}")),
        }
    }
    (themes, problems)
}

pub fn save(path: &Path, theme: &Theme) -> io::Result<()> {
    write_atomically(path, &render(theme.name, theme))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_that_share_a_file_are_taken() {
        let taken = ["Mine".to_string(), "a_b".to_string()];
        assert!(name_taken("mine", &taken));
        assert!(name_taken("a/b", &taken), "a/b would be saved as a_b.toml");
        assert!(name_taken("dark", &taken), "built-in names are taken");
        assert!(!name_taken("Paper", &taken));
    }

    #[test]
    fn a_small_file_changes_only_what_it_names() {
        let (name, theme) = parse(
            "name = \"Midnight\"\nbased_on = \"dark\"\n[colors]\ngrid_background = \"#0B1020\"\n[rule_colors.Blue]\nrow = \"112233\"\n",
        )
        .unwrap();
        assert_eq!(name, "Midnight");
        assert_eq!(theme.grid_background, Color::rgb(0x0B1020));
        assert_eq!(theme.rule_colors[4].row, Color::rgb(0x112233));
        assert_eq!(theme.rule_colors[4].text, Theme::DARK.rule_colors[4].text);
        assert_eq!(theme.primary_text, Theme::DARK.primary_text);
        assert!(theme.is_dark);
        let (_, light) = parse("name = \"Plain\"\n").unwrap();
        assert_eq!(light.grid_background, Theme::CLASSIC.grid_background);
        let (_, dark) = parse("name = \"Night\"\ndark = true\n").unwrap();
        assert_eq!(dark.grid_background, Theme::DARK.grid_background);
    }

    #[test]
    fn mistakes_say_what_and_where() {
        for (text, expected) in [
            ("name = \"\"", "it needs a name"),
            ("name = \"Dark\"", "Dark is a built-in theme's name"),
            (
                "name = \"A\"\nbased_on = \"Solarized\"",
                "based_on is \"Solarized\"",
            ),
            (
                "name = \"A\"\n[colors]\ngrid_backgroud = \"#000000\"",
                "\"grid_backgroud\" isn't a color name",
            ),
            (
                "name = \"A\"\n[colors]\nborder = \"blue\"",
                "border is \"blue\", which isn't #RRGGBB",
            ),
            (
                "name = \"A\"\n[rule_colors.teal]\nrow = \"#000000\"",
                "\"teal\" isn't a rule color",
            ),
            ("name = \"A\"\nsparkle = 1", "unknown field `sparkle`"),
            ("name = \"A\"\n[colors\n", "line 2"),
        ] {
            let problem = parse(text).unwrap_err();
            assert!(problem.contains(expected), "{text}: {problem}");
        }
    }

    #[test]
    fn a_saved_theme_reads_back_the_same() {
        let mut theme = Theme::NEON;
        theme.border = Color::rgb(0x123456);
        theme.rule_colors[2].marker = Color::rgb(0xABCDEF);
        let text = render("Neon copy", &theme);
        assert!(text.starts_with("# A TraceView theme."));
        let (name, read) = parse(&text).unwrap();
        assert_eq!(name, "Neon copy");
        let mut expected = theme;
        expected.name = read.name;
        assert_eq!(read, expected);
    }

    #[test]
    fn kept_themes_are_shared_when_they_match() {
        let first = keep("Kept", Theme::LIGHT);
        let again = keep("Kept", Theme::LIGHT);
        assert!(std::ptr::eq(first, again));
        assert_eq!(first.name, "Kept");
        let mut changed = Theme::LIGHT;
        changed.border = Color::rgb(0x010203);
        assert!(!std::ptr::eq(first, keep("Kept", changed)));
        assert!(!std::ptr::eq(first, keep("Other", Theme::LIGHT)));
    }

    #[test]
    fn a_folder_loads_in_order_and_skips_what_it_cant_use() {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, text: &str| std::fs::write(dir.path().join(name), text).unwrap();
        write("b.toml", "name = \"Beta\"\n");
        write("a.toml", "name = \"Alpha\"\n");
        write("c.toml", "name = \"alpha\"\n");
        write("d.toml", "name = \"Broken\"\n[colors]\nborder = \"no\"\n");
        write("notes.txt", "not a theme");
        let (themes, problems) = load(dir.path());
        let names: Vec<&str> = themes.iter().map(|file| file.theme.name).collect();
        assert_eq!(names, ["Alpha", "Beta"]);
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems[0].contains("c.toml") && problems[0].contains("already called alpha"));
        assert!(problems[1].contains("d.toml"));
        assert_eq!(load(&dir.path().join("missing")).0.len(), 0);
    }

    #[test]
    fn file_names_are_safe() {
        assert_eq!(file_name("My Dark"), "My Dark.toml");
        assert_eq!(file_name("a/b:c"), "a_b_c.toml");
        assert_eq!(file_name("  "), "Theme.toml");
        assert_eq!(parse_color("#0a0B0c"), Some(Color::rgb(0x0A0B0C)));
        assert_eq!(parse_color("#12345"), None);
        assert_eq!(color_text(Color::rgb(0x0A0B0C)), "#0A0B0C");
    }
}
