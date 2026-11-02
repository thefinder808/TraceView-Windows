//! The details pane under the grid, as in CMTrace: the selected row's fields and
//! its whole message, which the grid cuts to one line. The row is read on a
//! background thread like every other read.

use super::log_view::level_name;
use super::row_cache::SharedLog;
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use tv_engine::{encoding, Level};
use tv_ui::{Align, Canvas, FontRole, Rect, TextStyle, Theme};

pub const PANE_HEIGHT: f32 = 150.0;
/// Longer messages are cut here. The full row is always in the file.
const MAX_MESSAGE_CHARS: usize = 64 * 1024;
const PADDING: f32 = 10.0;
const LINE_HEIGHT: f32 = 20.0;
const LABEL: TextStyle = TextStyle::new(FontRole::Ui, Align::Left);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RowDetail {
    pub row: usize,
    pub time: String,
    pub level: Option<Level>,
    pub component: String,
    pub thread: String,
    pub source: String,
    pub message: String,
    /// What Windows says the error codes in the message mean.
    pub codes: Vec<String>,
    /// The file the row is from, shown in merged tabs.
    pub file: Option<String>,
}

pub fn read_detail(log: &tv_engine::LogFile, row: usize) -> Option<RowDetail> {
    let (mut buffer, mut scratch) = (Vec::new(), Vec::new());
    let mut detail = None;
    log.read_rows(row..row + 1, &mut buffer, |_, raw, _| {
        let fields = log.fields(log.text(raw, &mut scratch));
        let message = encoding::display(fields.message);
        detail = Some(RowDetail {
            row,
            time: fields
                .stamp
                .map(|stamp| stamp.to_string().replace('T', " "))
                .unwrap_or_default(),
            level: Some(log.level(row)),
            component: encoding::display(fields.component).into_owned(),
            thread: encoding::display(fields.thread).into_owned(),
            source: encoding::display(fields.source).into_owned(),
            message: message
                .chars()
                .take(MAX_MESSAGE_CHARS)
                .collect::<String>()
                .replace("\r\n", "\n"),
            codes: Vec::new(),
            file: None,
        });
    })
    .ok()?;
    detail
}

/// The row to show: which file it's in, and where.
pub struct DetailTarget {
    pub log: SharedLog,
    pub source: usize,
    pub row: usize,
    pub file: Option<String>,
}

#[derive(Default)]
pub struct Details {
    /// The file and row asked for last.
    requested: Option<(usize, usize)>,
    loading: Option<Receiver<Option<RowDetail>>>,
    shown: Option<RowDetail>,
}

impl Details {
    /// Asks for a row's details. Asking again for the same row does nothing.
    pub fn request(&mut self, target: Option<DetailTarget>, wake: impl Fn() + Send + 'static) {
        let key = target.as_ref().map(|target| (target.source, target.row));
        if key == self.requested {
            return;
        }
        self.requested = key;
        let Some(DetailTarget { log, row, file, .. }) = target else {
            self.shown = None;
            self.loading = None;
            return;
        };
        let (sender, receiver) = channel();
        std::thread::spawn(move || {
            let detail =
                log.read()
                    .ok()
                    .and_then(|log| read_detail(&log, row))
                    .map(|mut detail| {
                        // After the read lock is released, as Windows may load message tables.
                        detail.codes = super::lookup::code_lines(&detail.message);
                        detail.file = file;
                        detail
                    });
            let _ = sender.send(detail);
            wake();
        });
        self.loading = Some(receiver);
    }

    /// Forgets the shown row, so it's read again, for example after live tail
    /// added to it.
    pub fn refresh(&mut self) {
        self.requested = None;
    }

    #[cfg(test)]
    pub fn poll(&mut self) {
        self.poll_changed();
    }

    /// Picks up a finished read. Returns true if the pane changed.
    pub fn poll_changed(&mut self) -> bool {
        let Some(loading) = &self.loading else {
            return false;
        };
        match loading.try_recv() {
            Ok(detail) => {
                self.shown = detail;
                self.loading = None;
                true
            }
            Err(TryRecvError::Empty) => false,
            Err(TryRecvError::Disconnected) => {
                self.loading = None;
                false
            }
        }
    }

    pub fn shown(&self) -> Option<&RowDetail> {
        self.shown.as_ref()
    }

