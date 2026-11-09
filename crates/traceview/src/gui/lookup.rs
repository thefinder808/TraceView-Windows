//! Error Lookup, as in CMTrace: type or paste a code, or a whole log line, and
//! read what Windows says it means. The details pane also explains the codes in
//! the selected row. Windows may load a DLL's message table from disk to answer,
//! so lookups run on background threads.

use std::sync::mpsc::{channel, Receiver, TryRecvError};
use tv_errors::{find_codes, lookup, lookup_all, CodeKind, Decoded};
use tv_platform::Waker;
use tv_ui::text_field::TextField;
use tv_ui::{Align, Canvas, FontRole, Point, Rect, TextStyle, Theme};

const WIDTH: f32 = 580.0;
const HEIGHT: f32 = 270.0;
/// How far below the top of the grid the panel sits.
const OFFSET: f32 = 24.0;
const PADDING: f32 = 14.0;
const FIELD_HEIGHT: f32 = 26.0;
const LINE_HEIGHT: f32 = 22.0;
/// The details pane has room for this many explained codes above the message.
const MOST_CODE_LINES: usize = 2;
const LABEL: TextStyle = TextStyle::new(FontRole::Ui, Align::Left);
const TITLE: &str = "Error lookup";
const HINT: &str = "Enter looks up     Ctrl+Shift+C copies the result     Esc closes";
const PLACEHOLDER: &str = "A code or a log line: 0x80070005, -2147024891, 5";

/// Names a reading the way people search for it: Win32 codes in decimal, the
/// rest in hex with the signed form logs often print.
fn heading(decoded: &Decoded) -> String {
    match decoded.kind {
        CodeKind::Win32 | CodeKind::WinHttp => {
            format!("{} {} ({})", decoded.kind.name(), decoded.code, decoded.hex)
        }
        _ => {
            let mut heading = format!(
                "{} {} ({})",
                decoded.kind.name(),
                decoded.hex,
                decoded.signed
            );
            if let Some(facility) = &decoded.facility {
                heading.push_str(&format!(", facility {facility}"));
            }
            heading
        }
    }
}

