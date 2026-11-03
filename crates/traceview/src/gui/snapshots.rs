//! Draws the whole window with the real renderer, offscreen, and checks the
//! pixels. Set `TV_SNAPSHOT_DIR` to also save each frame as a PNG to look at.

use super::app::fonts_for;
use super::columns::Column;
use super::log_view::RowSource;
use super::settings::{Rule, Settings, Store};
use super::workspace::command;
use super::workspace::{ThemeChoice, Workspace};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tv_platform::{Commands, Waker};
use tv_render::Renderer;
use tv_ui::recording::RecordingCanvas;
use tv_ui::{Color, Event, Key, Modifiers, Rect, Theme};

const NONE: Modifiers = Modifiers {
    ctrl: false,
    shift: false,
    alt: false,
};
const CTRL: Modifiers = Modifiers {
    ctrl: true,
    shift: false,
    alt: false,
};
const ALT: Modifiers = Modifiers {
    ctrl: false,
    shift: false,
    alt: true,
};

fn sample_log(dir: &tempfile::TempDir) -> PathBuf {
    let messages = [
        (1, "Connected to management point mp01.contoso.com"),
        (1, "Sending state message body to the management point"),
        (3, "Failed to download policy from mp01 - error 0x80004005"),
        (2, "Retrying policy download in 15 minutes"),
        (1, "Software update scan completed, 3 updates applicable"),
        (3, "Content download failed for KB5034441: hash mismatch"),
        (1, "Client certificate validation succeeded"),
    ];
    let mut text = String::new();
    for row in 0..2000 {
        let (kind, message) = messages[row % messages.len()];
        text.push_str(&format!(
            "<![LOG[{message}]LOG]!><time=\"10:{:02}:{:02}.{:03}+000\" date=\"04-06-2026\" component=\"PolicyAgent\" context=\"\" type=\"{kind}\" thread=\"4128\" file=\"a.cpp\">\r\n",
            row / 60 % 60,
            row % 60,
            row % 1000
        ));
    }
    let path = dir.path().join("PolicyAgent.log");
    std::fs::write(&path, text).unwrap();
    path
}

struct Frame {
    pixels: Vec<u8>,
    width: u32,
    scale: f32,
}

impl Frame {
    fn at(&self, x: f32, y: f32) -> Color {
        let at = (((y * self.scale) as u32 * self.width + (x * self.scale) as u32) * 4) as usize;
        Color {
            r: self.pixels[at],
            g: self.pixels[at + 1],
            b: self.pixels[at + 2],
        }
    }

    /// The colour just inside a row's right edge, clear of its text.
    fn row_color(&self, workspace: &Workspace, row: usize) -> Color {
        let rect = workspace.active_tab().unwrap().view.row_rect(row);
        self.at(rect.right - 30.0, rect.top + rect.height() / 2.0)
    }

    fn has_ink(&self, rect: Rect, background: Color) -> bool {
        let mut y = rect.top;
        while y < rect.bottom {
            let mut x = rect.left;
            while x < rect.right {
                if self.at(x, y) != background {
                    return true;
                }
                x += 0.5;
            }
            y += 0.5;
        }
        false
    }
}

