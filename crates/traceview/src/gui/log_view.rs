//! The log grid: scrolling, selection, keyboard and drawing. It works through
//! the `Canvas` and `RowSource` interfaces, so it's tested without a window.

use super::columns::{Column, ColumnSpec, MAX_WIDTH, MIN_WIDTH};
use super::row_cache::RowCells;
use super::rules::Style;
use std::sync::Arc;
use tv_engine::Level;
use tv_ui::scrollbar::{self, Press, ScrollState};
use tv_ui::{
    Align, Canvas, Color, Event, FontRole, Key, MouseButton, Point, Rect, TextStyle, Theme,
};

const GRID_LEFT: TextStyle = TextStyle::new(FontRole::Grid, Align::Left);
const GRID_RIGHT: TextStyle = TextStyle::new(FontRole::Grid, Align::Right);
const CELL_PADDING: f32 = 8.0;
const MIN_ROW_NUMBER_CHARS: f32 = 6.0;
/// How near a header's edge a press has to be to resize the column.
const EDGE_GRAB: f32 = 4.0;
/// How far a header has to be dragged before it moves.
const DRAG_START: f32 = 4.0;
/// The bookmark marker, a bar at the left edge of the row number.
const BOOKMARK_WIDTH: f32 = 3.0;
/// A highlight rule's marker, a bar between the row number and the time.
const MARKER_WIDTH: f32 = 4.0;

/// Where rows come from. The GUI backs this with the row cache.
pub trait RowSource {
    fn row_count(&self) -> usize;
    fn level(&self, row: usize) -> Level;
    fn cells(&self, row: usize) -> Option<Arc<RowCells>>;
    /// The row's number in the file. Rows are numbered by their place in the
    /// file even when a filter hides others, so they can be found again.
    fn file_row(&self, row: usize) -> usize {
        row
    }
    /// True for rows matching the find text, which get the highlight color.
    fn highlighted(&self, _row: usize) -> bool {
        false
    }
    /// True for bookmarked rows, which get a marker beside their number.
    fn bookmarked(&self, _row: usize) -> bool {
        false
    }
}

/// What a view needs its owner to do after handling input.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Response {
    pub redraw: bool,
    pub copy_selection: bool,
    pub selection_changed: bool,
    /// A column was resized or moved, so the owner should save [`LogView::columns`].
    pub columns_changed: bool,
    /// The header was right-clicked, which opens the columns editor.
    pub columns_menu: bool,
}

impl Response {
    const REDRAW: Response = Response {
        redraw: true,
        copy_selection: false,
        selection_changed: false,
        columns_changed: false,
        columns_menu: false,
    };
    const SELECTED: Response = Response {
        redraw: true,
        copy_selection: false,
        selection_changed: true,
        columns_changed: false,
        columns_menu: false,
    };
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Drag {
    Thumb {
        grab: f32,
    },
    Rows,
    /// A column's right edge, with `column` indexing `LogView::columns`.
    Resize {
        column: usize,
        left: f32,
    },
    /// A header being dragged to a new place. `target` is where it would land,
    /// as a position among the shown columns, once it has moved far enough.
    Move {
        column: usize,
        start: f32,
        target: Option<usize>,
    },
}

/// A shown column and where it sits.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Placed {
    column: Column,
    left: f32,
    /// Where its text stops. The last column runs to the grid's edge.
    right: f32,
    /// Where it would stop at its own width.
    natural_right: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Metrics {
    pub row_height: f32,
    pub char_width: f32,
    pub header_height: f32,
}

/// Where each part of the grid goes, in device-independent pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Layout {
    header: Rect,
    body: Rect,
    track: Rect,
    row_number_right: f32,
    /// Where the first column starts, after the gap that holds rule markers.
    first_left: f32,
}

#[derive(Default)]
pub struct LogView {
    top: f64,
    /// The selection's fixed end, set by a click or an unshifted move.
    anchor: Option<usize>,
    /// The selection's moving end, and the row keyboard moves start from.
    cursor: Option<usize>,
    drag: Option<Drag>,
    layout: Layout,
    metrics: Metrics,
    rows: usize,
    /// Every column in order, shown or not.
    columns: Vec<ColumnSpec>,
    placed: Vec<Placed>,
}

/// Rounds to a whole number of physical pixels, which keeps rows crisp.
pub fn snap(value: f32, scale: f32) -> f32 {
    (value * scale).round() / scale
}

fn digits(rows: usize) -> f32 {
    let mut count = 1.0;
    let mut remaining = rows / 10;
    while remaining > 0 {
        count += 1.0;
        remaining /= 10;
    }
    count
}

pub fn short_level(level: Level) -> &'static str {
    match level {
        Level::Debug => "DBG",
        Level::Info => "INFO",
        Level::Notice => "NOT",
        Level::Warning => "WARN",
        Level::Error => "ERR",
        Level::Critical => "CRT",
    }
}

pub fn level_name(level: Level) -> &'static str {
    match level {
        Level::Debug => "Debug",
        Level::Info => "Information",
        Level::Notice => "Notice",
        Level::Warning => "Warning",
        Level::Error => "Error",
        Level::Critical => "Critical",
    }
}

