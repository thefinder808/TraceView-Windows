//! A scripted run through the real window, for performance checks. It's only
//! compiled with the `bench` feature, so it isn't part of shipped builds.

use super::log_view::{LogView, RowSource};
use serde::Serialize;
use std::path::PathBuf;
use std::time::Instant;
use tv_platform::metrics;
use tv_platform::{Command, Commands};
use tv_render::Presentation;

pub struct Options {
    pub report: PathBuf,
    pub log: Option<PathBuf>,
    pub frames: usize,
    pub launch_only: bool,
    /// In physical pixels, so a run measures the same window at any display
    /// scale and the budgets mean the same everywhere.
    pub window: (f32, f32),
    pub presentation: Presentation,
}

impl Options {
    pub fn parse(args: &[String]) -> Option<Self> {
        let mut args = args.iter();
        let mut options = Self {
            report: PathBuf::from(args.next()?),
            log: None,
            frames: 600,
            launch_only: false,
            window: (1400.0, 850.0),
            presentation: Presentation::FlipWarp,
        };
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--frames" => options.frames = args.next()?.parse().ok()?,
                "--launch-only" => options.launch_only = true,
                "--window" => {
                    let (width, height) = args.next()?.split_once('x')?;
                    options.window = (width.parse().ok()?, height.parse().ok()?);
                }
                "--presentation" => {
                    options.presentation = match args.next()?.as_str() {
                        "warp" => Presentation::FlipWarp,
                        "hardware" => Presentation::FlipHardware,
                        "software" => Presentation::SoftwareWindow,
                        _ => return None,
                    };
                }
                path => options.log = Some(PathBuf::from(path)),
            }
        }
        Some(options)
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Stage {
    WaitingForLog,
    Settling(usize),
    Scrolling { stage: usize, frame: usize },
    Done,
}

const STAGES: [&str; 4] = ["wheel", "page_down", "random_jump", "scrollbar_sweep"];

#[derive(Serialize)]
struct Percentiles {
    p50: f64,
    p95: f64,
    p99: f64,
    max: f64,
}

fn percentiles(mut samples: Vec<f64>) -> Percentiles {
    samples.sort_by(f64::total_cmp);
    let at = |share: f64| {
        samples
            .get(((samples.len().max(1) - 1) as f64 * share) as usize)
            .copied()
            .unwrap_or(0.0)
    };
    Percentiles {
        p50: at(0.5),
        p95: at(0.95),
        p99: at(0.99),
        max: samples.last().copied().unwrap_or(0.0),
    }
}

#[derive(Serialize)]
struct StageReport {
    name: &'static str,
    frames: usize,
    /// Frames where a visible row was still loading.
    frames_with_placeholders: usize,
    frame_ms: Percentiles,
}

#[derive(Serialize)]
struct Report<'a> {
    presentation: String,
    window_pixels: (f32, f32),
    launch_ms: f64,
    log: Option<&'a PathBuf>,
    rows: usize,
    index_mb: f64,
    private_mb: f64,
    /// Memory beyond the row index, which is what the 50 MB budget covers.
    overhead_mb: f64,
    cpu_seconds: f64,
    stages: Vec<StageReport>,
}

pub struct Bench {
    options: Options,
    stage: Stage,
    launch_ms: Option<f64>,
    samples: Vec<f64>,
    placeholders: usize,
    finished: Vec<StageReport>,
    random: u64,
    frame_started: Instant,
    window_pixels: (f32, f32),
}

impl Bench {
    pub fn new(options: Options) -> Self {
        Self {
            options,
            stage: Stage::WaitingForLog,
            launch_ms: None,
            samples: Vec::new(),
            placeholders: 0,
            finished: Vec::new(),
            random: 0x2545_F491_4F6C_DD1D,
            frame_started: Instant::now(),
            window_pixels: (0.0, 0.0),
        }
    }

    pub fn presentation(&self) -> Presentation {
        self.options.presentation
    }