fn save(name: &str, frame: &Frame, height: u32) {
    let Some(dir) = std::env::var_os("TV_SNAPSHOT_DIR") else {
        return;
    };
    let path = PathBuf::from(dir).join(format!("{name}.png"));
    let file = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    let mut encoder = png::Encoder::new(file, frame.width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(&frame.pixels).unwrap();
    writer.finish().unwrap();
}

fn send(workspace: &mut Workspace, event: Event) {
    workspace.event(event, &mut RecordingCanvas::new(), &mut Commands::default());
}

/// Draws frames the way the window would until the visible rows and the
/// details pane have loaded and `done` holds, then keeps the last frame.
fn render(
    workspace: &mut Workspace,
    name: &str,
    dpi: f32,
    done: impl Fn(&Workspace) -> bool,
) -> Frame {
    let scale = dpi / 96.0;
    let (width, height) = ((1200.0 * scale) as u32, (640.0 * scale) as u32);
    let mut renderer = Renderer::offscreen(width, height, dpi).unwrap();
    renderer.set_fonts(fonts_for(workspace.theme()));
    let size = renderer.logical_size();
    let started = Instant::now();
    loop {
        let mut commands = Commands::default();
        workspace.woken(&mut commands);
        // Checked before drawing, so the frame kept was drawn with everything loaded.
        let loaded = workspace.active_tab().is_none_or(|tab| {
            // An empty view right after opening hasn't been laid out yet, so it doesn't count.
            let rows_ready = tab.rows().is_some_and(|rows| {
                let mut visible = tab.view.visible_rows().peekable();
                rows.row_count() == 0
                    || (visible.peek().is_some() && visible.all(|row| rows.cells(row).is_some()))
            });
            let details_ready = tab.cursor_file_row().is_none() || tab.details.shown().is_some();
            rows_ready && details_ready
        });
        let finished = loaded && done(workspace);
        renderer.begin();
        workspace.draw(&mut renderer, size, scale, &mut commands);
        renderer.end().unwrap();
        if finished {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{name}: never finished loading"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let frame = Frame {
        pixels: renderer.rgba_pixels().unwrap(),
        width,
        scale,
    };
    save(name, &frame, height);
    frame
}

fn opened(theme: ThemeChoice) -> (tempfile::TempDir, Workspace) {
    let dir = tempfile::tempdir().unwrap();
    let path = sample_log(&dir);
    let mut workspace = Workspace::new(Waker::detached(), 0, false);
    workspace.theme_choice = theme;
    workspace.open_paths(vec![path]);
    // The checks look at the first rows, so stay at the top rather than following.
    workspace.tabs[0].following = false;
    (dir, workspace)
}

fn check_theme(choice: ThemeChoice, name: &str, dpi: f32) {
    let (_dir, mut workspace) = opened(choice);
    // Select once the file is open, so there's a selected row to check.
    render(&mut workspace, name, dpi, |workspace| {
        workspace.tabs[0].document().is_some()
    });
    workspace.tabs[0].select_file_row(9);
    let frame = render(&mut workspace, name, dpi, |_| true);
    let theme = workspace.theme();

    assert_eq!(
        frame.row_color(&workspace, 2),
        theme.error_row,
        "{name}: error row"
    );
    assert_eq!(
        frame.row_color(&workspace, 3),
        theme.warning_row,
        "{name}: warning row"
    );
    assert_eq!(
        frame.row_color(&workspace, 9),
        theme.selected_row,
        "{name}: selected row"
    );

    let first = workspace.tabs[0].view.row_rect(0);
    let message = Rect::new(
        first.left + 340.0,
        first.top,
        first.left + 640.0,
        first.bottom,
    );
    assert!(
        frame.has_ink(message, theme.grid_background),
        "{name}: the first row's text didn't draw"
    );
    let menu_bar = Rect::new(0.0, 0.0, 200.0, 24.0);
    assert!(
        frame.has_ink(menu_bar, frame.at(199.0, 2.0)),
        "{name}: the menu titles didn't draw"
    );
    let filter = workspace.filter_rect();
    assert_eq!(
        frame.at(filter.right - 4.0, filter.top + filter.height() / 2.0),
        theme.input_background
    );
}

#[test]
fn every_theme_renders_correctly() {
    for theme in Theme::ALL {
        check_theme(
            ThemeChoice::Fixed(theme),
            &format!("{}-100", theme.name.to_lowercase()),
            96.0,
        );
    }
}

#[test]
fn fractional_display_scale_renders_correctly() {
    check_theme(ThemeChoice::Fixed(&Theme::CLASSIC), "classic-125", 120.0);
}

#[test]
fn high_density_display_renders_correctly() {
    check_theme(ThemeChoice::Fixed(&Theme::DARK), "dark-200", 192.0);
}

#[test]
fn find_matches_are_highlighted() {
    let (_dir, mut workspace) = opened(ThemeChoice::Fixed(&Theme::CLASSIC));
    render(&mut workspace, "find", 96.0, |workspace| {
        workspace.tabs[0].document().is_some()
    });
    send(&mut workspace, Event::KeyDown(Key::Char('F'), CTRL));
    for character in "hash mismatch".chars() {
        send(&mut workspace, Event::Char(character));
    }
    send(&mut workspace, Event::KeyDown(Key::Enter, NONE));
    let frame = render(&mut workspace, "find", 96.0, |workspace| {
        workspace.tabs[0].cursor_file_row().is_some()
    });
    let theme = workspace.theme();
    // The first match is selected; the next one is highlighted over its error colour.
    assert_eq!(workspace.tabs[0].cursor_file_row(), Some(5));
    assert_eq!(frame.row_color(&workspace, 12), theme.highlight_row);
    assert_eq!(frame.row_color(&workspace, 2), theme.error_row);
}

#[test]
fn an_open_menu_draws_over_the_grid() {
    let (_dir, mut workspace) = opened(ThemeChoice::Fixed(&Theme::CONSOLE));
    render(&mut workspace, "menu", 96.0, |workspace| {
        workspace.tabs[0].document().is_some()
    });
    send(&mut workspace, Event::KeyDown(Key::Char('F'), ALT));
    let frame = render(&mut workspace, "menu", 96.0, |_| true);
    let theme = workspace.theme();
    let highlighted = workspace.menu().item_rect(0).unwrap();
    let plain = workspace.menu().item_rect(1).unwrap();
    assert_eq!(
        frame.at(highlighted.left + 3.0, highlighted.top + 3.0),
        theme.hover_background
    );
    assert_eq!(
        frame.at(plain.left + 3.0, plain.top + 3.0),
        theme.panel_background
    );
}

#[test]
fn welcome_screen_shows_with_no_log() {
    let mut workspace = Workspace::new(Waker::detached(), 0, false);
    workspace.theme_choice = ThemeChoice::Fixed(&Theme::CLASSIC);
    let frame = render(&mut workspace, "welcome", 96.0, |_| true);
    let grid = workspace.grid_rect();
    let welcome = Rect::new(grid.left, grid.top, grid.left + 400.0, grid.top + 60.0);
    assert!(frame.has_ink(welcome, workspace.theme().window_background));
}

#[test]
fn error_lookup_explains_the_selected_rows_code() {
    let (_dir, mut workspace) = opened(ThemeChoice::Fixed(&Theme::CLASSIC));
    render(&mut workspace, "lookup", 96.0, |workspace| {
        workspace.tabs[0].document().is_some()
    });
    workspace.tabs[0].select_file_row(2);
    render(&mut workspace, "lookup", 96.0, |workspace| {
        workspace.tabs[0].details.shown().is_some()
    });
    send(&mut workspace, Event::KeyDown(Key::Char('L'), CTRL));
    let frame = render(&mut workspace, "lookup", 96.0, |workspace| {
        workspace
            .lookup
            .as_ref()
            .is_some_and(|panel| !panel.result().is_empty())
    });
    let theme = workspace.theme();
    let panel = workspace.lookup.as_ref().unwrap();
    assert!(panel.result().starts_with("HRESULT 0x80004005"));
    let rect = panel.rect();
    assert_eq!(
        frame.at(rect.right - 4.0, rect.top + 4.0),
        theme.panel_background
    );
    let result = Rect::new(
        rect.left + 14.0,
        panel.field_rect().bottom + 10.0,
        rect.right - 14.0,
        panel.field_rect().bottom + 40.0,
    );
    assert!(
        frame.has_ink(result, theme.panel_background),
        "the result didn't draw"
    );
}

#[test]
fn highlight_rules_color_rows_and_mark_them_in_every_theme() {
    for theme in Theme::ALL {
        let (_dir, mut workspace) = opened(ThemeChoice::Fixed(theme));
        let settings = Settings {
            rules: vec![
                Rule {
                    pattern: "hash mismatch".to_string(),
                    color: "blue".to_string(),
                    ..Rule::default()
                },
                Rule {
                    pattern: "Retrying".to_string(),
                    color: "red".to_string(),
                    style: "marker".to_string(),
                    ..Rule::default()
                },
            ],
            ..Settings::default()
        };
        workspace.use_settings(settings, Store::disabled(), None);
        workspace.theme_choice = ThemeChoice::Fixed(theme);
        let name = format!("rules-{}", theme.name.to_lowercase());
        let frame = render(&mut workspace, &name, 96.0, |workspace| {
            workspace.tabs[0]
                .rows()
                .and_then(|rows| rows.cells(5))
                .is_some_and(|cells| cells.mark.is_some())
        });
        let colors = theme.rule_colors;
        // Row 6 is an error matching the blue rule, which colors the whole row.
        assert_eq!(
            frame.row_color(&workspace, 5),
            colors[4].row,
            "{name}: rule row"
        );
        // Row 4 is a warning matching the red marker rule, so it stays a warning.
        assert_eq!(frame.row_color(&workspace, 3), theme.warning_row, "{name}");
        let marker = workspace.tabs[0].view.marker_rect(3);
        assert_eq!(
            frame.at(marker.left + 2.0, marker.top + marker.height() / 2.0),
            colors[0].marker,
            "{name}: marker"
        );
    }
}

#[test]
fn the_highlight_rules_editor_draws_over_the_logs() {
    let (_dir, mut workspace) = opened(ThemeChoice::Fixed(&Theme::DARK));
    let settings = Settings {
        rules: vec![Rule {
            pattern: "download failed level:error".to_string(),
            ..Rule::default()
        }],
        ..Settings::default()
    };
    workspace.use_settings(settings, Store::disabled(), None);
    workspace.theme_choice = ThemeChoice::Fixed(&Theme::DARK);
    render(&mut workspace, "rules-editor", 96.0, |workspace| {
        workspace.tabs[0].document().is_some()
    });
    send(&mut workspace, Event::KeyDown(Key::Char('H'), CTRL));
    let frame = render(&mut workspace, "rules-editor", 96.0, |_| true);
    let theme = workspace.theme();
    let panel = workspace.rules_panel.as_ref().unwrap();
    let rect = panel.rect();
    assert_eq!(
        frame.at(rect.right - 4.0, rect.top + 4.0),
        theme.panel_background
    );
    let field = panel.field_rect();
    assert_eq!(
        frame.at(field.right - 4.0, field.top + field.height() / 2.0),
        theme.input_background
    );
    let first_rule = panel.list.item_rect(0).unwrap();
    assert!(
        frame.has_ink(first_rule, theme.sidebar_background),
        "the rule list didn't draw"
    );
}

#[test]
fn sccm_logs_show_the_thread_and_the_columns_editor_draws() {
    let (_dir, mut workspace) = opened(ThemeChoice::Fixed(&Theme::CONSOLE));
    let frame = render(&mut workspace, "columns", 96.0, |workspace| {
        workspace.tabs[0].document().is_some()
    });
    let theme = workspace.theme();
    let view = &workspace.tabs[0].view;
    let thread = view.header_rect(Column::Thread).unwrap();
    assert!(
        frame.has_ink(thread, theme.header_background),
        "the Thread header didn't draw"
    );
    workspace.run_command(command::COLUMNS, &mut Commands::default());
    let frame = render(&mut workspace, "columns-editor", 96.0, |_| true);
    let panel = workspace.columns_panel.as_ref().unwrap();
    assert_eq!(panel.title(), "Columns for SCCM logs");
    let rect = panel.rect();
    assert_eq!(
        frame.at(rect.right - 4.0, rect.top + 4.0),
        theme.panel_background
    );
    let first = panel.list.item_rect(0).unwrap();
    assert!(
        frame.has_ink(first, theme.selected_row),
        "the column list didn't draw"
    );
}

#[test]
fn the_sidebar_draws_beside_the_logs() {
    let (_dir, mut workspace) = opened(ThemeChoice::Fixed(&Theme::CLASSIC));
    workspace.show_sidebar = true;
    let frame = render(&mut workspace, "sidebar", 96.0, |workspace| {
        workspace.tabs[0].document().is_some() && workspace.sidebar.list.items.len() > 3
    });
    let theme = workspace.theme();
    let grid = workspace.grid_rect();
    assert!(grid.left >= workspace.sidebar.width());
    assert_eq!(frame.at(4.0, 600.0), theme.sidebar_background);
    let first = workspace.sidebar.list.item_rect(1).unwrap();
    assert!(
        frame.has_ink(first, theme.sidebar_background),
        "the first entry's text drew"
    );
}

#[test]
fn chips_and_the_histogram_draw_above_the_grid() {
    let (_dir, mut workspace) = opened(ThemeChoice::Fixed(&Theme::DARK));
    workspace.show_histogram = true;
    let frame = render(&mut workspace, "overview", 96.0, |workspace| {
        workspace.histogram_rect().height() > 0.0
    });
    let theme = workspace.theme();
    let (error_chip, rect) = workspace
        .chip_list()
        .find(|(chip, _)| chip.label == "Error")
        .unwrap();
    assert_eq!(error_chip.count, Some(571));
    assert!(frame.has_ink(rect.unwrap(), theme.chip_background));
    let bars = workspace.histogram_rect();
    let error_bar = Rect::new(bars.left, bars.top, bars.right - 4.0, bars.top + 28.0);
    let mut red = false;
    let mut y = error_bar.top;
    while y < error_bar.bottom && !red {
        let mut x = error_bar.left;
        while x < error_bar.right && !red {
            red = frame.at(x, y) == theme.histogram_error;
            x += 1.0;
        }
        y += 1.0;
    }
    assert!(red, "error segments drew");
}

#[test]
fn the_command_palette_draws_over_everything() {
    let (_dir, mut workspace) = opened(ThemeChoice::Fixed(&Theme::CLASSIC));
    render(&mut workspace, "palette", 96.0, |workspace| {
        workspace.tabs[0].document().is_some()
    });
    send(&mut workspace, Event::KeyDown(Key::Char('K'), CTRL));
    for character in "theme".chars() {
        send(&mut workspace, Event::Char(character));
    }
    let frame = render(&mut workspace, "palette", 96.0, |_| true);
    let theme = workspace.theme();
    let palette = &workspace.palette.as_ref().unwrap().palette;
    assert!(palette.match_count() >= 5, "every theme matches");
    let first = palette.match_rect(0).unwrap();
    assert!(frame.has_ink(first, frame.at(first.right - 2.0, first.top + 2.0)));
    let _ = theme;
}

#[test]
fn split_view_draws_two_logs_and_marks_the_focused_one() {
    let dir = tempfile::tempdir().unwrap();
    let first = sample_log(&dir);
    let second = dir.path().join("Second.log");
    std::fs::copy(&first, &second).unwrap();
    let mut workspace = Workspace::new(Waker::detached(), 0, false);
    workspace.theme_choice = ThemeChoice::Fixed(&Theme::CONSOLE);
    workspace.open_paths(vec![first, second]);
    render(&mut workspace, "split", 96.0, |workspace| {
        workspace.tabs.iter().all(|tab| tab.document().is_some())
    });
    for tab in &mut workspace.tabs {
        tab.following = false;
        tab.view.scroll_to(0.0);
    }
    send(&mut workspace, Event::KeyDown(Key::Char('\\'), CTRL));
    let frame = render(&mut workspace, "split", 96.0, |workspace| {
        workspace.split.is_some()
    });
    let theme = workspace.theme();
    let focused = workspace.tabs[workspace.active].view.row_rect(2);
    assert_eq!(
        frame.at(focused.left + 200.0, focused.top + 8.0),
        theme.error_row,
        "the focused pane's error row"
    );
    let other = workspace.tabs[0].view.row_rect(2);
    assert_eq!(
        frame.at(other.left + 200.0, other.top + 8.0),
        theme.error_row,
        "the other pane's error row"
    );
    assert_eq!(
        frame.at(20.0, 52.0 + 1.0),
        theme.accent,
        "the focused pane is marked"
    );
}
