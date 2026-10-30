//! Window state, background open and filter jobs, and the benchmark script.

use crate::bench::{Bench, Stage, FILTERS};
use crate::grid::Grid;
use crate::theme::Theme;
use crate::{metrics, Options};
use eframe::egui::{self, FontData, FontDefinitions, FontFamily, FontTweak, ViewportCommand};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::Instant;
use tv_engine::{filter, LogFile, OpenStats};

struct Opening {
    path: PathBuf,
    size: u64,
    bytes_done: Arc<AtomicU64>,
    result: Receiver<std::io::Result<(LogFile, OpenStats)>>,
}

struct Filtering {
    rows_done: Arc<AtomicUsize>,
    cancel: Arc<AtomicBool>,
    started: Instant,
    result: Receiver<Option<Vec<u32>>>,
}

pub struct App {
    options: Options,
    theme: Theme,
    log: Option<Arc<LogFile>>,
    open_stats: Option<OpenStats>,
    opening: Option<Opening>,
    filtering: Option<Filtering>,
    view: Option<Arc<Vec<u32>>>,
    grid: Grid,
    filter_text: String,
    use_regex: bool,
    status: String,
    bench: Option<Bench>,
    frames_drawn: u64,
    applied_top_row: bool,
}

impl App {
    pub fn new(creation: &eframe::CreationContext<'_>, options: Options) -> Self {
        metrics::mark("renderer_ready");
        install_fonts(&creation.egui_ctx);
        metrics::mark("fonts_installed");
        creation.egui_ctx.all_styles_mut(|style| {
            // Full-pixel glyph positions with font hinting give the sharpest small text.
            style.visuals.text_options.font_hinting = true;
            style.visuals.text_options.subpixel_binning = false;
        });
        creation.egui_ctx.set_theme(if options.light {
            egui::Theme::Light
        } else {
            egui::Theme::Dark
        });

        let mut bench = options
            .bench
            .clone()
            .map(|output| Bench::new(output, options.frames));
        if let Some(bench) = &mut bench {
            bench.adapter = match creation.wgpu_render_state.as_ref() {
                Some(state) => {
                    let info = state.adapter.get_info();
                    format!("{} ({:?}, {:?})", info.name, info.backend, info.device_type)
                }
                None => "glow (OpenGL)".to_string(),
            };
        }
        let theme = if options.light { Theme::light() } else { Theme::console() };
        let mut app = Self {
            theme,
            log: None,
            open_stats: None,
            opening: None,
            filtering: None,
            view: None,
            grid: Grid::default(),
            filter_text: String::new(),
            use_regex: false,
            status: "Drop a log file on this window".into(),
            bench,
            frames_drawn: 0,
            applied_top_row: false,
            options,
        };
        if let Some(path) = app.options.path.clone() {
            app.start_open(path, &creation.egui_ctx);
        }
        app
    }

