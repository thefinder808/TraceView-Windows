//! Connects the window, renderer and log view. Slow work runs on other threads,
//! which wake the window when something's ready, so the UI thread never waits.

use super::accessibility;
#[cfg(feature = "bench")]
use super::bench::{Bench, Options as BenchOptions};
use super::log_view::{level_name, snap, LogView, Metrics, RowSource};
use super::row_cache::{cells_from_row, RowCache, RowCells};
use accesskit::{Action, ActionRequest, TreeUpdate};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::Duration;
use tv_engine::{CacheUse, Encoding, Format, Level, LogFile, OpenStats};
use tv_platform::{Command, Commands, Handler, WindowContext};
use tv_render::{Fonts, FrameResult, Presentation, Renderer, DEFAULT_ORDER};
use tv_ui::{Align, Canvas, Event, FontRole, Key, Rect, TextStyle, Theme};

const UI_LEFT: TextStyle = TextStyle::new(FontRole::Ui, Align::Left);
const STATUS_PADDING: f32 = 8.0;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
/// More than this many rows in one copy would make a clipboard entry too large
/// for most apps to accept anyway.
const MAX_COPY_ROWS: usize = 100_000;
const WELCOME: &str = "Open a log with Ctrl+O, or drop one here";

struct Document {
    log: Arc<LogFile>,
    stats: OpenStats,
    cache: RowCache,
}

impl RowSource for Document {
    fn row_count(&self) -> usize {
        self.log.rows()
    }

    fn level(&self, row: usize) -> Level {
        self.log.level(row)
    }

    fn cells(&self, row: usize) -> Option<Arc<RowCells>> {
        self.cache.get(row)
    }
}

struct Opening {
    path: PathBuf,
    size: u64,
    bytes_done: Arc<AtomicU64>,
    result: Receiver<io::Result<(LogFile, OpenStats)>>,
}

struct CopyResult {
    text: String,
    rows: usize,
    cut_short: bool,
}

pub struct App {
    renderer: Renderer,
    theme: &'static Theme,
    view: LogView,
    document: Option<Document>,
    opening: Option<Opening>,
    copying: Option<Receiver<io::Result<CopyResult>>>,
    message: Option<String>,
    waker: tv_platform::Waker,
    caption_applied: bool,
    #[cfg(feature = "bench")]
    bench: Option<Bench>,
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// Groups digits: 26904325 becomes 26,904,325.
fn with_commas(value: usize) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

fn readable_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit + 1 < UNITS.len() {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

fn encoding_name(encoding: Encoding) -> &'static str {
    match encoding {
        Encoding::Utf8 => "UTF-8",
        Encoding::Utf16Le | Encoding::Utf16Be => "UTF-16",
    }
}

fn format_name(format: Format) -> &'static str {
    match format {
        Format::Plain => "Text",
        Format::Sccm => "SCCM",
    }
}

fn status_text(
    document: Option<&Document>,
    opening: Option<&Opening>,
    message: Option<&str>,
    selection: Option<std::ops::RangeInclusive<usize>>,
) -> String {
    if let Some(opening) = opening {
        let done = opening.bytes_done.load(Ordering::Relaxed).min(opening.size);
        let percent = done * 100 / opening.size.max(1);
        return format!("Indexing {}: {percent}%", file_name(&opening.path));
    }
    let mut parts = Vec::new();
    if let Some(document) = document {
        let log = &document.log;
        parts.push(format!("{} rows", with_commas(log.rows())));
        if let Some(range) = selection {
            let count = range.end() - range.start() + 1;
            if count > 1 {
                parts.push(format!("{} selected", with_commas(count)));
            } else {
                parts.push(format!("Row {} selected", with_commas(range.start() + 1)));
            }
        }
        parts.push(encoding_name(log.encoding()).to_string());
        parts.push(readable_size(log.indexed_len()));
        parts.push(format_name(log.format()).to_string());
        parts.push(format!(
            "Opened in {:.2} s{}",
            document.stats.seconds,
            match document.stats.cache {
                CacheUse::Hit => " from cache",
                CacheUse::Extended => " from cache, plus new rows",
                CacheUse::Miss => "",
            }
        ));
    }
    if let Some(message) = message {
        parts.push(message.to_string());
    }
    parts.join("   |   ")
}

