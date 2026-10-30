//! Virtual log grid. It paints only the visible rows and tracks its own scroll
//! position as an `f64`, because egui's `f32` pixel offsets lose accuracy
//! long before the end of a 50-million-row file.

use crate::theme::Theme;
use eframe::egui::{self, Align2, FontFamily, FontId, Key, Pos2, Rect, Sense, Vec2};
use std::borrow::Cow;
use tv_engine::{parse, LogFile};

const SCROLLBAR_WIDTH: f32 = 14.0;
const MIN_THUMB_HEIGHT: f32 = 28.0;
/// Rows can be megabytes long, and only the visible start ever needs layout.
const MAX_DISPLAY_BYTES: usize = 1024;

#[derive(Default)]
pub struct Grid {
    pub top: f64,
    pub selected: Option<usize>,
    pub visible_rows: f64,
}

impl Grid {
    pub fn max_top(&self, rows: usize) -> f64 {
        (rows as f64 - self.visible_rows.floor()).max(0.0)
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        log: &LogFile,
        view: Option<&[u32]>,
        theme: &Theme,
        font_size: f32,
    ) {
        let rows = view.map_or(log.rows(), <[u32]>::len);
        let font = FontId::new(font_size, FontFamily::Monospace);
        let row_height = (font_size * 1.4).round();
        let rect = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(rect, Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, theme.background);

        let scale = font_size / 13.0;
        let line_right = rect.left() + 88.0 * scale;
        let time_left = line_right + 12.0;
        let component_left = time_left + 172.0 * scale;
        let message_left = component_left + 180.0 * scale;

        let header = Rect::from_min_size(rect.min, Vec2::new(rect.width(), row_height + 4.0));
        let body = Rect::from_min_max(
            Pos2::new(rect.left(), header.bottom()),
            Pos2::new(rect.right() - SCROLLBAR_WIDTH, rect.bottom()),
        );
        self.visible_rows = f64::from(body.height() / row_height);

        self.handle_input(ui, &response, rows, row_height);
        self.scrollbar(ui, &response, rect, header.bottom(), rows, theme);

        // Whole physical pixels keep hinted text on the grid while scrolling.
        let pixels_per_point = ui.ctx().pixels_per_point();
        let first = self.top.floor() as usize;
        let scroll_offset = (self.top - self.top.floor()) as f32 * row_height;
        let row_count = (body.height() / row_height).ceil() as usize + 1;
        let clipped = painter.with_clip_rect(body);

        for slot in 0..row_count {
            let position = first + slot;
            if position >= rows {
                break;
            }
            let row = view.map_or(position, |view| view[position] as usize);
            let y = body.top() + slot as f32 * row_height - scroll_offset;
            let y = (y * pixels_per_point).round() / pixels_per_point;
            let row_rect = Rect::from_min_size(
                Pos2::new(body.left(), y),
                Vec2::new(body.width(), row_height),
            );

            let background = if self.selected == Some(row) {
                Some(theme.selected)
            } else {
                theme.row_background(log.level(row))
            };
            if let Some(color) = background {
                clipped.rect_filled(row_rect, 0.0, color);
            }
            if response.clicked()
                && response
                    .interact_pointer_pos()
                    .is_some_and(|pointer| row_rect.contains(pointer))
            {
                self.selected = Some(row);
            }

            let fields = parse::fields(log.format(), log.row_bytes(row));
            let text_y = y + ((row_height - font_size) / 2.0 - 1.0).round();
            let cell = |left: f32, right: f32, text: String, color| {
                let galley = ui.ctx().fonts_mut(|fonts| fonts.layout_no_wrap(text, font.clone(), color));
                let cell_rect = Rect::from_min_max(
                    Pos2::new(left, row_rect.top()),
                    Pos2::new(right.min(body.right()), row_rect.bottom()),
                );
                clipped
                    .with_clip_rect(cell_rect)
                    .galley(Pos2::new(left, text_y), galley, color);
            };

            let line_text = format!("{}", row + 1);
            clipped.with_clip_rect(row_rect).text(
                Pos2::new(line_right, text_y),
                Align2::RIGHT_TOP,
                line_text,
                font.clone(),
                theme.muted,
            );
            let time = if fields.date.is_empty() {
                display_text(fields.time).into_owned()
            } else {
                format!("{} {}", display_text(fields.date), display_text(fields.time))
            };
            cell(time_left, component_left - 8.0, time, theme.muted);
            cell(
                component_left,
                message_left - 8.0,
                display_text(fields.component).into_owned(),
                theme.muted,
            );
            cell(
                message_left,
                body.right(),
                display_text(fields.message).into_owned(),
                theme.text,
            );
        }

        painter.rect_filled(header, 0.0, theme.header);
        for (x, title, anchor) in [
            (line_right, "Line", Align2::RIGHT_TOP),
            (time_left, "Time", Align2::LEFT_TOP),
            (component_left, "Component", Align2::LEFT_TOP),
            (message_left, "Message", Align2::LEFT_TOP),
        ] {
            painter.text(
                Pos2::new(x, header.top() + 3.0),
                anchor,
                title,
                font.clone(),
                theme.text,
            );
        }
    }