impl LogView {
    /// Recomputes where everything goes. Call before drawing or hit-testing.
    /// While a column is being resized or moved, the view keeps its own copy
    /// of the columns until the drag ends.
    pub fn arrange(&mut self, area: Rect, rows: usize, metrics: Metrics, columns: &[ColumnSpec]) {
        self.rows = rows;
        self.metrics = metrics;
        let mut body = area;
        let header = body.take_top(metrics.header_height);
        let track = body.take_right(scrollbar::WIDTH);
        let number_chars = digits(rows).max(MIN_ROW_NUMBER_CHARS);
        let row_number_right = body.left + CELL_PADDING + number_chars * metrics.char_width;
        self.layout = Layout {
            header,
            body,
            track,
            row_number_right,
            first_left: row_number_right + CELL_PADDING * 1.5,
        };
        if !matches!(self.drag, Some(Drag::Resize { .. } | Drag::Move { .. })) {
            self.columns = columns.to_vec();
        }
        self.place();
        self.clamp_top();
    }

    fn place(&mut self) {
        let char_width = self.metrics.char_width;
        let mut left = self.layout.first_left;
        self.placed.clear();
        for spec in self.columns.iter().filter(|spec| spec.shown) {
            let right = left + spec.width * char_width;
            self.placed.push(Placed {
                column: spec.column,
                left,
                right,
                natural_right: right,
            });
            left = right + CELL_PADDING;
        }
        // The last column takes whatever room is left, and stops at the edge.
        if let Some(last) = self.placed.last_mut() {
            last.right = self.layout.body.right.max(last.left);
        }
    }

    /// Every column in order, as the view has it, including while a drag
    /// changes them.
    pub fn columns(&self) -> &[ColumnSpec] {
        &self.columns
    }

