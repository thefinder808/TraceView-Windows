//! Highlight rules: rows matching a rule's pattern are colored, or marked, in
//! the rule's color. Patterns use the filter box's syntax. Rules are checked as
//! rows load for the screen, so a rule costs nothing on rows nobody looks at,
//! however large the log.

use super::search;
use super::settings::Rule;
use tv_engine::query::Matcher;
use tv_engine::LogFile;
use tv_ui::theme::RULE_COLOR_NAMES;

/// Yellow, as in CMTrace's highlight.
const FIRST_COLOR: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    /// The whole row takes the rule's color.
    Row,
    /// A mark by the row number, leaving the row's severity color alone.
    Marker,
}

impl Style {
    pub fn name(self) -> &'static str {
        match self {
            Style::Row => "row",
            Style::Marker => "marker",
        }
    }

    /// Anything but "marker", perhaps from a newer version, colors the row.
    pub fn from_name(name: &str) -> Self {
        if name.eq_ignore_ascii_case("marker") {
            Style::Marker
        } else {
            Style::Row
        }
    }
}

/// How a matching rule shows on a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mark {
    /// Into `Theme::rule_colors`.
    pub color: usize,
    pub style: Style,
}

/// A color by name, in any case. A name this version doesn't know gets yellow.
pub fn color_index(name: &str) -> usize {
    RULE_COLOR_NAMES
        .iter()
        .position(|known| known.eq_ignore_ascii_case(name))
        .unwrap_or(FIRST_COLOR)
}

/// The name a color is saved under.
pub fn color_name(index: usize) -> String {
    RULE_COLOR_NAMES[index % RULE_COLOR_NAMES.len()].to_lowercase()
}

/// A different color for each new rule, starting with yellow.
pub fn color_for_new_rule(existing: usize) -> usize {
    (FIRST_COLOR + existing) % RULE_COLOR_NAMES.len()
}

pub fn mark_of(rule: &Rule) -> Mark {
    Mark {
        color: color_index(&rule.color),
        style: Style::from_name(&rule.style),
    }
}

/// Says what's wrong with a pattern, if anything. An empty one is fine, but
/// it never matches.
pub fn check(pattern: &str) -> Result<(), String> {
    search::parse(pattern).map(|_| ())
}

/// Rules ready to test rows, in order.
#[derive(Default)]
pub struct RuleSet {
    rules: Vec<(Matcher, Mark)>,
}

impl RuleSet {
    /// Compiles the turned-on rules that have a pattern. One that doesn't
    /// parse is left out, and the editor says why.
    pub fn compile(rules: &[Rule]) -> Self {
        let rules = rules
            .iter()
            .filter(|rule| rule.enabled && !rule.pattern.trim().is_empty())
            .filter_map(|rule| {
                let query = search::parse(&rule.pattern).ok()?;
                Some((Matcher::new(&query).ok()?, mark_of(rule)))
            })
            .collect();
        Self { rules }
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// How the first matching rule shows on a row, given the row's text from
    /// [`LogFile::text`].
    pub fn mark(&self, log: &LogFile, row: usize, text: &[u8]) -> Option<Mark> {
        self.rules
            .iter()
            .find(|(matcher, _)| matcher.matches(log, row, text))
            .map(|(_, mark)| *mark)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    fn rule(pattern: &str, color: &str, style: &str) -> Rule {
        Rule {
            pattern: pattern.to_string(),
            color: color.to_string(),
            style: style.to_string(),
            enabled: true,
        }
    }

    fn marks(text: &str, rules: &[Rule]) -> Vec<Option<Mark>> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.log");
        std::fs::write(&path, text).unwrap();
        let (log, _) = LogFile::open_with_cache(&path, None, &AtomicU64::new(0)).unwrap();
        let set = RuleSet::compile(rules);
        let (mut buffer, mut scratch, mut found) = (Vec::new(), Vec::new(), Vec::new());
        log.read_rows(0..log.rows(), &mut buffer, |row, raw, _| {
            found.push(set.mark(&log, row, log.text(raw, &mut scratch)));
        })
        .unwrap();
        found
    }

    const LOG: &str = "2026-01-01 10:00:00 host app[1]: ERROR: download failed\n\
                       2026-01-01 10:00:01 host app[1]: download finished\n\
                       2026-01-01 10:00:02 host other[2]: policy applied\n";

    #[test]
    fn the_first_matching_rule_wins() {
        let rules = [
            rule("download level:error", "red", "row"),
            rule("download", "blue", "marker"),
        ];
        let red = Mark {
            color: 0,
            style: Style::Row,
        };
        let blue = Mark {
            color: 4,
            style: Style::Marker,
        };
        assert_eq!(marks(LOG, &rules), vec![Some(red), Some(blue), None]);
    }

    #[test]
    fn patterns_use_the_filter_syntax() {
        let rules = [rule("component:other", "green", "row")];
        assert_eq!(marks(LOG, &rules)[2].map(|mark| mark.color), Some(3));
        let rules = [rule("/fail|finish/", "green", "row")];
        let found = marks(LOG, &rules);
        assert!(found[0].is_some() && found[1].is_some() && found[2].is_none());
    }

    #[test]
    fn rules_turned_off_empty_or_broken_never_match() {
        let mut off = rule("download", "red", "row");
        off.enabled = false;
        let rules = [off, rule("  ", "red", "row"), rule("/(/", "red", "row")];
        assert!(RuleSet::compile(&rules).is_empty());
        assert!(check("/(/").is_err());
        assert!(check("level:sometimes").is_err());
        assert!(check("").is_ok());
    }

    #[test]
    fn unknown_names_fall_back_rather_than_fail() {
        let mark = mark_of(&rule("x", "Teal", "glow"));
        assert_eq!(mark.color, FIRST_COLOR);
        assert_eq!(mark.style, Style::Row);
        assert_eq!(color_index("PURPLE"), 5);
        assert_eq!(color_name(4), "blue");
        assert_eq!(Style::from_name("Marker"), Style::Marker);
    }

    #[test]
    fn new_rules_each_get_another_color() {
        let colors: Vec<usize> = (0..7).map(color_for_new_rule).collect();
        assert_eq!(colors, vec![2, 3, 4, 5, 0, 1, 2]);
    }
}