/// Explains every code in `text`, for the lookup panel.
pub fn explain(text: &str) -> String {
    let readings = lookup_all(text);
    if readings.is_empty() {
        return "No error code found. Try a value such as 0x80070005, -2147024891 or 5."
            .to_string();
    }
    readings
        .iter()
        .map(|decoded| {
            let message = match (decoded.message.as_str(), decoded.name) {
                ("", Some(name)) => {
                    format!("{name}. Windows has no text for this code, so this is its name.")
                }
                ("", None) => "Windows has no text for this code.".to_string(),
                // A Windows Update code with CMTrace's text keeps its name,
                // which is what people search for.
                (message, Some(name)) => format!("{name}: {message}"),
                (message, None) => message.to_string(),
            };
            format!("{}\n{message}", heading(decoded))
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// One line for each code in a log message that Windows can explain, for the
/// details pane.
pub fn code_lines(message: &str) -> Vec<String> {
    find_codes(message)
        .iter()
        .filter_map(|found| {
            let decoded = lookup(&found.text).into_iter().next()?;
            let meaning = match decoded.name {
                Some(name) if decoded.message.is_empty() => name,
                _ => &decoded.message,
            };
            Some(format!("{}: {meaning}", found.text))
        })
        .take(MOST_CODE_LINES)
        .collect()
}

/// The first code in a message, to start a lookup with.
pub fn first_code(message: &str) -> Option<String> {
    find_codes(message)
        .into_iter()
        .next()
        .map(|found| found.text)
}

/// The panel opened by Tools > Error lookup. It floats over the grid and takes
/// all input until it closes.
#[derive(Default)]
pub struct LookupPanel {
    pub field: TextField,
    result: String,
    pending: Option<Receiver<String>>,
    rect: Rect,
    field_rect: Rect,
}

impl LookupPanel {
    /// Opens the panel, looking `text` up at once if there is any.
    pub fn new(text: &str, waker: &Waker) -> Self {
        let mut panel = Self::default();
        if !text.is_empty() {
            panel.field.set_text(text);
            panel.field.select_all();
            panel.look_up(waker);
        }
        panel
    }

    pub fn look_up(&mut self, waker: &Waker) {
        let text = self.field.text().trim().to_string();
        if text.is_empty() {
            self.result.clear();
            self.pending = None;
            return;
        }
        let (sender, receiver) = channel();
        let waker = waker.clone();
        std::thread::spawn(move || {
            let _ = sender.send(explain(&text));
            waker.wake();
        });
        self.pending = Some(receiver);
    }

    /// Picks up a finished lookup. Returns true if the result changed.
    pub fn poll(&mut self) -> bool {
        let Some(pending) = &self.pending else {
            return false;
        };
        match pending.try_recv() {
            Ok(result) => {
                self.result = result;
                self.pending = None;
                true
            }
            Err(TryRecvError::Empty) => false,
            Err(TryRecvError::Disconnected) => {
                self.pending = None;
                false
            }
        }
    }

    pub fn result(&self) -> &str {
        &self.result
    }

    pub fn rect(&self) -> Rect {
        self.rect
    }

    pub fn field_rect(&self) -> Rect {
        self.field_rect
    }

    /// Centres the panel near the top of `area`.
    pub fn arrange(&mut self, area: Rect) {
        let width = WIDTH.min(area.width() - 2.0 * PADDING).max(0.0);
        let left = area.left + (area.width() - width) / 2.0;
        let top = area.top + OFFSET;
        self.rect = Rect::new(
            left,
            top,
            left + width,
            (top + HEIGHT).min(area.bottom.max(top)),
        );
        let field_top = top + PADDING + LINE_HEIGHT;
        self.field_rect = Rect::new(
            left + PADDING,
            field_top,
            left + width - PADDING,
            field_top + FIELD_HEIGHT,
        );
    }

    /// Draws the panel and returns where its caret is, for the IME.
    pub fn draw(&mut self, canvas: &mut dyn Canvas, theme: &Theme) -> Point {
        let rect = self.rect;
        canvas.fill_rect(
            Rect::new(
                rect.left - 1.0,
                rect.top - 1.0,
                rect.right + 1.0,
                rect.bottom + 1.0,
            ),
            theme.border,
        );
        canvas.fill_rect(rect, theme.panel_background);
        canvas.push_clip(rect);
        let inner = Rect::new(
            rect.left + PADDING,
            rect.top + PADDING,
            rect.right - PADDING,
            rect.bottom - PADDING,
        );
        canvas.text(
            TITLE,
            Rect::new(
                inner.left,
                inner.top - 4.0,
                inner.right,
                inner.top + LINE_HEIGHT - 4.0,
            ),
            LABEL,
            theme.primary_text,
        );
        let caret = self
            .field
            .draw(canvas, self.field_rect, theme, true, PLACEHOLDER);
        let text = if self.pending.is_some() && self.result.is_empty() {
            "Looking up..."
        } else {
            &self.result
        };
        let result = Rect::new(
            inner.left,
            self.field_rect.bottom + 10.0,
            inner.right,
            inner.bottom - LINE_HEIGHT,
        );
        canvas.text_block(text, result, FontRole::Ui, theme.primary_text);
        canvas.text(
            HINT,
            Rect::new(
                inner.left,
                inner.bottom - LINE_HEIGHT,
                inner.right,
                inner.bottom,
            ),
            LABEL,
            theme.secondary_text,
        );
        canvas.pop_clip();
        caret
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use tv_ui::recording::RecordingCanvas;

    // Windows words its messages in the user's language, so these check the
    // parts TraceView writes, never Windows' own text.

    #[test]
    fn explanations_name_each_reading() {
        let text = explain("0x80070005");
        assert!(
            text.starts_with("HRESULT 0x80070005 (-2147024891), facility WIN32 (7)\n"),
            "{text}"
        );
        assert!(text.contains("\n\nWin32 5 (0x00000005)\n"), "{text}");
    }

    #[test]
    fn codes_without_text_still_show_their_forms() {
        let text = explain("0x80FFFFFF");
        assert_eq!(
            text,
            "HRESULT 0x80FFFFFF (-2130706433), facility 255\nWindows has no text for this code."
        );
    }

    #[test]
    fn text_without_a_code_says_so() {
        assert!(explain("hello").starts_with("No error code found"));
    }

    #[test]
    fn details_explain_the_codes_in_a_message() {
        let lines =
            code_lines("Download failed, hr=0x80070005, error 5 and 0x80070002 and 0x8007000E");
        assert_eq!(lines.len(), MOST_CODE_LINES);
        assert!(lines[0].starts_with("0x80070005: "), "{lines:?}");
        assert!(lines[0].len() > "0x80070005: ".len());
        assert!(code_lines("Retrying in 15 minutes").is_empty());
    }

    #[test]
    fn the_first_code_starts_a_lookup() {
        assert_eq!(
            first_code("policy failed - error 0x80004005").as_deref(),
            Some("0x80004005")
        );
        assert_eq!(first_code("all good"), None);
    }

    #[test]
    fn the_panel_looks_up_what_it_opens_with() {
        let mut panel = LookupPanel::new("5", &Waker::detached());
        let started = Instant::now();
        while !panel.poll() {
            assert!(started.elapsed() < Duration::from_secs(5), "never answered");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(panel.result().starts_with("Win32 5 (0x00000005)\n"));
        assert_eq!(panel.field.text(), "5");
    }

    #[test]
    fn the_panel_fits_small_windows_and_balances_clips() {
        let mut panel = LookupPanel::default();
        panel.arrange(Rect::new(0.0, 100.0, 300.0, 200.0));
        assert!(panel.rect().width() <= 300.0 && panel.rect().bottom <= 200.0);
        let mut canvas = RecordingCanvas::new();
        panel.draw(&mut canvas, &Theme::CLASSIC);
        assert!(canvas.clips_balance());
        assert!(canvas.texts().contains(&TITLE));
    }
}