/// Draws a whole frame. It works on any canvas, so tests can draw it into memory.
fn draw_frame(
    canvas: &mut dyn Canvas,
    view: &LogView,
    source: Option<&dyn RowSource>,
    theme: &Theme,
    status: &str,
    area: Rect,
    scale: f32,
) {
    canvas.clear(theme.window_background);
    let mut body = area;
    let status_bar = body.take_bottom(snap(theme.status_height, scale));
    match source {
        Some(source) => view.draw(canvas, source, theme),
        None => {
            let message = if status.starts_with("Indexing") {
                ""
            } else {
                WELCOME
            };
            canvas.text(
                message,
                Rect::new(
                    body.left + 24.0,
                    body.top + 20.0,
                    body.right,
                    body.top + 60.0,
                ),
                UI_LEFT,
                theme.secondary_text,
            );
        }
    }
    canvas.fill_rect(status_bar, theme.status_background);
    canvas.fill_rect(
        Rect::new(
            status_bar.left,
            status_bar.top,
            status_bar.right,
            status_bar.top + 1.0,
        ),
        theme.border,
    );
    canvas.text(
        status,
        Rect::new(
            status_bar.left + STATUS_PADDING,
            status_bar.top,
            status_bar.right - STATUS_PADDING,
            status_bar.bottom,
        ),
        UI_LEFT,
        theme.secondary_text,
    );
}

/// Builds tab-separated text, so pasted rows line up in a spreadsheet or editor.
fn copy_rows(log: &LogFile, rows: std::ops::Range<usize>) -> io::Result<String> {
    let (mut buffer, mut scratch) = (Vec::new(), Vec::new());
    let mut text = String::new();
    log.read_rows(rows, &mut buffer, |row, raw, _| {
        let cells = cells_from_row(log, raw, &mut scratch);
        text.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\r\n",
            row + 1,
            cells.time,
            level_name(log.level(row)),
            cells.component,
            cells.message
        ));
    })?;
    Ok(text)
}

impl App {
    pub fn create(
        context: WindowContext,
        path: Option<PathBuf>,
        #[cfg(feature = "bench")] bench: Option<BenchOptions>,
        #[cfg(not(feature = "bench"))] _bench: Option<()>,
    ) -> Result<Self, String> {
        #[cfg(feature = "bench")]
        let bench = bench.map(Bench::new);
        #[cfg(feature = "bench")]
        let single;
        #[cfg(feature = "bench")]
        let order: &[Presentation] = match &bench {
            Some(bench) => {
                single = [bench.presentation()];
                &single
            }
            None => &DEFAULT_ORDER,
        };
        #[cfg(not(feature = "bench"))]
        let order: &[Presentation] = &DEFAULT_ORDER;

        let mut renderer = Renderer::for_window(
            context.hwnd,
            context.width,
            context.height,
            context.dpi,
            order,
        )
        .map_err(|error| format!("couldn't set up drawing: {error}"))?;
        #[cfg(feature = "bench")]
        if bench.is_some() {
            renderer.set_vsync(false);
        }
        let theme = Theme::for_system(context.system_is_dark);
        renderer.set_fonts(fonts_for(theme));
        let mut app = Self {
            renderer,
            theme,
            view: LogView::default(),
            document: None,
            opening: None,
            copying: None,
            message: None,
            waker: context.waker,
            caption_applied: false,
            #[cfg(feature = "bench")]
            bench,
        };
        if let Some(path) = path {
            app.start_open(path);
        }
        Ok(app)
    }

    fn start_open(&mut self, path: PathBuf) {
        let size = std::fs::metadata(&path).map_or(0, |metadata| metadata.len());
        let bytes_done = Arc::new(AtomicU64::new(0));
        let finished = Arc::new(AtomicBool::new(false));
        let (sender, result) = channel();

        let (open_path, progress, open_finished, open_waker) = (
            path.clone(),
            bytes_done.clone(),
            finished.clone(),
            self.waker.clone(),
        );
        std::thread::spawn(move || {
            let opened = LogFile::open(&open_path, &progress);
            open_finished.store(true, Ordering::Release);
            let _ = sender.send(opened);
            open_waker.wake();
        });

        // Keeps the progress display moving during a long first open.
        let (ticker_finished, ticker_waker) = (finished.clone(), self.waker.clone());
        std::thread::spawn(move || {
            while !ticker_finished.load(Ordering::Acquire) {
                std::thread::sleep(PROGRESS_INTERVAL);
                ticker_waker.wake();
            }
        });

        self.message = None;
        self.opening = Some(Opening {
            path,
            size,
            bytes_done,
            result,
        });
    }