    fn start_open(&mut self, path: PathBuf, ctx: &egui::Context) {
        let (sender, result) = channel();
        let bytes_done = Arc::new(AtomicU64::new(0));
        let size = std::fs::metadata(&path).map_or(0, |metadata| metadata.len());
        let progress = bytes_done.clone();
        let open_path = path.clone();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let _ = sender.send(LogFile::open(&open_path, &progress));
            repaint.request_repaint();
        });
        self.cancel_filter();
        self.opening = Some(Opening {
            path,
            size,
            bytes_done,
            result,
        });
    }

    fn cancel_filter(&mut self) {
        if let Some(filtering) = self.filtering.take() {
            filtering.cancel.store(true, Ordering::Relaxed);
        }
    }

    fn start_filter(&mut self, text: &str, use_regex: bool, ctx: &egui::Context) {
        self.cancel_filter();
        self.grid.top = 0.0;
        let Some(log) = self.log.clone() else {
            return;
        };
        if text.is_empty() {
            self.view = None;
            return;
        }
        let matcher = match filter::compile(text, !use_regex) {
            Ok(matcher) => matcher,
            Err(error) => {
                self.status = format!("Invalid pattern: {error}");
                return;
            }
        };
        let (sender, result) = channel();
        let rows_done = Arc::new(AtomicUsize::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let (progress, stop, repaint) = (rows_done.clone(), cancel.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = sender.send(filter::run(&log, &matcher, &progress, &stop));
            repaint.request_repaint();
        });
        self.filtering = Some(Filtering {
            rows_done,
            cancel,
            started: Instant::now(),
            result,
        });
    }

    fn poll_jobs(&mut self, ctx: &egui::Context) {
        if let Some(opening) = &self.opening {
            match opening.result.try_recv() {
                Ok(Ok((log, stats))) => {
                    self.status = open_summary(&opening.path, &log, &stats);
                    self.log = Some(Arc::new(log));
                    self.open_stats = Some(stats);
                    self.view = None;
                    self.grid.top = 0.0;
                    self.opening = None;
                }
                Ok(Err(error)) => {
                    self.status = format!("Couldn't open {}: {error}", opening.path.display());
                    self.opening = None;
                }
                Err(_) => {
                    let percent = opening.bytes_done.load(Ordering::Relaxed) as f64
                        / opening.size.max(1) as f64
                        * 100.0;
                    self.status = format!("Indexing {}: {percent:.0}%", opening.path.display());
                    ctx.request_repaint_after(std::time::Duration::from_millis(100));
                }
            }
        }
        if let Some(filtering) = &self.filtering {
            match filtering.result.try_recv() {
                Ok(Some(matched)) => {
                    self.status = format!(
                        "{} matches in {:.2}s",
                        matched.len(),
                        filtering.started.elapsed().as_secs_f64()
                    );
                    self.view = Some(Arc::new(matched));
                    self.filtering = None;
                }
                Ok(None) => self.filtering = None,
                Err(_) => {
                    let total = self.log.as_ref().map_or(1, |log| log.rows()).max(1);
                    let percent =
                        filtering.rows_done.load(Ordering::Relaxed) as f64 / total as f64 * 100.0;
                    self.status = format!("Filtering: {percent:.0}%");
                    ctx.request_repaint_after(std::time::Duration::from_millis(100));
                }
            }
        }
    }

    /// Runs one step of the scripted benchmark, before the frame is drawn.
    fn step_bench(&mut self, ctx: &egui::Context) {
        let Some(mut bench) = self.bench.take() else {
            return;
        };
        ctx.request_repaint();
        if self.frames_drawn == 0 {
            metrics::mark("first_frame_started");
        }
        if bench.launch_ms.is_none() && self.frames_drawn >= 1 {
            metrics::mark("second_frame_started");
            bench.launch_ms = Some(metrics::ms_since_process_start());
        }
        if self.options.launch_only {
            if bench.launch_ms.is_some() {
                bench.write_report();
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
            self.bench = Some(bench);
            return;
        }
        let rows = self.view.as_ref().map_or_else(
            || self.log.as_ref().map_or(0, |log| log.rows()),
            |view| view.len(),
        );
        let max_top = self.grid.max_top(rows);

        match bench.stage {
            Stage::Loading => {
                if let (Some(log), Some(stats)) = (&self.log, &self.open_stats) {
                    bench.open_seconds = stats.seconds;
                    bench.cache_hit = stats.cache_hit;
                    bench.rows = log.rows();
                    bench.index_bytes = log.index.memory_bytes();
                    bench.file = log.path.display().to_string();
                    bench.stage = Stage::Settling(0);
                } else if self.opening.is_none() {
                    let _ = std::fs::write(bench.output.with_extension("err"), &self.status);
                    bench.stage = Stage::Done;
                }
            }
            Stage::Settling(frames) if frames < 10 => bench.stage = Stage::Settling(frames + 1),
            Stage::Settling(_) => {
                let (done, path) = (bench.screenshot_done.clone(), bench.screenshot_path());
                ctx.request_screenshot(move |image| {
                    let _ = save_png(&path, &image);
                    done.store(true, Ordering::Release);
                });
                bench.stage = Stage::AwaitScreenshot;
            }
            Stage::AwaitScreenshot => {
                if bench.screenshot_ready() {
                    bench.stage = Stage::Wheel;
                }
            }
            Stage::Wheel => {
                self.grid.top += 3.0;
                if bench.stage_finished() {
                    bench.finish_stage("wheel", Stage::PageDown);
                }
            }
            Stage::PageDown => {
                self.grid.top += self.grid.visible_rows.floor();
                if bench.stage_finished() {
                    bench.finish_stage("page_down", Stage::RandomJump);
                }
            }
            Stage::RandomJump => {
                self.grid.top = (bench.next_random() % (max_top.max(1.0) as u64)) as f64;
                if bench.stage_finished() {
                    bench.finish_stage("random_jump", Stage::Sweep);
                }
            }
            Stage::Sweep => {
                self.grid.top += max_top / bench.frames_per_stage as f64;
                if bench.stage_finished() {
                    bench.finish_stage("scrollbar_sweep", Stage::Filter(0));
                    let (_, text, literal) = FILTERS[0];
                    self.start_filter(text, !literal, ctx);
                }
            }
            Stage::Filter(which) => {
                if self.filtering.is_none() {
                    let (name, _, _) = FILTERS[which];
                    let matches = self.view.as_ref().map_or(0, |view| view.len());
                    let seconds = parse_seconds(&self.status);
                    let next = which + 1;
                    if next < FILTERS.len() {
                        bench.finish_filter(name, seconds, matches, Stage::Filter(next));
                        let (_, text, literal) = FILTERS[next];
                        self.start_filter(text, !literal, ctx);
                    } else {
                        bench.finish_filter(name, seconds, matches, Stage::Done);
                    }
                }
            }
            Stage::Done => {
                bench.write_report();
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
        }
        self.bench = Some(bench);
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let started = Instant::now();
        let ctx = ui.ctx().clone();
        self.poll_jobs(&ctx);

        let dropped = ctx.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_path_buf())
                .find(|path| !path.as_os_str().is_empty())
        });
        if let Some(path) = dropped {
            self.start_open(path, &ctx);
        }
        self.step_bench(&ctx);

        egui::Panel::top("filter").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("Filter");
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut self.filter_text)
                        .desired_width(420.0)
                        .hint_text("Press Enter to apply"),
                );
                let regex_changed = ui.checkbox(&mut self.use_regex, "Regex").changed();
                let submitted = edit.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                if submitted || regex_changed {
                    let (text, use_regex) = (self.filter_text.clone(), self.use_regex);
                    self.start_filter(&text, use_regex, &ctx);
                }
            });
        });
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.label(&self.status);
        });
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                if let Some(log) = self.log.clone() {
                    if !self.applied_top_row {
                        self.grid.top = self.options.top_row;
                        self.applied_top_row = true;
                    }
                    let view = self.view.clone();
                    self.grid.show(
                        ui,
                        &log,
                        view.as_deref().map(Vec::as_slice),
                        &self.theme,
                        self.options.font_size,
                    );
                }
            });

        self.frames_drawn += 1;
        if let Some(bench) = &mut self.bench {
            let measuring = !matches!(bench.stage, Stage::Loading | Stage::Settling(_) | Stage::AwaitScreenshot | Stage::Done);
            if measuring {
                bench.record_frame(started.elapsed().as_secs_f64() * 1000.0);
            }
        }
    }
}