    pub fn draw(&self, canvas: &mut dyn Canvas, area: Rect, theme: &Theme) {
        canvas.fill_rect(area, theme.panel_background);
        canvas.fill_rect(
            Rect::new(area.left, area.top, area.right, area.top + 1.0),
            theme.border,
        );
        let inner = Rect::new(
            area.left + PADDING,
            area.top + 6.0,
            area.right - PADDING,
            area.bottom - 4.0,
        );
        canvas.push_clip(inner);
        let Some(detail) = &self.shown else {
            canvas.text(
                "Select a row to see its details",
                Rect::new(inner.left, inner.top, inner.right, inner.top + LINE_HEIGHT),
                LABEL,
                theme.secondary_text,
            );
            canvas.pop_clip();
            return;
        };
        let mut summary = vec![format!("Row {}", detail.row + 1)];
        if let Some(file) = &detail.file {
            summary.insert(0, file.clone());
        }
        if let Some(level) = detail.level {
            summary.push(level_name(level).to_string());
        }
        for (label, value) in [
            ("", &detail.time),
            ("Component ", &detail.component),
            ("Thread ", &detail.thread),
            ("Source ", &detail.source),
        ] {
            if !value.is_empty() {
                summary.push(format!("{label}{value}"));
            }
        }
        canvas.text(
            &summary.join("     "),
            Rect::new(inner.left, inner.top, inner.right, inner.top + LINE_HEIGHT),
            LABEL,
            theme.secondary_text,
        );
        let mut top = inner.top + LINE_HEIGHT + 2.0;
        for line in &detail.codes {
            canvas.text(
                line,
                Rect::new(inner.left, top, inner.right, top + LINE_HEIGHT),
                LABEL,
                theme.accent,
            );
            top += LINE_HEIGHT;
        }
        canvas.text_block(
            &detail.message,
            Rect::new(inner.left, top + 2.0, inner.right, inner.bottom),
            FontRole::Grid,
            theme.primary_text,
        );
        canvas.pop_clip();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;
    use std::sync::{Arc, RwLock};
    use std::time::{Duration, Instant};
    use tv_engine::LogFile;
    use tv_ui::recording::RecordingCanvas;

    fn open(text: &str) -> (tempfile::TempDir, SharedLog) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.log");
        std::fs::write(&path, text).unwrap();
        let (log, _) = LogFile::open_with_cache(&path, None, &AtomicU64::new(0)).unwrap();
        (dir, Arc::new(RwLock::new(log)))
    }

    fn wait(details: &mut Details) -> RowDetail {
        let started = Instant::now();
        loop {
            details.poll();
            if let Some(shown) = details.shown() {
                return shown.clone();
            }
            assert!(started.elapsed() < Duration::from_secs(5), "never loaded");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn shows_every_field_and_the_whole_multi_line_message() {
        let (_dir, log) = open(
            "<![LOG[first line\r\nsecond line]LOG]!><time=\"10:23:01.442+000\" date=\"04-06-2026\" component=\"PolicyAgent\" context=\"\" type=\"3\" thread=\"5240\" file=\"policy.cpp\">\r\n",
        );
        let mut details = Details::default();
        details.request(
            Some(DetailTarget {
                log: log.clone(),
                source: 0,
                row: 0,
                file: None,
            }),
            || {},
        );
        let shown = wait(&mut details);
        assert_eq!(shown.message, "first line\nsecond line");
        assert_eq!(shown.component, "PolicyAgent");
        assert_eq!(shown.thread, "5240");
        assert_eq!(shown.source, "policy.cpp");
        assert_eq!(shown.level, Some(Level::Error));
        assert_eq!(shown.time, "2026-04-06 10:23:01.442");
    }

    #[test]
    fn clearing_the_selection_clears_the_pane() {
        let (_dir, log) = open("a\n");
        let mut details = Details::default();
        details.request(
            Some(DetailTarget {
                log: log.clone(),
                source: 0,
                row: 0,
                file: None,
            }),
            || {},
        );
        wait(&mut details);
        details.request(None, || {});
        assert!(details.shown().is_none());
    }

    #[test]
    fn draws_a_prompt_with_no_selection_and_balances_clips() {
        let mut canvas = RecordingCanvas::new();
        Details::default().draw(
            &mut canvas,
            Rect::new(0.0, 0.0, 800.0, PANE_HEIGHT),
            &Theme::CLASSIC,
        );
        assert!(canvas.texts()[0].contains("Select a row"));
        assert!(canvas.clips_balance());
    }
}