    fn finish_open(&mut self, commands: &mut Commands) {
        let Some(opening) = &self.opening else {
            return;
        };
        match opening.result.try_recv() {
            Ok(Ok((log, stats))) => {
                let log = Arc::new(log);
                let waker = self.waker.clone();
                let cache = RowCache::new(log.clone(), move || waker.wake());
                commands.push(Command::SetTitle(format!(
                    "TraceView - {}",
                    file_name(log.path())
                )));
                self.view.reset();
                self.document = Some(Document { log, stats, cache });
                self.opening = None;
            }
            Ok(Err(error)) => {
                self.message = Some(format!(
                    "Couldn't open {}: {error}",
                    file_name(&opening.path)
                ));
                self.opening = None;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.message = Some(format!("Couldn't open {}", file_name(&opening.path)));
                self.opening = None;
            }
        }
    }

    fn start_copy(&mut self) {
        let (Some(document), Some(selection)) = (&self.document, self.view.selection()) else {
            return;
        };
        let start = *selection.start();
        let total = selection.end() - start + 1;
        let count = total.min(MAX_COPY_ROWS);
        let log = document.log.clone();
        let waker = self.waker.clone();
        let (sender, result) = channel();
        std::thread::spawn(move || {
            let copied = copy_rows(&log, start..start + count).map(|text| CopyResult {
                text,
                rows: count,
                cut_short: total > count,
            });
            let _ = sender.send(copied);
            waker.wake();
        });
        self.copying = Some(result);
        self.message = Some("Copying...".to_string());
    }

    fn finish_copy(&mut self, commands: &mut Commands) {
        let Some(copying) = &self.copying else {
            return;
        };
        match copying.try_recv() {
            Ok(Ok(copied)) => {
                commands.push(Command::SetClipboard(copied.text));
                let rows = with_commas(copied.rows);
                self.message = Some(if copied.cut_short {
                    format!("Copied the first {rows} rows")
                } else {
                    format!("Copied {rows} rows")
                });
                self.copying = None;
            }
            Ok(Err(error)) => {
                self.message = Some(format!("Couldn't copy: {error}"));
                self.copying = None;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => self.copying = None,
        }
    }

    fn status(&self) -> String {
        status_text(
            self.document.as_ref(),
            self.opening.as_ref(),
            self.message.as_deref(),
            self.view.selection(),
        )
    }

    fn window_title(&self) -> String {
        self.document.as_ref().map_or_else(
            || "TraceView".to_string(),
            |document| format!("TraceView - {}", file_name(document.log.path())),
        )
    }

    /// Updates the view's layout for the current window size and row count.
    fn arrange(&mut self) {
        let scale = self.renderer.scale();
        let (width, height) = self.renderer.logical_size();
        let mut area = Rect::new(0.0, 0.0, width, height);
        area.take_bottom(snap(self.theme.status_height, scale));
        let rows = self
            .document
            .as_ref()
            .map_or(0, |document| document.log.rows());
        let metrics = Metrics {
            row_height: snap(self.theme.row_height, scale),
            char_width: self.renderer.char_width(FontRole::Grid),
        };
        self.view.arrange(area, rows, metrics, self.theme, scale);
    }

    fn apply_caption(&mut self, commands: &mut Commands) {
        commands.push(Command::SetCaptionColors {
            background: self.theme.status_background,
            text: self.theme.primary_text,
            dark: self.theme.is_dark,
        });
        self.caption_applied = true;
    }

    fn draw(&mut self, commands: &mut Commands) {
        self.arrange();
        if let Some(document) = &mut self.document {
            document.cache.request(self.view.visible_rows());
        }
        let status = self.status();
        let scale = self.renderer.scale();
        let (width, height) = self.renderer.logical_size();
        self.renderer.begin();
        draw_frame(
            &mut self.renderer,
            &self.view,
            self.document
                .as_ref()
                .map(|document| document as &dyn RowSource),
            self.theme,
            &status,
            Rect::new(0.0, 0.0, width, height),
            scale,
        );
        match self.renderer.end() {
            Ok(FrameResult::Presented) => {}
            Ok(FrameResult::DeviceRecreated) => commands.redraw(),
            Err(error) => self.message = Some(format!("Drawing failed: {error}")),
        }
    }
}

fn fonts_for(theme: &Theme) -> Fonts {
    Fonts {
        ui_size: theme.ui_font_size,
        grid_size: theme.grid_font_size,
        ..Fonts::default()
    }
}

impl Handler for App {
    fn event(&mut self, event: Event, commands: &mut Commands) {
        if let Event::KeyDown(Key::Char('O'), modifiers) = event {
            if modifiers.ctrl && !modifiers.alt {
                commands.push(Command::OpenFileDialog);
                return;
            }
        }
        let response = self.view.handle(event);
        if response.copy_selection {
            self.start_copy();
            commands.redraw();
        }
        if response.redraw {
            commands.redraw();
            commands.accessibility_changed();
        }
    }