fn parse_seconds(status: &str) -> f64 {
    status
        .rsplit(' ')
        .next()
        .and_then(|last| last.strip_suffix('s'))
        .and_then(|seconds| seconds.parse().ok())
        .unwrap_or(0.0)
}

fn open_summary(path: &Path, log: &LogFile, stats: &OpenStats) -> String {
    format!(
        "{}: {} rows, opened in {:.2}s{}",
        path.display(),
        log.rows(),
        stats.seconds,
        if stats.cache_hit { " (cached index)" } else { "" },
    )
}

/// Uses the system's Consolas and Segoe UI, so no fonts ship in the EXE. If
/// they're missing (as on some stripped-down images), egui's built-ins remain.
fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let windir = std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into());
    let font_dir = Path::new(&windir).join("Fonts");
    for (file, name, family) in [
        ("consola.ttf", "Consolas", FontFamily::Monospace),
        ("segoeui.ttf", "Segoe UI", FontFamily::Proportional),
    ] {
        if let Ok(bytes) = std::fs::read(font_dir.join(file)) {
            let data = FontData::from_owned(bytes).tweak(FontTweak::default());
            fonts.font_data.insert(name.to_string(), Arc::new(data));
            fonts
                .families
                .entry(family)
                .or_default()
                .insert(0, name.to_string());
        }
    }
    ctx.set_fonts(fonts);
}

fn save_png(path: &Path, image: &egui::ColorImage) -> Result<(), Box<dyn std::error::Error>> {
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut encoder = png::Encoder::new(file, image.size[0] as u32, image.size[1] as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    let pixels: Vec<u8> = image.pixels.iter().flat_map(|pixel| pixel.to_array()).collect();
    writer.write_image_data(&pixels)?;
    Ok(())
}