    /// The columns on screen, in order.
    pub fn shown_columns(&self) -> impl Iterator<Item = Column> + '_ {
        self.placed.iter().map(|placed| placed.column)
    }

    /// The shown column whose right edge is under `point` in the header. The
    /// last column fills the grid, so its edge can't be dragged.
    fn edge_at(&self, point: Point) -> Option<usize> {
        if !self.layout.header.contains(point) {
            return None;
        }
        let last = self.placed.len().checked_sub(1)?;
        self.placed[..last]
            .iter()
            .position(|placed| (point.x - (placed.right + CELL_PADDING / 2.0)).abs() <= EDGE_GRAB)
    }

    /// True where a press would resize a column, or while one is resizing,
    /// for the pointer's shape.
    pub fn shows_resize_cursor(&self, point: Point) -> bool {
        matches!(self.drag, Some(Drag::Resize { .. })) || self.edge_at(point).is_some()
    }

    fn spec_index(&self, column: Column) -> Option<usize> {
        self.columns.iter().position(|spec| spec.column == column)
    }

    /// True when the last row is on screen, which is when live tail follows.
    pub fn at_bottom(&self) -> bool {
        self.top >= self.max_top()
    }

    pub fn scroll_to_end(&mut self) {
        self.top = self.max_top();
    }

    pub fn top_row(&self) -> usize {
        self.top as usize
    }

    /// How many whole rows fit, which is what a page moves by.
    pub fn page_rows(&self) -> usize {
        if self.metrics.row_height <= 0.0 {
            return 1;
        }
        ((self.layout.body.height() / self.metrics.row_height) as usize).max(1)
    }

    /// The rows on screen, including a partly visible last row.
    pub fn visible_rows(&self) -> std::ops::Range<usize> {
        if self.metrics.row_height <= 0.0 {
            return 0..0;
        }
        let first = self.top_row();
        let count = (self.layout.body.height() / self.metrics.row_height).ceil() as usize;
        first.min(self.rows)..(first + count).min(self.rows)
    }

    /// The selected rows, in file order.
    pub fn selection(&self) -> Option<std::ops::RangeInclusive<usize>> {
        let (anchor, cursor) = (self.anchor?, self.cursor?);
        Some(anchor.min(cursor)..=anchor.max(cursor))
    }

    pub fn cursor(&self) -> Option<usize> {
        self.cursor
    }

    fn is_selected(&self, row: usize) -> bool {
        self.selection().is_some_and(|range| range.contains(&row))
    }

    fn max_top(&self) -> f64 {
        (self.rows as f64 - self.page_rows() as f64).max(0.0)
    }

    fn clamp_top(&mut self) {
        self.top = self.top.clamp(0.0, self.max_top());
    }

    fn scroll_state(&self) -> ScrollState {
        ScrollState {
            total: self.rows as f64,
            visible: self.page_rows() as f64,
            top: self.top,
        }
    }

    /// Moves to an absolute scroll position.
    pub fn scroll_to(&mut self, top: f64) {
        self.top = top;
        self.clamp_top();
    }

    /// Scrolls just far enough to bring a row on screen.
    fn reveal(&mut self, row: usize) {
        let page = self.page_rows();
        let top = self.top_row();
        if row < top {
            self.top = row as f64;
        } else if row >= top + page {
            self.top = (row + 1 - page) as f64;
        }
        self.clamp_top();
    }

    /// Selects one row, or extends the selection to it, and brings it on screen.
    pub fn select(&mut self, row: usize, extend: bool) {
        if self.rows == 0 {
            return;
        }
        let row = row.min(self.rows - 1);
        if !extend || self.anchor.is_none() {
            self.anchor = Some(row);
        }
        self.cursor = Some(row);
        self.reveal(row);
    }

    fn select_all(&mut self) {
        if self.rows > 0 {
            self.anchor = Some(0);
            self.cursor = Some(self.rows - 1);
        }
    }

    /// Changes the row count before the next frame does, so a row past the old
    /// end can be selected. A merged timeline uses this when it's rebuilt.
    pub fn set_row_count(&mut self, rows: usize) {
        self.rows = rows;
        self.anchor = self.cursor;
    }

    /// Clears the selection and scroll position for a new set of rows. The layout
    /// stays, so a row can be selected again before the next frame arranges it.
    pub fn reset(&mut self, rows: usize) {
        *self = Self {
            layout: self.layout,
            metrics: self.metrics,
            rows,
            columns: std::mem::take(&mut self.columns),
            placed: std::mem::take(&mut self.placed),
            ..Self::default()
        };
    }

    fn move_cursor(&mut self, delta: i64, extend: bool) {
        if self.rows == 0 {
            return;
        }
        let start = self.cursor.unwrap_or_else(|| self.top_row()) as i64;
        let target = (start + delta).clamp(0, self.rows as i64 - 1) as usize;
        self.select(target, extend);
    }

    fn row_at(&self, point: Point) -> Option<usize> {
        if !self.layout.body.contains(point) || self.metrics.row_height <= 0.0 {
            return None;
        }
        let offset = ((point.y - self.layout.body.top) / self.metrics.row_height) as usize;
        let row = self.top_row() + offset;
        (row < self.rows).then_some(row)
    }

    fn handle_key(&mut self, key: Key, ctrl: bool, shift: bool) -> Response {
        let page = self.page_rows() as i64;
        match (key, ctrl) {
            (Key::Up, false) => self.move_cursor(-1, shift),
            (Key::Down, false) => self.move_cursor(1, shift),
            (Key::PageUp, false) => self.move_cursor(-page, shift),
            (Key::PageDown, false) => self.move_cursor(page, shift),
            (Key::Home, _) => self.select(0, shift),
            (Key::End, _) => self.select(self.rows.saturating_sub(1), shift),
            (Key::Up, true) => {
                self.scroll_to(self.top - 1.0);
                return Response::REDRAW;
            }
            (Key::Down, true) => {
                self.scroll_to(self.top + 1.0);
                return Response::REDRAW;
            }
            (Key::Char('A'), true) => self.select_all(),
            (Key::Char('C'), true) => {
                return Response {
                    copy_selection: self.selection().is_some(),
                    ..Response::default()
                };
            }
            (Key::Escape, _) => {
                self.anchor = None;
                self.cursor = None;
            }
            _ => return Response::default(),
        }
        Response::SELECTED
    }

    fn handle_press(&mut self, point: Point, shift: bool) -> Response {
        if self.layout.track.contains(point) {
            let state = self.scroll_state();
            match scrollbar::press_at(&self.layout.track, &state, point) {
                Press::Thumb { grab } => self.drag = Some(Drag::Thumb { grab }),
                Press::Before => self.scroll_to(self.top - self.page_rows() as f64),
                Press::After => self.scroll_to(self.top + self.page_rows() as f64),
            }
            return Response::REDRAW;
        }
        if let Some(index) = self.edge_at(point) {
            let placed = self.placed[index];
            if let Some(column) = self.spec_index(placed.column) {
                self.drag = Some(Drag::Resize {
                    column,
                    left: placed.left,
                });
            }
            return Response::default();
        }
        if self.layout.header.contains(point) {
            let pressed = self
                .placed
                .iter()
                .find(|placed| point.x < placed.right + CELL_PADDING)
                .and_then(|placed| self.spec_index(placed.column));
            if let Some(column) = pressed {
                self.drag = Some(Drag::Move {
                    column,
                    start: point.x,
                    target: None,
                });
            }
            return Response::default();
        }
        if let Some(row) = self.row_at(point) {
            self.select(row, shift);
            self.drag = Some(Drag::Rows);
            return Response::SELECTED;
        }
        Response::default()
    }

    /// Where a dragged header would land: before the first shown column whose
    /// middle is right of the pointer, or after them all.
    fn drop_target(&self, x: f32) -> usize {
        self.placed
            .iter()
            .position(|placed| x < (placed.left + placed.natural_right) / 2.0)
            .unwrap_or(self.placed.len())
    }

    /// Ends a header drag, moving the column if it was dropped somewhere new.
    fn finish_drag(&mut self) -> Response {
        let changed = match self.drag.take() {
            Some(Drag::Resize { .. }) => true,
            Some(Drag::Move {
                column,
                target: Some(target),
                ..
            }) => self.move_column(column, target),
            _ => false,
        };
        Response {
            redraw: changed,
            columns_changed: changed,
            ..Response::default()
        }
    }

    /// Moves a column to just before the shown column at `target`, or after
    /// the last one. Returns false if that's where it already is.
    fn move_column(&mut self, column: usize, target: usize) -> bool {
        let Some(moving) = self.columns.get(column).map(|spec| spec.column) else {
            return false;
        };
        let before = self.placed.get(target).map(|placed| placed.column);
        let after = target
            .checked_sub(1)
            .and_then(|index| self.placed.get(index))
            .map(|placed| placed.column);
        if before == Some(moving) || after == Some(moving) {
            return false;
        }
        let spec = self.columns.remove(column);
        let at = before
            .and_then(|before| self.spec_index(before))
            .unwrap_or(self.columns.len());
        self.columns.insert(at, spec);
        self.place();
        true
    }

    fn handle_drag(&mut self, point: Point) -> Response {
        match self.drag {
            Some(Drag::Thumb { grab }) => {
                let state = self.scroll_state();
                self.top = scrollbar::drag_position(&self.layout.track, &state, point.y, grab);
                self.clamp_top();
                Response::REDRAW
            }
            Some(Drag::Rows) => {
                // Dragging past the top or bottom edge scrolls by a row.
                let row = if point.y < self.layout.body.top {
                    Some(self.top_row().saturating_sub(1))
                } else if point.y >= self.layout.body.bottom {
                    Some((self.top_row() + self.page_rows()).min(self.rows.saturating_sub(1)))
                } else {
                    self.row_at(Point::new(self.layout.body.left, point.y))
                };
                match row {
                    Some(row) if self.cursor != Some(row) => {
                        self.select(row, true);
                        Response::SELECTED
                    }
                    _ => Response::default(),
                }
            }
            Some(Drag::Resize { column, left }) => {
                if self.metrics.char_width > 0.0 {
                    let width =
                        ((point.x - left) / self.metrics.char_width).clamp(MIN_WIDTH, MAX_WIDTH);
                    if let Some(spec) = self.columns.get_mut(column) {
                        spec.width = width;
                    }
                    self.place();
                }
                Response::REDRAW
            }
            Some(Drag::Move { column, start, .. }) => {
                let target =
                    ((point.x - start).abs() > DRAG_START).then(|| self.drop_target(point.x));
                self.drag = Some(Drag::Move {
                    column,
                    start,
                    target,
                });
                Response::REDRAW
            }
            None => Response::default(),
        }
    }

    pub fn handle(&mut self, event: Event) -> Response {
        match event {
            Event::KeyDown(key, modifiers) => {
                if modifiers.alt {
                    return Response::default();
                }
                self.handle_key(key, modifiers.ctrl, modifiers.shift)
            }
            Event::MouseDown(point, MouseButton::Left, modifiers) => {
                self.handle_press(point, modifiers.shift)
            }
            Event::MouseDown(point, MouseButton::Right, _)
                if self.layout.header.contains(point) =>
            {
                Response {
                    columns_menu: true,
                    ..Response::default()
                }
            }
            Event::MouseUp(_, MouseButton::Left) => self.finish_drag(),
            Event::MouseMove(point) => self.handle_drag(point),
            Event::Wheel(rows) => {
                self.scroll_to(self.top - f64::from(rows));
                Response::REDRAW
            }
            _ => Response::default(),
        }
    }

    pub fn row_rect(&self, row: usize) -> Rect {
        let top = self.layout.body.top
            + (row as f64 - self.top_row() as f64) as f32 * self.metrics.row_height;
        Rect::new(
            self.layout.body.left,
            top,
            self.layout.body.right,
            top + self.metrics.row_height,
        )
    }

    pub fn body_rect(&self) -> Rect {
        self.layout.body
    }

    /// Where a highlight rule's marker goes: in the gap between the row number
    /// and the first column.
    pub fn marker_rect(&self, row: usize) -> Rect {
        let rect = self.row_rect(row);
        let gap = self.layout.first_left - self.layout.row_number_right;
        let left = self.layout.row_number_right + (gap - MARKER_WIDTH) / 2.0;
        Rect::new(left, rect.top + 1.0, left + MARKER_WIDTH, rect.bottom - 1.0)
    }

    /// Where a shown column's header is.
    #[cfg(test)]
    pub fn header_rect(&self, column: Column) -> Option<Rect> {
        let header = self.layout.header;
        self.placed
            .iter()
            .find(|placed| placed.column == column)
            .map(|placed| Rect::new(placed.left, header.top, placed.right, header.bottom))
    }

    fn draw_header(&self, canvas: &mut dyn Canvas, theme: &Theme) {
        let layout = &self.layout;
        let header = layout.header;
        canvas.fill_rect(header, theme.header_background);
        let cell = |left: f32, right: f32| Rect::new(left, header.top, right, header.bottom);
        canvas.text(
            "Row",
            cell(header.left, layout.row_number_right),
            GRID_RIGHT,
            theme.primary_text,
        );
        canvas.push_clip(header);
        for placed in &self.placed {
            canvas.text(
                placed.column.title(),
                cell(placed.left, placed.right),
                GRID_LEFT,
                theme.primary_text,
            );
        }
        // Where a dragged header would land.
        if let Some(Drag::Move {
            target: Some(target),
            ..
        }) = self.drag
        {
            let x = match self.placed.get(target) {
                Some(placed) => placed.left - CELL_PADDING / 2.0,
                None => self.placed.last().map_or(layout.first_left, |placed| {
                    placed.natural_right + CELL_PADDING / 2.0
                }),
            };
            canvas.fill_rect(
                Rect::new(x - 1.0, header.top, x + 1.0, header.bottom),
                theme.accent,
            );
        }
        canvas.pop_clip();
        canvas.fill_rect(
            Rect::new(
                header.left,
                header.bottom - 1.0,
                header.right,
                header.bottom,
            ),
            theme.border,
        );
    }

    fn draw_row(&self, canvas: &mut dyn Canvas, source: &dyn RowSource, theme: &Theme, row: usize) {
        let layout = &self.layout;
        let rect = self.row_rect(row);
        let level = source.level(row);
        let cells = source.cells(row);
        let mark = cells.as_ref().and_then(|cells| cells.mark);
        let selected = self.is_selected(row);
        let highlighted = !selected && source.highlighted(row);
        // A rule's row color gives way to the selection and to find matches,
        // which are what someone is looking at right now.
        let rule_row = mark
            .filter(|mark| mark.style == Style::Row)
            .and_then(|mark| theme.rule_colors.get(mark.color));
        let (background, message_color) = match (level, rule_row) {
            _ if selected => (Some(theme.selected_row), theme.selected_text),
            (Level::Critical | Level::Error, _) if highlighted => {
                (Some(theme.highlight_row), theme.error_text)
            }
            _ if highlighted => (Some(theme.highlight_row), theme.primary_text),
            (_, Some(colors)) => (Some(colors.row), colors.text),
            (Level::Critical, None) => (Some(theme.critical_row), theme.critical_text),
            (Level::Error, None) => (Some(theme.error_row), theme.error_text),
            (Level::Warning, None) => (Some(theme.warning_row), theme.warning_text),
            (Level::Notice, None) => (None, theme.notice_text),
            (Level::Debug, None) => (None, theme.debug_text),
            (Level::Info, None) => (None, theme.primary_text),
        };
        if let Some(color) = background {
            canvas.fill_rect(rect, color);
        }
        let marker = mark
            .filter(|mark| mark.style == Style::Marker)
            .and_then(|mark| theme.rule_colors.get(mark.color));
        if let Some(colors) = marker {
            canvas.fill_rect(
                self.marker_rect(row),
                if selected {
                    theme.selected_text
                } else {
                    colors.marker
                },
            );
        }
        if source.bookmarked(row) {
            let marker = Rect::new(
                layout.body.left,
                rect.top + 1.0,
                layout.body.left + BOOKMARK_WIDTH,
                rect.bottom - 1.0,
            );
            canvas.fill_rect(
                marker,
                if selected {
                    theme.selected_text
                } else {
                    theme.bookmark
                },
            );
        }
        // A colored row draws every column in its text color, as CMTrace does,
        // so each one keeps its contrast against the row.
        let themed = |color: Color| {
            if background.is_some() {
                message_color
            } else {
                color
            }
        };
        let cell = |left: f32, right: f32| Rect::new(left, rect.top, right, rect.bottom);

        let number = (source.file_row(row) + 1).to_string();
        canvas.text(
            &number,
            cell(layout.body.left, layout.row_number_right),
            GRID_RIGHT,
            themed(theme.tertiary_text),
        );
        // The level comes from the index, so it draws before the row has loaded.
        for placed in &self.placed {
            let (text, color) = match (placed.column, &cells) {
                (Column::Level, _) => (short_level(level), message_color),
                (Column::Message, Some(cells)) => (cells.message.as_str(), message_color),
                (Column::Message, None) => ("...", themed(theme.placeholder_text)),
                (_, None) => continue,
                (Column::Time, Some(cells)) => (cells.time.as_str(), themed(theme.timestamp_text)),
                (column, Some(cells)) => (column.text(cells), themed(theme.component_text)),
            };
            canvas.text(text, cell(placed.left, placed.right), GRID_LEFT, color);
        }
    }

    pub fn draw(&self, canvas: &mut dyn Canvas, source: &dyn RowSource, theme: &Theme) {
        let layout = &self.layout;
        canvas.fill_rect(layout.body, theme.grid_background);
        canvas.push_clip(layout.body);
        for row in self.visible_rows() {
            self.draw_row(canvas, source, theme, row);
        }
        canvas.pop_clip();
        self.draw_header(canvas, theme);

        canvas.fill_rect(layout.track, theme.scrollbar_track);
        if self.rows > 0 {
            let thumb = scrollbar::thumb_rect(&layout.track, &self.scroll_state());
            canvas.fill_rect(thumb, theme.scrollbar_thumb);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::columns::defaults;
    use crate::gui::rules::Mark;
    use tv_engine::Format;
    use tv_ui::recording::{DrawOp, RecordingCanvas};
    use tv_ui::Modifiers;

    const SHIFT: Modifiers = Modifiers {
        ctrl: false,
        shift: true,
        alt: false,
    };
    const CTRL: Modifiers = Modifiers {
        ctrl: true,
        shift: false,
        alt: false,
    };
    const NONE: Modifiers = Modifiers {
        ctrl: false,
        shift: false,
        alt: false,
    };
    const AREA: Rect = Rect::new(0.0, 0.0, 1000.0, 400.0);
    const METRICS: Metrics = Metrics {
        row_height: 18.0,
        char_width: 7.0,
        header_height: 22.0,
    };

    struct FakeRows {
        count: usize,
        loaded: bool,
    }

    impl RowSource for FakeRows {
        fn row_count(&self) -> usize {
            self.count
        }

        fn level(&self, row: usize) -> Level {
            match row % 4 {
                1 => Level::Error,
                2 => Level::Warning,
                _ => Level::Info,
            }
        }

        fn cells(&self, row: usize) -> Option<Arc<RowCells>> {
            self.loaded.then(|| {
                Arc::new(RowCells {
                    time: "2026-01-01 10:00:00.000".to_string(),
                    component: "App".to_string(),
                    thread: "1".to_string(),
                    source: String::new(),
                    message: format!("message {row}"),
                    mark: None,
                })
            })
        }
    }

    /// A hundred rows that all match one highlight rule. Row 5 is also a find match.
    struct MarkedRows(Mark);

    impl RowSource for MarkedRows {
        fn row_count(&self) -> usize {
            100
        }

        fn level(&self, row: usize) -> Level {
            FakeRows {
                count: 100,
                loaded: true,
            }
            .level(row)
        }

        fn cells(&self, row: usize) -> Option<Arc<RowCells>> {
            Some(Arc::new(RowCells {
                message: format!("message {row}"),
                mark: Some(self.0),
                ..RowCells::default()
            }))
        }

        fn highlighted(&self, row: usize) -> bool {
            row == 5
        }
    }

    fn fill_at(canvas: &RecordingCanvas, rect: Rect) -> Option<Color> {
        canvas.ops.iter().find_map(|op| match op {
            DrawOp::Fill(filled, color) if *filled == rect => Some(*color),
            _ => None,
        })
    }

    fn text_color(canvas: &RecordingCanvas, text: &str) -> Option<Color> {
        canvas.ops.iter().find_map(|op| match op {
            DrawOp::Text(drawn, _, _, color) if drawn == text => Some(*color),
            _ => None,
        })
    }

    fn view(rows: usize) -> LogView {
        let mut view = LogView::default();
        view.arrange(AREA, rows, METRICS, &defaults(Format::Plain));
        view
    }

    fn key(view: &mut LogView, key: Key, modifiers: Modifiers) -> Response {
        view.handle(Event::KeyDown(key, modifiers))
    }

    fn click(view: &mut LogView, x: f32, y: f32, modifiers: Modifiers) {
        view.handle(Event::MouseDown(
            Point::new(x, y),
            MouseButton::Left,
            modifiers,
        ));
        view.handle(Event::MouseUp(Point::new(x, y), MouseButton::Left));
    }

    #[test]
    fn visible_rows_follow_the_scroll_position() {
        let mut view = view(1000);
        assert_eq!(view.visible_rows(), 0..21);
        view.scroll_to(100.0);
        assert_eq!(view.visible_rows(), 100..121);
    }

    #[test]
    fn scrolling_stops_at_both_ends() {
        let mut view = view(1000);
        view.handle(Event::Wheel(50.0));
        assert_eq!(view.top_row(), 0);
        view.scroll_to(1e12);
        assert_eq!(view.top_row(), 1000 - view.page_rows());
        assert_eq!(view.visible_rows().end, 1000);
    }

    #[test]
    fn huge_files_can_scroll_to_their_last_row() {
        let mut view = view(47_815_464);
        view.scroll_to(1e12);
        assert_eq!(view.visible_rows().end, 47_815_464);
        view.handle(Event::Wheel(1.0));
        assert_eq!(view.top_row(), 47_815_464 - view.page_rows() - 1);
    }

    #[test]
    fn small_files_do_not_scroll() {
        let mut view = view(3);
        view.scroll_to(100.0);
        assert_eq!(view.top_row(), 0);
        assert_eq!(view.visible_rows(), 0..3);
    }

    #[test]
    fn wheel_moves_in_the_expected_direction() {
        let mut view = view(1000);
        view.handle(Event::Wheel(-3.0));
        assert_eq!(view.top_row(), 3);
        view.handle(Event::Wheel(1.0));
        assert_eq!(view.top_row(), 2);
    }

    #[test]
    fn arrow_keys_move_the_selection_and_keep_it_on_screen() {
        let mut view = view(1000);
        key(&mut view, Key::Down, NONE);
        assert_eq!(view.cursor(), Some(1));
        for _ in 0..100 {
            key(&mut view, Key::Down, NONE);
        }
        assert_eq!(view.cursor(), Some(101));
        assert!(view.visible_rows().contains(&101));
        key(&mut view, Key::Up, NONE);
        assert_eq!(view.cursor(), Some(100));
    }

    #[test]
    fn paging_moves_by_a_screen_and_stops_at_the_ends() {
        let mut view = view(1000);
        let page = view.page_rows();
        key(&mut view, Key::Down, NONE);
        key(&mut view, Key::PageDown, NONE);
        assert_eq!(view.cursor(), Some(1 + page));
        key(&mut view, Key::End, NONE);
        assert_eq!(view.cursor(), Some(999));
        key(&mut view, Key::PageDown, NONE);
        assert_eq!(view.cursor(), Some(999));
        key(&mut view, Key::Home, NONE);
        assert_eq!(view.cursor(), Some(0));
        assert_eq!(view.top_row(), 0);
    }

    #[test]
    fn shift_extends_the_selection_in_either_direction() {
        let mut view = view(1000);
        view.select(10, false);
        key(&mut view, Key::Down, SHIFT);
        key(&mut view, Key::Down, SHIFT);
        assert_eq!(view.selection(), Some(10..=12));
        for _ in 0..5 {
            key(&mut view, Key::Up, SHIFT);
        }
        assert_eq!(view.selection(), Some(7..=10));
    }

    #[test]
    fn a_plain_move_collapses_the_selection() {
        let mut view = view(1000);
        view.select(10, false);
        view.select(15, true);
        key(&mut view, Key::Down, NONE);
        assert_eq!(view.selection(), Some(16..=16));
    }

    #[test]
    fn clicking_selects_the_row_under_the_mouse() {
        let mut view = view(1000);
        let body_top = view.body_rect().top;
        click(&mut view, 100.0, body_top + 18.0 * 3.0 + 2.0, NONE);
        assert_eq!(view.selection(), Some(3..=3));
        click(&mut view, 100.0, body_top + 18.0 * 6.0 + 2.0, SHIFT);
        assert_eq!(view.selection(), Some(3..=6));
    }

    #[test]
    fn clicking_past_the_last_row_selects_nothing() {
        let mut view = view(2);
        let body_top = view.body_rect().top;
        click(&mut view, 100.0, body_top + 18.0 * 10.0, NONE);
        assert_eq!(view.selection(), None);
    }

    #[test]
    fn dragging_selects_a_range_and_scrolls_at_the_edge() {
        let mut view = view(1000);
        let body = view.body_rect();
        view.handle(Event::MouseDown(
            Point::new(100.0, body.top + 2.0),
            MouseButton::Left,
            NONE,
        ));
        view.handle(Event::MouseMove(Point::new(
            100.0,
            body.top + 18.0 * 4.0 + 2.0,
        )));
        assert_eq!(view.selection(), Some(0..=4));
        view.handle(Event::MouseMove(Point::new(100.0, body.bottom + 50.0)));
        assert_eq!(view.selection(), Some(0..=view.page_rows()));
        assert!(view.top_row() > 0);
        view.handle(Event::MouseUp(Point::new(100.0, 0.0), MouseButton::Left));
        let selection = view.selection();
        view.handle(Event::MouseMove(Point::new(100.0, body.top + 2.0)));
        assert_eq!(view.selection(), selection, "moved after release");
    }

    #[test]
    fn dragging_the_thumb_reaches_the_last_row() {
        let mut view = view(47_815_464);
        let x = AREA.right - 5.0;
        let top = view.body_rect().top;
        view.handle(Event::MouseDown(
            Point::new(x, top + 2.0),
            MouseButton::Left,
            NONE,
        ));
        view.handle(Event::MouseMove(Point::new(x, AREA.bottom + 200.0)));
        assert_eq!(view.visible_rows().end, 47_815_464);
        view.handle(Event::MouseUp(Point::new(x, 0.0), MouseButton::Left));
    }

    #[test]
    fn clicking_the_track_pages() {
        let mut view = view(100_000);
        let x = AREA.right - 5.0;
        click(&mut view, x, AREA.bottom - 5.0, NONE);
        assert_eq!(view.top_row(), view.page_rows());
        assert_eq!(view.selection(), None, "the track must not select rows");
    }

    #[test]
    fn select_all_and_copy() {
        let mut view = view(1000);
        assert!(!key(&mut view, Key::Char('C'), CTRL).copy_selection);
        key(&mut view, Key::Char('A'), CTRL);
        assert_eq!(view.selection(), Some(0..=999));
        assert!(key(&mut view, Key::Char('C'), CTRL).copy_selection);
    }

    #[test]
    fn escape_clears_the_selection() {
        let mut view = view(1000);
        view.select(5, false);
        key(&mut view, Key::Escape, NONE);
        assert_eq!(view.selection(), None);
    }

    #[test]
    fn empty_logs_are_safe() {
        let mut view = view(0);
        for pressed in [Key::Up, Key::Down, Key::PageDown, Key::Home, Key::End] {
            key(&mut view, pressed, NONE);
        }
        click(&mut view, 100.0, 100.0, NONE);
        view.handle(Event::Wheel(-3.0));
        assert_eq!(view.selection(), None);
        assert_eq!(view.visible_rows(), 0..0);
        let mut canvas = RecordingCanvas::new();
        view.draw(
            &mut canvas,
            &FakeRows {
                count: 0,
                loaded: true,
            },
            &Theme::CLASSIC,
        );
        assert!(canvas.clips_balance());
    }

    #[test]
    fn draws_only_the_visible_rows() {
        let mut view = view(47_815_464);
        view.scroll_to(1000.0);
        let mut canvas = RecordingCanvas::new();
        let rows = FakeRows {
            count: 47_815_464,
            loaded: true,
        };
        view.draw(&mut canvas, &rows, &Theme::CLASSIC);
        let texts = canvas.texts();
        assert!(texts.contains(&"message 1000"));
        assert!(texts.contains(&"1001"));
        assert!(!texts.contains(&"message 999"));
        assert!(!texts.contains(&"message 1100"));
        assert!(canvas.clips_balance());
    }

    #[test]
    fn rows_not_loaded_yet_draw_placeholders_with_their_numbers() {
        let view = view(100);
        let mut canvas = RecordingCanvas::new();
        view.draw(
            &mut canvas,
            &FakeRows {
                count: 100,
                loaded: false,
            },
            &Theme::CLASSIC,
        );
        let texts = canvas.texts();
        assert!(texts.contains(&"..."));
        assert!(texts.contains(&"1"));
    }

    #[test]
    fn rows_are_colored_by_severity() {
        let view = view(100);
        let mut canvas = RecordingCanvas::new();
        let theme = Theme::CLASSIC;
        view.draw(
            &mut canvas,
            &FakeRows {
                count: 100,
                loaded: true,
            },
            &theme,
        );
        let fills: Vec<Color> = canvas
            .ops
            .iter()
            .filter_map(|op| match op {
                DrawOp::Fill(_, color) => Some(*color),
                _ => None,
            })
            .collect();
        assert!(fills.contains(&theme.error_row));
        assert!(fills.contains(&theme.warning_row));
    }

    #[test]
    fn selection_wins_over_severity_color() {
        let mut view = view(100);
        view.select(1, false);
        let theme = Theme::CLASSIC;
        let mut canvas = RecordingCanvas::new();
        view.draw(
            &mut canvas,
            &FakeRows {
                count: 100,
                loaded: true,
            },
            &theme,
        );
        let row_rect = view.row_rect(1);
        let row_fill = canvas.ops.iter().find_map(|op| match op {
            DrawOp::Fill(rect, color) if *rect == row_rect => Some(*color),
            _ => None,
        });
        assert_eq!(row_fill, Some(theme.selected_row));
    }

    #[test]
    fn rule_rows_take_their_color_but_selection_and_find_win() {
        let mut view = view(100);
        view.select(3, false);
        let theme = Theme::CLASSIC;
        let blue = theme.rule_colors[4];
        let mut canvas = RecordingCanvas::new();
        let rows = MarkedRows(Mark {
            color: 4,
            style: Style::Row,
        });
        view.draw(&mut canvas, &rows, &theme);
        assert_eq!(fill_at(&canvas, view.row_rect(0)), Some(blue.row));
        assert_eq!(text_color(&canvas, "message 0"), Some(blue.text));
        // Row 1 is an error, and the rule's color replaces the severity's.
        assert_eq!(fill_at(&canvas, view.row_rect(1)), Some(blue.row));
        assert_eq!(fill_at(&canvas, view.row_rect(3)), Some(theme.selected_row));
        assert_eq!(
            fill_at(&canvas, view.row_rect(5)),
            Some(theme.highlight_row)
        );
    }

    #[test]
    fn markers_sit_by_the_number_and_keep_the_severity_color() {
        let mut view = view(100);
        view.select(2, false);
        let theme = Theme::CLASSIC;
        let red = theme.rule_colors[0];
        let mut canvas = RecordingCanvas::new();
        let rows = MarkedRows(Mark {
            color: 0,
            style: Style::Marker,
        });
        view.draw(&mut canvas, &rows, &theme);
        assert_eq!(fill_at(&canvas, view.row_rect(1)), Some(theme.error_row));
        let markers: Vec<(Rect, Color)> = canvas
            .ops
            .iter()
            .filter_map(|op| match op {
                DrawOp::Fill(rect, color) if rect.width() == MARKER_WIDTH => Some((*rect, *color)),
                _ => None,
            })
            .collect();
        assert_eq!(markers.len(), view.visible_rows().len());
        let (first, color) = markers[0];
        assert!(first.left > view.layout.row_number_right);
        assert!(first.right < view.layout.first_left);
        assert_eq!(color, red.marker);
        // On the selected row it takes the selection's text color, as bookmarks do.
        assert_eq!(markers[2].1, theme.selected_text);
    }

    #[test]
    fn row_number_column_grows_for_huge_files() {
        let small = view(100);
        let large = view(47_815_464);
        assert!(large.layout.first_left > small.layout.first_left);
    }

    fn order(view: &LogView) -> Vec<Column> {
        view.shown_columns().collect()
    }

    fn press(view: &mut LogView, x: f32) -> Response {
        view.handle(Event::MouseDown(
            Point::new(x, 10.0),
            MouseButton::Left,
            NONE,
        ))
    }

    fn move_to(view: &mut LogView, x: f32) {
        view.handle(Event::MouseMove(Point::new(x, 10.0)));
    }

    fn release(view: &mut LogView, x: f32) -> Response {
        view.handle(Event::MouseUp(Point::new(x, 10.0), MouseButton::Left))
    }

    #[test]
    fn shown_columns_draw_in_order_and_the_last_fills_the_grid() {
        let mut view = view(100);
        view.arrange(AREA, 100, METRICS, &defaults(Format::Sccm));
        let mut canvas = RecordingCanvas::new();
        let rows = FakeRows {
            count: 100,
            loaded: true,
        };
        view.draw(&mut canvas, &rows, &Theme::CLASSIC);
        let texts = canvas.texts();
        assert!(texts.contains(&"Thread") && !texts.contains(&"Source"));
        assert_eq!(
            order(&view),
            [
                Column::Time,
                Column::Level,
                Column::Component,
                Column::Thread,
                Column::Message
            ]
        );
        let message = view.header_rect(Column::Message).unwrap();
        assert_eq!(message.right, view.body_rect().right);
    }

    #[test]
    fn dragging_a_header_edge_resizes_the_column() {
        let mut view = view(100);
        let time = view.header_rect(Column::Time).unwrap();
        let edge = time.right + CELL_PADDING / 2.0;
        assert!(view.shows_resize_cursor(Point::new(edge, 10.0)));
        assert!(!view.shows_resize_cursor(Point::new(time.left + 20.0, 10.0)));
        assert_eq!(press(&mut view, edge), Response::default());
        move_to(&mut view, time.left + 140.0);
        // The owner's columns don't replace the view's during the drag.
        view.arrange(AREA, 100, METRICS, &defaults(Format::Plain));
        assert_eq!(view.columns()[0].width, 20.0);
        assert!(release(&mut view, time.left + 140.0).columns_changed);
        assert_eq!(
            view.header_rect(Column::Level).unwrap().left,
            time.left + 148.0
        );
        // It never shrinks past its minimum.
        let edge = view.header_rect(Column::Time).unwrap().right + CELL_PADDING / 2.0;
        press(&mut view, edge);
        move_to(&mut view, 0.0);
        release(&mut view, 0.0);
        assert_eq!(view.columns()[0].width, MIN_WIDTH);
    }

    #[test]
    fn dragging_a_header_moves_the_column_and_a_click_doesnt() {
        let mut view = view(100);
        let component = view.header_rect(Column::Component).unwrap();
        press(&mut view, component.left + 10.0);
        move_to(&mut view, 70.0);
        let response = release(&mut view, 70.0);
        assert!(response.columns_changed);
        assert_eq!(
            order(&view),
            [
                Column::Component,
                Column::Time,
                Column::Level,
                Column::Message
            ]
        );
        // Dropped after everything, it goes last.
        let time = view.header_rect(Column::Time).unwrap();
        press(&mut view, time.left + 10.0);
        move_to(&mut view, 990.0);
        assert!(release(&mut view, 990.0).columns_changed);
        assert_eq!(
            order(&view),
            [
                Column::Component,
                Column::Level,
                Column::Message,
                Column::Time
            ]
        );
        // A click, or a drop back where it was, changes nothing.
        let level = view.header_rect(Column::Level).unwrap();
        press(&mut view, level.left + 5.0);
        assert!(!release(&mut view, level.left + 5.0).columns_changed);
        press(&mut view, level.left + 5.0);
        move_to(&mut view, level.left + 12.0);
        assert!(!release(&mut view, level.left + 12.0).columns_changed);
        assert_eq!(view.columns().len(), 6, "hidden columns are kept");
    }

    #[test]
    fn right_clicking_the_header_asks_for_the_columns_editor() {
        let mut view = view(100);
        let header = Event::MouseDown(Point::new(300.0, 10.0), MouseButton::Right, NONE);
        assert!(view.handle(header).columns_menu);
        let row = Event::MouseDown(Point::new(300.0, 100.0), MouseButton::Right, NONE);
        assert!(!view.handle(row).columns_menu);
    }

    #[test]
    fn snapping_lands_on_whole_physical_pixels() {
        let snapped = snap(18.0, 1.25);
        assert_eq!(snapped * 1.25, (snapped * 1.25).round());
        assert_eq!(snap(18.0, 1.0), 18.0);
    }
}
