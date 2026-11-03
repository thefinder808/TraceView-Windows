//! The grid's columns: which show, in what order, and how wide, kept for each
//! log format, since SCCM logs have threads and source files and most others
//! don't. The row number always comes first and the message always shows.
//!
//! Widths are in characters of the grid font, so they keep fitting their text
//! whatever the font size.

use super::row_cache::RowCells;
use super::settings::ColumnSetting;
use tv_engine::{Format, LogFile};

pub const MIN_WIDTH: f32 = 3.0;
pub const MAX_WIDTH: f32 = 400.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Column {
    Time,
    Level,
    Component,
    Thread,
    /// The source file that wrote the row, as SCCM logs name it.
    Source,
    Message,
}

impl Column {
    /// Every column, in the default order.
    pub const ALL: [Column; 6] = [
        Column::Time,
        Column::Level,
        Column::Component,
        Column::Thread,
        Column::Source,
        Column::Message,
    ];

    /// The name it's saved under.
    pub fn name(self) -> &'static str {
        match self {
            Column::Time => "time",
            Column::Level => "level",
            Column::Component => "component",
            Column::Thread => "thread",
            Column::Source => "source",
            Column::Message => "message",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|column| column.name().eq_ignore_ascii_case(name.trim()))
    }

    /// The header's title, short where the column is narrow.
    pub fn title(self) -> &'static str {
        match self {
            Column::Level => "Lvl",
            _ => self.label(),
        }
    }

    /// The full name, for the editor and screen readers.
    pub fn label(self) -> &'static str {
        match self {
            Column::Time => "Time",
            Column::Level => "Level",
            Column::Component => "Component",
            Column::Thread => "Thread",
            Column::Source => "Source",
            Column::Message => "Message",
        }
    }

    fn default_width(self) -> f32 {
        match self {
            Column::Time => 23.0,
            Column::Level => 4.0,
            Column::Component => 22.0,
            Column::Thread => 6.0,
            Column::Source => 18.0,
            Column::Message => 80.0,
        }
    }

    /// The cell's text. The level comes from the index rather than the row's
    /// text, so it has none here.
    pub fn text(self, cells: &RowCells) -> &str {
        match self {
            Column::Time => &cells.time,
            Column::Component => &cells.component,
            Column::Thread => &cells.thread,
            Column::Source => &cells.source,
            Column::Message => &cells.message,
            Column::Level => "",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColumnSpec {
    pub column: Column,
    /// In characters.
    pub width: f32,
    pub shown: bool,
}

/// What decides a log's columns: its format, and for a custom format, its
/// name and the parts its pattern names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogKind {
    pub format: Format,
    /// A custom format's name, or empty.
    pub name: String,
    /// The columns a custom format's pattern fills.
    pub parts: Vec<Column>,
}

impl LogKind {
    pub fn builtin(format: Format) -> Self {
        Self {
            format,
            name: String::new(),
            parts: Vec::new(),
        }
    }

    pub fn of(log: &LogFile) -> Self {
        match log.custom_format() {
            Some(custom) => Self {
                format: Format::Custom,
                name: custom.name().to_string(),
                parts: Column::ALL
                    .into_iter()
                    .filter(|column| custom.has_part(column.name()))
                    .collect(),
            },
            None => Self::builtin(log.format()),
        }
    }

    /// The settings key for its columns.
    pub fn key(&self) -> String {
        match self.format {
            Format::Plain => "plain".to_string(),
            Format::Sccm => "sccm".to_string(),
            Format::Custom => format!("custom:{}", self.name.to_lowercase()),
        }
    }

    /// What its logs are called, as in "Columns for SCCM logs".
    pub fn called(&self) -> String {
        match self.format {
            Format::Sccm => "SCCM logs".to_string(),
            Format::Plain => "other logs".to_string(),
            Format::Custom => format!("{} logs", self.name),
        }
    }

    /// Every column in its default order. A custom format shows the parts it
    /// names, with the level and message always.
    pub fn defaults(&self) -> Vec<ColumnSpec> {
        if self.format != Format::Custom {
            return defaults(self.format);
        }
        Column::ALL
            .into_iter()
            .map(|column| ColumnSpec {
                column,
                width: column.default_width(),
                shown: matches!(column, Column::Level | Column::Message)
                    || self.parts.contains(&column),
            })
            .collect()
    }
}

/// Every column in its default order for a built-in format. SCCM logs show
/// the thread, as CMTrace does, and other logs rarely have one.
pub fn defaults(format: Format) -> Vec<ColumnSpec> {
    Column::ALL
        .into_iter()
        .map(|column| ColumnSpec {
            column,
            width: column.default_width(),
            shown: match column {
                Column::Thread => format == Format::Sccm,
                Column::Source => false,
                _ => true,
            },
        })
        .collect()
}

fn clamp_width(width: f32, column: Column) -> f32 {
    if width.is_finite() {
        width.clamp(MIN_WIDTH, MAX_WIDTH)
    } else {
        column.default_width()
    }
}

/// Reads saved columns. Each column comes once, a name this version doesn't
/// know is skipped, any column the file leaves out goes at the end as it
/// would by default, and the message always shows.
pub fn from_settings(saved: Option<&[ColumnSetting]>, kind: &LogKind) -> Vec<ColumnSpec> {
    let mut specs: Vec<ColumnSpec> = Vec::new();
    for setting in saved.unwrap_or_default() {
        let Some(column) = Column::from_name(&setting.name) else {
            continue;
        };
        if specs.iter().all(|spec| spec.column != column) {
            specs.push(ColumnSpec {
                column,
                width: clamp_width(setting.width, column),
                shown: setting.shown || column == Column::Message,
            });
        }
    }
    for default in kind.defaults() {
        if specs.iter().all(|spec| spec.column != default.column) {
            specs.push(default);
        }
    }
    specs
}

pub fn to_settings(specs: &[ColumnSpec]) -> Vec<ColumnSetting> {
    specs
        .iter()
        .map(|spec| ColumnSetting {
            name: spec.column.name().to_string(),
            width: (spec.width * 10.0).round() / 10.0,
            shown: spec.shown,
        })
        .collect()
}

/// The shown columns, in order.
#[cfg(test)]
pub fn shown(specs: &[ColumnSpec]) -> impl Iterator<Item = Column> + '_ {
    specs
        .iter()
        .filter(|spec| spec.shown)
        .map(|spec| spec.column)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setting(name: &str, width: f32, shown: bool) -> ColumnSetting {
        ColumnSetting {
            name: name.to_string(),
            width,
            shown,
        }
    }

    #[test]
    fn sccm_shows_the_thread_and_other_logs_dont() {
        let sccm: Vec<Column> = shown(&defaults(Format::Sccm)).collect();
        assert_eq!(
            sccm,
            [
                Column::Time,
                Column::Level,
                Column::Component,
                Column::Thread,
                Column::Message
            ]
        );
        let plain: Vec<Column> = shown(&defaults(Format::Plain)).collect();
        assert_eq!(
            plain,
            [
                Column::Time,
                Column::Level,
                Column::Component,
                Column::Message
            ]
        );
    }

    #[test]
    fn saved_columns_round_trip() {
        let mut specs = defaults(Format::Sccm);
        specs.swap(0, 5);
        specs[1].width = 9.0;
        specs[4].shown = true;
        let saved = to_settings(&specs);
        assert_eq!(
            from_settings(Some(&saved), &LogKind::builtin(Format::Sccm)),
            specs
        );
    }

    #[test]
    fn odd_files_still_give_every_column_once() {
        let saved = [
            setting("Message", 50.0, false),
            setting("thread", f32::NAN, true),
            setting("thread", 9.0, false),
            setting("sparkle", 10.0, true),
            setting("time", 100_000.0, true),
        ];
        let specs = from_settings(Some(&saved), &LogKind::builtin(Format::Plain));
        let order: Vec<Column> = specs.iter().map(|spec| spec.column).collect();
        assert_eq!(
            order,
            [
                Column::Message,
                Column::Thread,
                Column::Time,
                Column::Level,
                Column::Component,
                Column::Source
            ]
        );
        assert!(specs[0].shown, "the message always shows");
        assert_eq!(specs[1].width, Column::Thread.default_width());
        assert_eq!(specs[2].width, MAX_WIDTH);
        assert!(
            specs[3].shown && !specs[5].shown,
            "left-out columns take their defaults"
        );
    }

    #[test]
    fn names_match_in_any_case() {
        assert_eq!(Column::from_name(" Source "), Some(Column::Source));
        assert_eq!(Column::from_name("lvl"), None);
        assert_eq!(Column::Level.title(), "Lvl");
        assert_eq!(Column::Level.label(), "Level");
    }
}
