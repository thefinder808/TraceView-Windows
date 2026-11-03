//! The command palette (Ctrl+K): every menu command, open tab, recent log and
//! filter preset in one searchable list. Typing ":" and a number goes to that
//! row, which is what Ctrl+G opens it for. It also asks for a preset's or a
//! layout's name, and lists installed fonts to choose from.

use super::fonts::Target;
use super::layouts;
use std::path::PathBuf;
use tv_ui::palette::{Palette, PaletteItem};
use tv_ui::{Canvas, Event, Point, Rect, Theme};

/// What choosing an entry does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Command(u32),
    Tab(usize),
    Open(PathBuf),
    /// A row of the tab, counting from 0.
    GoToRow(usize),
    ApplyPreset(usize),
    DeletePreset(usize),
    SavePreset(String),
    /// A saved layout, counting from 0 after the built-in ones.
    DeleteLayout(usize),
    SaveLayout(String),
    /// A font family, by name.
    SetFont(Target, String),
    /// Asks for a command's new shortcut.
    Rebind(u32),
    /// Puts every shortcut back to its default.
    ResetKeys,
    /// A hint that does nothing when chosen.
    Nothing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Everything,
    /// Asks for the name to save the current filter under.
    SavePreset,
    /// Asks for the name to save the panes shown now under.
    SaveLayout,
    /// Lists installed fonts for the log or the window.
    Font(Target),
    /// Lists commands with their shortcuts, to change one.
    Keys,
}

/// What happened, for the workspace to act on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub redraw: bool,
    pub chosen: Option<Action>,
    pub cancelled: bool,
    pub copy: Option<String>,
    pub wants_paste: bool,
}

pub struct CommandPalette {
    pub palette: Palette,
    mode: Mode,
    /// What each listed item does, by its id.
    actions: Vec<Action>,
    /// What's listed when the text isn't a row number.
    everything: Vec<(PaletteItem, Action)>,
    rows: usize,
    query: String,
}

impl CommandPalette {
    /// Opens the palette. `everything` is what it lists, and `rows` is how many
    /// rows the active tab has, for going to one.
    pub fn open(
        mode: Mode,
        everything: Vec<(PaletteItem, Action)>,
        rows: usize,
        query: &str,
    ) -> Self {
        let mut palette = Self {
            palette: Palette::new(Vec::new()),
            mode,
            actions: Vec::new(),
            everything,
            rows,
            query: String::new(),
        };
        palette.palette.set_query(query);
        palette.refresh();
        palette
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// What the match at `index` does.
    pub fn action_at(&self, index: usize) -> Option<Action> {
        let item = self.palette.matched_item(index)?;
        self.actions.get(item.id as usize).cloned()
    }

    pub fn placeholder(&self) -> &'static str {
        match self.mode {
            Mode::Everything => "Type a command, a log, a preset, or : and a row number",
            Mode::SavePreset => "Name for this filter preset",
            Mode::SaveLayout => "Name for this layout",
            Mode::Font(Target::Log) => "Type to find a font. Fixed-width fonts line logs up best",
            Mode::Font(Target::Window) => "Type to find a font",
            Mode::Keys => "Type to find a command, then press Enter to change its shortcut",
        }
    }