    fn paint(&mut self, commands: &mut Commands) {
        #[cfg(feature = "bench")]
        if let Some(bench) = &mut self.bench {
            bench.frame_started();
            let (width, height) = self.renderer.logical_size();
            let scale = self.renderer.scale();
            bench.set_window_pixels(width * scale, height * scale);
        }
        self.draw(commands);
        #[cfg(feature = "bench")]
        if let Some(bench) = &mut self.bench {
            let index_bytes = self
                .document
                .as_ref()
                .map_or(0, |document| document.log.index_memory_bytes());
            let source = self
                .document
                .as_ref()
                .map(|document| document as &dyn RowSource);
            bench.after_frame(
                &mut self.view,
                source,
                index_bytes,
                self.renderer.presentation(),
                commands,
            );
        }
    }

    fn resized(&mut self, width: u32, height: u32, dpi: f32, commands: &mut Commands) {
        if let Err(error) = self.renderer.resize(width, height, dpi) {
            self.message = Some(format!("Couldn't resize: {error}"));
        }
        self.arrange();
        if !self.caption_applied {
            self.apply_caption(commands);
        }
        commands.accessibility_changed();
    }

    fn woken(&mut self, commands: &mut Commands) {
        self.finish_open(commands);
        self.finish_copy(commands);
        commands.redraw();
        commands.accessibility_changed();
    }

    fn files_chosen(&mut self, paths: Vec<PathBuf>, commands: &mut Commands) {
        if let Some(path) = paths.into_iter().next() {
            self.start_open(path);
            commands.redraw();
        }
    }

    fn system_theme_changed(&mut self, dark: bool, commands: &mut Commands) {
        let theme = Theme::for_system(dark);
        if theme.name != self.theme.name {
            self.theme = theme;
            self.renderer.set_fonts(fonts_for(theme));
            self.apply_caption(commands);
            commands.redraw();
        }
    }

    fn accessibility_tree(&mut self) -> TreeUpdate {
        accessibility::build(
            &self.view,
            self.document
                .as_ref()
                .map(|document| document as &dyn RowSource),
            &self.window_title(),
            &self.status(),
            self.renderer.scale(),
        )
    }

    fn accessibility_action(&mut self, request: ActionRequest, commands: &mut Commands) {
        match request.action {
            Action::Focus | Action::Click => {
                if let Some(row) = accessibility::row_from_id(request.target_node) {
                    self.view.select(row, false);
                }
            }
            Action::ScrollUp => {
                let page = self.view.page_rows() as f64;
                self.view.scroll_to(self.view.top_row() as f64 - page);
            }
            Action::ScrollDown => {
                let page = self.view.page_rows() as f64;
                self.view.scroll_to(self.view.top_row() as f64 + page);
            }
            _ => return,
        }
        commands.redraw();
        commands.accessibility_changed();
    }
}

#[cfg(test)]
mod tests {
    use super::super::log_view::Metrics;
    use super::*;
    use tv_ui::Color;

    #[test]
    fn numbers_get_thousands_separators() {
        assert_eq!(with_commas(0), "0");
        assert_eq!(with_commas(999), "999");
        assert_eq!(with_commas(1000), "1,000");
        assert_eq!(with_commas(26_904_325), "26,904,325");
    }

