//! Named layouts: which of the sidebar, severity chips, histogram and details
//! pane show, and how wide the sidebar is. Two come built in, and any
//! arrangement can be saved under a name in `traceview.toml`. The status bar
//! always shows, since it carries row counts, live state and every message.

use super::settings::LayoutSetting;

/// The layouts that come with the app. They can't be deleted or replaced.
pub fn built_in() -> [LayoutSetting; 2] {
    [
        // The grid and its details, as CMTrace shows a log.
        LayoutSetting {
            name: "CMTrace".to_string(),
            details_pane: true,
            ..LayoutSetting::default()
        },
        LayoutSetting {
            name: "Everything".to_string(),
            sidebar: true,
            sidebar_width: None,
            chips: true,
            histogram: true,
            details_pane: true,
        },
    ]
}

pub fn is_built_in(name: &str) -> bool {
    built_in()
        .iter()
        .any(|layout| layout.name.eq_ignore_ascii_case(name.trim()))
}

/// Whether the window shows what `layout` does, given what it shows `now`.
/// The sidebar's width only counts while the sidebar shows.
pub fn is_shown(layout: &LayoutSetting, now: &LayoutSetting) -> bool {
    let same_width = match (layout.sidebar_width, now.sidebar_width) {
        (Some(width), Some(current)) => (width - current).abs() < 0.5,
        _ => true,
    };
    layout.sidebar == now.sidebar
        && layout.chips == now.chips
        && layout.histogram == now.histogram
        && layout.details_pane == now.details_pane
        && (!layout.sidebar || same_width)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now(sidebar: bool, width: f32) -> LayoutSetting {
        LayoutSetting {
            name: String::new(),
            sidebar,
            sidebar_width: Some(width),
            chips: false,
            histogram: false,
            details_pane: true,
        }
    }

    #[test]
    fn built_in_names_are_known_in_any_case() {
        assert!(is_built_in("cmtrace"));
        assert!(is_built_in(" Everything "));
        assert!(!is_built_in("Mine"));
    }

    #[test]
    fn a_layout_is_shown_when_its_panes_match() {
        let [cmtrace, everything] = built_in();
        assert!(is_shown(&cmtrace, &now(false, 240.0)));
        assert!(!is_shown(&everything, &now(false, 240.0)));
        let mut wide = now(true, 400.0);
        wide.name = "Wide".to_string();
        assert!(is_shown(&wide, &now(true, 400.2)));
        assert!(!is_shown(&wide, &now(true, 300.0)), "the width counts");
        let mut hidden = now(false, 400.0);
        hidden.name = "Hidden".to_string();
        assert!(
            is_shown(&hidden, &now(false, 300.0)),
            "but not while the sidebar is hidden"
        );
    }
}