    /// The palette's name, for screen readers.
    pub fn title(&self) -> &'static str {
        match self.mode {
            Mode::Everything => "Command palette",
            Mode::SavePreset => "Save filter preset",
            Mode::SaveLayout => "Save layout",
            Mode::Font(Target::Log) => "Choose the log font",
            Mode::Font(Target::Window) => "Choose the window font",
            Mode::Keys => "Keyboard shortcuts",
        }
    }

    /// Lists what fits the text typed so far.
    fn refresh(&mut self) {
        let query = self.palette.query().to_string();
        let listed: Vec<(PaletteItem, Action)> = match self.mode {
            Mode::SavePreset => {
                let name = query.trim();
                if name.is_empty() {
                    Vec::new()
                } else {
                    vec![(
                        PaletteItem::new(&format!("Save filter preset: {name}"), "", "Enter", 0),
                        Action::SavePreset(name.to_string()),
                    )]
                }
            }
            Mode::SaveLayout => {
                let name = query.trim();
                if name.is_empty() {
                    Vec::new()
                } else if layouts::is_built_in(name) {
                    vec![(
                        PaletteItem::new(
                            &format!("{name}: a built-in layout has this name, so choose another"),
                            "",
                            "",
                            0,
                        ),
                        Action::Nothing,
                    )]
                } else {
                    vec![(
                        PaletteItem::new(&format!("Save layout: {name}"), "", "Enter", 0),
                        Action::SaveLayout(name.to_string()),
                    )]
                }
            }
            Mode::Everything => match query.strip_prefix(':') {
                Some(number) => {
                    let row = number
                        .trim()
                        .parse::<usize>()
                        .ok()
                        .filter(|row| (1..=self.rows).contains(row));
                    let item = match row {
                        // The label starts with what was typed, so it matches.
                        Some(row) => (
                            PaletteItem::new(
                                &format!(":{number}  Go to row {row}"),
                                "",
                                "Enter",
                                0,
                            ),
                            Action::GoToRow(row - 1),
                        ),
                        None => (
                            PaletteItem::new(
                                &format!(":{number}  Type a row number from 1 to {}", self.rows),
                                "",
                                "",
                                0,
                            ),
                            Action::Nothing,
                        ),
                    };
                    vec![item]
                }
                None => self.everything.clone(),
            },
            Mode::Font(_) | Mode::Keys => self.everything.clone(),
        };
        let (mut items, actions): (Vec<PaletteItem>, Vec<Action>) = listed.into_iter().unzip();
        for (id, item) in items.iter_mut().enumerate() {
            item.id = id as u32;
        }
        self.palette.set_items(items);
        self.actions = actions;
        self.query = query;
    }

    pub fn arrange(&mut self, area: Rect) {
        self.palette.arrange(area);
    }

    pub fn handle(&mut self, event: Event, canvas: &mut dyn Canvas) -> Outcome {
        let response = self.palette.handle(event, canvas);
        if self.palette.query() != self.query {
            self.refresh();
        }
        Outcome {
            redraw: response.redraw,
            chosen: response
                .chosen
                .and_then(|id| self.actions.get(id as usize).cloned()),
            cancelled: response.cancelled,
            copy: response.copy,
            wants_paste: response.wants_paste,
        }
    }

    /// Adds pasted text to what's typed.
    pub fn paste(&mut self, text: &str) {
        if self.palette.paste(text) {
            self.refresh();
        }
    }

    /// Draws the palette and returns its caret, for the IME.
    pub fn draw(&mut self, canvas: &mut dyn Canvas, theme: &Theme) -> Point {
        let placeholder = self.placeholder();
        self.palette.draw(canvas, theme, placeholder)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tv_ui::recording::RecordingCanvas;
    use tv_ui::{Key, Modifiers};

    fn type_text(palette: &mut CommandPalette, text: &str) -> Outcome {
        let mut canvas = RecordingCanvas::new();
        palette.arrange(Rect::new(0.0, 0.0, 800.0, 600.0));
        let mut outcome = Outcome::default();
        for character in text.chars() {
            outcome = palette.handle(Event::Char(character), &mut canvas);
        }
        outcome
    }

    fn enter(palette: &mut CommandPalette) -> Option<Action> {
        palette
            .handle(
                Event::KeyDown(Key::Enter, Modifiers::default()),
                &mut RecordingCanvas::new(),
            )
            .chosen
    }

    fn everything() -> Vec<(PaletteItem, Action)> {
        vec![
            (
                PaletteItem::new("Open...", "", "Ctrl+O", 0),
                Action::Command(1),
            ),
            (
                PaletteItem::new("Toggle bookmark", "", "Ctrl+D", 0),
                Action::Command(32),
            ),
            (
                PaletteItem::new("PolicyAgent.log", "Open tab", "", 0),
                Action::Tab(0),
            ),
        ]
    }

    #[test]
    fn typing_narrows_the_list_and_enter_chooses() {
        let mut palette = CommandPalette::open(Mode::Everything, everything(), 100, "");
        assert_eq!(palette.palette.match_count(), 3);
        type_text(&mut palette, "bkmk");
        assert_eq!(palette.palette.match_count(), 1);
        assert_eq!(enter(&mut palette), Some(Action::Command(32)));
    }

    #[test]
    fn a_colon_and_a_number_goes_to_that_row() {
        let mut palette = CommandPalette::open(Mode::Everything, everything(), 100, ":");
        assert_eq!(
            enter(&mut palette),
            Some(Action::Nothing),
            "a hint until a number is typed"
        );
        type_text(&mut palette, "42");
        assert_eq!(enter(&mut palette), Some(Action::GoToRow(41)));
        type_text(&mut palette, "0");
        assert_eq!(
            enter(&mut palette),
            Some(Action::Nothing),
            "row 420 is past the end"
        );
    }

    #[test]
    fn saving_a_preset_asks_for_its_name() {
        let mut palette = CommandPalette::open(Mode::SavePreset, Vec::new(), 100, "");
        assert_eq!(palette.palette.match_count(), 0);
        type_text(&mut palette, "Download errors");
        assert_eq!(
            enter(&mut palette),
            Some(Action::SavePreset("Download errors".to_string()))
        );
    }

    #[test]
    fn saving_a_layout_refuses_built_in_names() {
        let mut palette = CommandPalette::open(Mode::SaveLayout, Vec::new(), 100, "");
        type_text(&mut palette, "cmtrace");
        assert_eq!(enter(&mut palette), Some(Action::Nothing));
        assert!(palette
            .palette
            .matched_item(0)
            .is_some_and(|item| item.label.contains("a built-in layout has this name")));
        type_text(&mut palette, " wide");
        assert_eq!(
            enter(&mut palette),
            Some(Action::SaveLayout("cmtrace wide".to_string()))
        );
    }

    #[test]
    fn escape_cancels() {
        let mut palette = CommandPalette::open(Mode::Everything, everything(), 100, "");
        let outcome = palette.handle(
            Event::KeyDown(Key::Escape, Modifiers::default()),
            &mut RecordingCanvas::new(),
        );
        assert!(outcome.cancelled);
    }
}