    pub fn set_window_pixels(&mut self, width: f32, height: f32) {
        self.window_pixels = (width, height);
    }

    pub fn frame_started(&mut self) {
        self.frame_started = Instant::now();
    }

    fn next_random(&mut self) -> u64 {
        self.random ^= self.random << 13;
        self.random ^= self.random >> 7;
        self.random ^= self.random << 17;
        self.random
    }

    fn write_report(
        &mut self,
        presentation: Option<Presentation>,
        source: Option<&dyn RowSource>,
        index_bytes: usize,
    ) {
        let private = metrics::private_bytes();
        let megabytes = |bytes: usize| bytes as f64 / (1024.0 * 1024.0);
        let report = Report {
            presentation: format!("{presentation:?}"),
            window_pixels: self.window_pixels,
            launch_ms: self.launch_ms.unwrap_or(0.0),
            log: self.options.log.as_ref(),
            rows: source.map_or(0, |source| source.row_count()),
            index_mb: megabytes(index_bytes),
            private_mb: megabytes(private),
            overhead_mb: megabytes(private.saturating_sub(index_bytes)),
            cpu_seconds: metrics::cpu_seconds(),
            stages: std::mem::take(&mut self.finished),
        };
        if let Ok(json) = serde_json::to_string_pretty(&report) {
            let _ = std::fs::write(&self.options.report, json);
        }
    }

    /// Records the frame that just finished and sets up the next one.
    pub fn after_frame(
        &mut self,
        view: &mut LogView,
        source: Option<&dyn RowSource>,
        index_bytes: usize,
        presentation: Option<Presentation>,
        commands: &mut Commands,
    ) {
        let frame_ms = self.frame_started.elapsed().as_secs_f64() * 1000.0;
        if self.launch_ms.is_none() {
            self.launch_ms = Some(metrics::ms_since_process_start());
            if self.options.launch_only {
                self.write_report(presentation, source, index_bytes);
                commands.push(Command::Close);
                return;
            }
        }
        match self.stage {
            Stage::WaitingForLog => {
                if source.is_some() {
                    self.stage = Stage::Settling(0);
                }
            }
            Stage::Settling(frames) if frames < 10 => self.stage = Stage::Settling(frames + 1),
            Stage::Settling(_) => self.stage = Stage::Scrolling { stage: 0, frame: 0 },
            Stage::Scrolling { stage, frame } => {
                self.samples.push(frame_ms);
                if let Some(source) = source {
                    if view.visible_rows().any(|row| source.cells(row).is_none()) {
                        self.placeholders += 1;
                    }
                }
                let rows = source.map_or(0, |source| source.row_count());
                let page = view.page_rows() as f64;
                let max_top = (rows as f64 - page).max(1.0);
                match stage {
                    0 => view.scroll_to(view.top_row() as f64 + 3.0),
                    1 => view.scroll_to(view.top_row() as f64 + page),
                    2 => view.scroll_to((self.next_random() % max_top as u64) as f64),
                    _ => view.scroll_to(max_top * frame as f64 / self.options.frames as f64),
                }
                if frame + 1 < self.options.frames {
                    self.stage = Stage::Scrolling {
                        stage,
                        frame: frame + 1,
                    };
                } else {
                    self.finished.push(StageReport {
                        name: STAGES[stage],
                        frames: self.samples.len(),
                        frames_with_placeholders: std::mem::take(&mut self.placeholders),
                        frame_ms: percentiles(std::mem::take(&mut self.samples)),
                    });
                    self.stage = if stage + 1 < STAGES.len() {
                        Stage::Scrolling {
                            stage: stage + 1,
                            frame: 0,
                        }
                    } else {
                        Stage::Done
                    };
                }
            }
            Stage::Done => {
                self.write_report(presentation, source, index_bytes);
                commands.push(Command::Close);
                return;
            }
        }
        commands.redraw();
    }
}