    #[test]
    fn sizes_are_readable() {
        assert_eq!(readable_size(512), "512 B");
        assert_eq!(readable_size(164 * 1024), "164.0 KB");
        assert_eq!(readable_size(5_368_709_143), "5.0 GB");
    }

    fn open_log(text: &str) -> (tempfile::TempDir, Arc<LogFile>, OpenStats) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.log");
        std::fs::write(&path, text).unwrap();
        let (log, stats) = LogFile::open_with_cache(&path, None, &AtomicU64::new(0)).unwrap();
        (dir, Arc::new(log), stats)
    }

    #[test]
    fn copies_rows_as_tab_separated_text() {
        let (_dir, log, _) = open_log("2026-01-01 10:00:00 host app[1]: ERROR: disk full\nnext\n");
        assert_eq!(
            copy_rows(&log, 0..2).unwrap(),
            "1\t2026-01-01 10:00:00.000\tError\tapp\tdisk full\r\n2\t\tInformation\t\tnext\r\n"
        );
    }

    fn wait_for_rows(document: &Document, rows: std::ops::Range<usize>) {
        let started = std::time::Instant::now();
        while !rows.clone().all(|row| document.cells(row).is_some()) {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "rows never loaded"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn sample_log() -> String {
        let mut text = String::new();
        let messages = [
            (1, "Connected to management point mp01.contoso.com"),
            (1, "Sending state message body to the management point"),
            (3, "Failed to download policy from mp01 - error 0x80004005"),
            (2, "Retrying policy download in 15 minutes"),
            (1, "Software update scan completed, 3 updates applicable"),
            (3, "Content download failed for KB5034441: hash mismatch"),
            (1, "Client certificate validation succeeded"),
        ];
        for row in 0..2000 {
            let (kind, message) = messages[row % messages.len()];
            text.push_str(&format!(
                "<![LOG[{message}]LOG]!><time=\"10:{:02}:{:02}.{:03}+000\" date=\"04-06-2026\" component=\"PolicyAgent\" context=\"\" type=\"{kind}\" thread=\"4128\" file=\"a.cpp\">\r\n",
                row / 60 % 60,
                row % 60,
                row % 1000
            ));
        }
        text
    }

    /// Writes a frame out if `TV_SNAPSHOT_DIR` is set, so a person can look at it.
    fn save_snapshot(name: &str, pixels: &[u8], width: u32, height: u32) {
        let Some(dir) = std::env::var_os("TV_SNAPSHOT_DIR") else {
            return;
        };
        let path = PathBuf::from(dir).join(format!("{name}.png"));
        let file = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
        let mut encoder = png::Encoder::new(file, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(pixels).unwrap();
        writer.finish().unwrap();
    }

    fn pixel_at(pixels: &[u8], width: u32, x: f32, y: f32) -> Color {
        let at = ((y as u32 * width + x as u32) * 4) as usize;
        Color {
            r: pixels[at],
            g: pixels[at + 1],
            b: pixels[at + 2],
        }
    }

    /// Draws the real frame with the real renderer, then checks the pixels.
    fn render_snapshot(theme: &'static Theme, name: &str, dpi: f32) {
        let scale = dpi / 96.0;
        let (physical_width, physical_height) = ((1200.0 * scale) as u32, (500.0 * scale) as u32);
        let (_dir, log, stats) = open_log(&sample_log());
        let waker_flag = Arc::new(AtomicBool::new(false));
        let flag = waker_flag.clone();
        let mut cache = RowCache::new(log.clone(), move || flag.store(true, Ordering::SeqCst));
        cache.request(0..40);
        let document = Document { log, stats, cache };
        wait_for_rows(&document, 0..25);

        let mut renderer = Renderer::offscreen(physical_width, physical_height, dpi).unwrap();
        renderer.set_fonts(fonts_for(theme));
        let (width, height) = renderer.logical_size();
        let mut area = Rect::new(0.0, 0.0, width, height);
        area.take_bottom(snap(theme.status_height, scale));
        let metrics = Metrics {
            row_height: snap(theme.row_height, scale),
            char_width: renderer.char_width(FontRole::Grid),
        };
        let mut view = LogView::default();
        view.arrange(area, document.log.rows(), metrics, theme, scale);
        view.select(9, false);
        let status = status_text(Some(&document), None, None, view.selection());
        renderer.begin();
        draw_frame(
            &mut renderer,
            &view,
            Some(&document),
            theme,
            &status,
            Rect::new(0.0, 0.0, width, height),
            scale,
        );
        renderer.end().unwrap();
        let pixels = renderer.rgba_pixels().unwrap();
        save_snapshot(name, &pixels, physical_width, physical_height);

        let error_row = view.row_rect(2);
        let warning_row = view.row_rect(3);
        let selected_row = view.row_rect(9);
        let edge = (width - 30.0) * scale;
        let middle = |rect: Rect| (rect.top + rect.height() / 2.0) * scale;
        assert_eq!(
            pixel_at(&pixels, physical_width, edge, middle(error_row)),
            theme.error_row
        );
        assert_eq!(
            pixel_at(&pixels, physical_width, edge, middle(warning_row)),
            theme.warning_row
        );
        assert_eq!(
            pixel_at(&pixels, physical_width, edge, middle(selected_row)),
            theme.selected_row
        );

        let message_start = (view.row_rect(0).left + 340.0) * scale;
        let row_top = view.row_rect(0).top * scale;
        let has_ink = (message_start as u32..(message_start as u32 + 300)).any(|x| {
            (row_top as u32..(row_top + metrics.row_height * scale) as u32).any(|y| {
                pixel_at(&pixels, physical_width, x as f32, y as f32) != theme.grid_background
            })
        });
        assert!(has_ink, "{name}: the first row's text didn't draw");
    }

    #[test]
    fn classic_theme_renders_correctly() {
        render_snapshot(&Theme::CLASSIC, "classic-100", 96.0);
    }

    #[test]
    fn console_theme_renders_correctly() {
        render_snapshot(&Theme::CONSOLE, "console-100", 96.0);
    }

    #[test]
    fn fractional_display_scale_renders_correctly() {
        render_snapshot(&Theme::CLASSIC, "classic-125", 120.0);
    }

    #[test]
    fn high_density_display_renders_correctly() {
        render_snapshot(&Theme::CONSOLE, "console-200", 192.0);
    }

    #[test]
    fn welcome_screen_shows_with_no_log() {
        let mut renderer = Renderer::offscreen(800, 300, 96.0).unwrap();
        let theme = &Theme::CLASSIC;
        renderer.set_fonts(fonts_for(theme));
        renderer.begin();
        draw_frame(
            &mut renderer,
            &LogView::default(),
            None,
            theme,
            "",
            Rect::new(0.0, 0.0, 800.0, 300.0),
            1.0,
        );
        renderer.end().unwrap();
        let pixels = renderer.rgba_pixels().unwrap();
        save_snapshot("welcome", &pixels, 800, 300);
        let has_ink = pixels
            .as_chunks::<4>()
            .0
            .iter()
            .take(800 * 100)
            .any(|pixel| pixel[0] != 0xFF);
        assert!(has_ink);
    }

    #[test]
    fn status_shows_the_details_a_person_needs() {
        let (_dir, log, stats) = open_log("a\nb\n");
        let cache = RowCache::new(log.clone(), || {});
        let document = Document { log, stats, cache };
        let text = status_text(Some(&document), None, Some("Copied 2 rows"), Some(0..=1));
        assert!(text.contains("2 rows"));
        assert!(text.contains("2 selected"));
        assert!(text.contains("UTF-8"));
        assert!(text.contains("Copied 2 rows"));
    }

    #[test]
    fn opening_progress_replaces_the_status() {
        let (_sender, result) = channel();
        let opening = Opening {
            path: PathBuf::from("C:\\logs\\big.log"),
            size: 200,
            bytes_done: Arc::new(AtomicU64::new(50)),
            result,
        };
        assert_eq!(
            status_text(None, Some(&opening), None, None),
            "Indexing big.log: 25%"
        );
    }

    #[test]
    fn progress_past_the_estimate_stops_at_one_hundred() {
        let (_sender, result) = channel();
        let opening = Opening {
            path: PathBuf::from("big.log"),
            size: 10,
            bytes_done: Arc::new(AtomicU64::new(16_000_000)),
            result,
        };
        assert!(status_text(None, Some(&opening), None, None).ends_with("100%"));
    }
}