    fn handle_input(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        rows: usize,
        row_height: f32,
    ) {
        if response.hovered() {
            let wheel = ui.input(|input| input.smooth_scroll_delta.y);
            self.top -= f64::from(wheel / row_height);
        }
        if !ui.ctx().egui_wants_keyboard_input() {
            let page = self.visible_rows.floor().max(1.0);
            ui.input(|input| {
                for (key, delta) in [
                    (Key::ArrowDown, 1.0),
                    (Key::ArrowUp, -1.0),
                    (Key::PageDown, page),
                    (Key::PageUp, -page),
                ] {
                    if input.key_pressed(key) {
                        self.top += delta;
                    }
                }
                if input.key_pressed(Key::Home) {
                    self.top = 0.0;
                }
                if input.key_pressed(Key::End) {
                    self.top = f64::MAX;
                }
            });
        }
        self.top = self.top.clamp(0.0, self.max_top(rows));
    }

    fn scrollbar(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        rect: Rect,
        top: f32,
        rows: usize,
        theme: &Theme,
    ) {
        let track = Rect::from_min_max(
            Pos2::new(rect.right() - SCROLLBAR_WIDTH, top),
            rect.right_bottom(),
        );
        let painter = ui.painter_at(rect);
        painter.rect_filled(track, 0.0, theme.scrollbar);
        if rows == 0 {
            return;
        }
        let share = (self.visible_rows / rows as f64).min(1.0) as f32;
        let thumb_height = (track.height() * share).max(MIN_THUMB_HEIGHT);
        let travel = (track.height() - thumb_height).max(1.0);
        let max_top = self.max_top(rows);

        // Row accuracy needs f64 here: an f32 ratio can't land on every row of a huge file.
        let pressed_on_track = ui.input(|input| {
            input
                .pointer
                .press_origin()
                .is_some_and(|origin| track.contains(origin))
        });
        if pressed_on_track && response.is_pointer_button_down_on() {
            if let Some(pointer) = response.interact_pointer_pos() {
                let ratio =
                    f64::from(pointer.y - track.top() - thumb_height / 2.0) / f64::from(travel);
                self.top = (ratio.clamp(0.0, 1.0) * max_top).floor();
            }
        }

        let position = if max_top > 0.0 { (self.top / max_top) as f32 } else { 0.0 };
        let thumb = Rect::from_min_size(
            Pos2::new(track.left() + 2.0, track.top() + position * travel),
            Vec2::new(SCROLLBAR_WIDTH - 4.0, thumb_height),
        );
        painter.rect_filled(thumb, 3.0, theme.thumb);
    }
}

/// First line only, capped, with invalid UTF-8 replaced rather than rejected.
fn display_text(bytes: &[u8]) -> Cow<'_, str> {
    let capped = &bytes[..bytes.len().min(MAX_DISPLAY_BYTES)];
    let line_end = capped
        .iter()
        .position(|byte| matches!(byte, b'\n' | b'\r'))
        .unwrap_or(capped.len());
    String::from_utf8_lossy(&capped[..line_end])
}
