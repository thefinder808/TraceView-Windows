//! Scripted benchmark: drives the real UI through scroll and filter stages,
//! then writes a JSON report and a screenshot beside it.

use crate::metrics;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

pub const FILTERS: [(&str, &str, bool); 3] = [
    ("literal_rare", "hash mismatch", true),
    ("regex_error_code", r"error 0x[0-9a-fA-F]{8}", false),
    ("regex_miss", r"KB\d+: never appears", false),
];

#[derive(Clone, Copy, PartialEq)]
pub enum Stage {
    Loading,
    Settling(usize),
    AwaitScreenshot,
    Wheel,
    PageDown,
    RandomJump,
    Sweep,
    Filter(usize),
    Done,
}

#[derive(Default)]
struct Samples {
    frame_ms: Vec<f64>,
    ui_ms: Vec<f64>,
}

pub struct Bench {
    pub output: PathBuf,
    pub stage: Stage,
    pub screenshot_done: Arc<AtomicBool>,
    pub launch_ms: Option<f64>,
    pub open_seconds: f64,
    pub cache_hit: bool,
    pub adapter: String,
    pub file: String,
    pub rows: usize,
    pub index_bytes: usize,
    pub frames_per_stage: usize,
    stage_frame: usize,
    samples: Samples,
    random: u64,
    report: Vec<String>,
    last_frame: Option<Instant>,
}

impl Bench {
    pub fn new(output: PathBuf, frames_per_stage: usize) -> Self {
        Self {
            output,
            stage: Stage::Loading,
            screenshot_done: Arc::new(AtomicBool::new(false)),
            launch_ms: None,
            open_seconds: 0.0,
            cache_hit: false,
            adapter: String::new(),
            file: String::new(),
            rows: 0,
            index_bytes: 0,
            frames_per_stage,
            stage_frame: 0,
            samples: Samples::default(),
            random: 0x2545_F491_4F6C_DD1D,
            report: Vec::new(),
            last_frame: None,
        }
    }

    pub fn next_random(&mut self) -> u64 {
        self.random ^= self.random << 13;
        self.random ^= self.random >> 7;
        self.random ^= self.random << 17;
        self.random
    }

    pub fn screenshot_path(&self) -> PathBuf {
        self.output.with_extension("png")
    }

    pub fn screenshot_ready(&self) -> bool {
        self.screenshot_done.load(Ordering::Acquire)
    }

    /// Advances to the next scroll stage after enough frames.
    pub fn stage_finished(&mut self) -> bool {
        self.stage_frame += 1;
        self.stage_frame >= self.frames_per_stage
    }

    pub fn record_frame(&mut self, ui_ms: f64) {
        let now = Instant::now();
        if let Some(last) = self.last_frame {
            self.samples.frame_ms.push((now - last).as_secs_f64() * 1000.0);
            self.samples.ui_ms.push(ui_ms);
        }
        self.last_frame = Some(now);
    }

    pub fn finish_stage(&mut self, name: &str, next: Stage) {
        let samples = std::mem::take(&mut self.samples);
        self.report.push(format!(
            "    \"{name}\": {{ \"frames\": {}, \"frame_ms\": {}, \"ui_ms\": {} }}",
            samples.frame_ms.len(),
            summary(samples.frame_ms),
            summary(samples.ui_ms),
        ));
        self.stage_frame = 0;
        self.stage = next;
    }

    pub fn finish_filter(&mut self, name: &str, seconds: f64, matches: usize, next: Stage) {
        let samples = std::mem::take(&mut self.samples);
        self.report.push(format!(
            "    \"filter_{name}\": {{ \"seconds\": {seconds:.3}, \"matches\": {matches}, \"frames_while_running\": {}, \"frame_ms\": {} }}",
            samples.frame_ms.len(),
            summary(samples.frame_ms),
        ));
        self.stage_frame = 0;
        self.stage = next;
    }

    pub fn write_report(&self) {
        let (working_set, private) = metrics::memory_bytes();
        let mut json = String::from("{\n");
        let launch = self
            .launch_ms
            .map_or("null".to_string(), |launch| format!("{launch:.1}"));
        let _ = writeln!(json, "  \"file\": {:?},", self.file);
        let _ = writeln!(json, "  \"adapter\": {:?},", self.adapter);
        let _ = writeln!(json, "  \"launch_to_second_frame_ms\": {launch},");
        let marks: Vec<String> = metrics::marks()
            .iter()
            .map(|(name, ms)| format!("\"{name}\": {ms:.1}"))
            .collect();
        let _ = writeln!(json, "  \"startup_marks_ms\": {{ {} }},", marks.join(", "));
        let _ = writeln!(json, "  \"rows\": {},", self.rows);
        let _ = writeln!(json, "  \"open_seconds\": {:.3},", self.open_seconds);
        let _ = writeln!(json, "  \"cache_hit\": {},", self.cache_hit);
        let _ = writeln!(json, "  \"index_mb\": {:.1},", mib(self.index_bytes));
        let _ = writeln!(json, "  \"working_set_mb\": {:.1},", mib(working_set));
        let _ = writeln!(json, "  \"private_mb\": {:.1},", mib(private));
        json.push_str("  \"stages\": {\n");
        json.push_str(&self.report.join(",\n"));
        json.push_str("\n  }\n}\n");
        let _ = std::fs::write(&self.output, json);
    }
}

fn mib(bytes: usize) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn summary(mut samples: Vec<f64>) -> String {
    if samples.is_empty() {
        return "null".into();
    }
    samples.sort_by(f64::total_cmp);
    let percentile = |share: f64| samples[((samples.len() - 1) as f64 * share) as usize];
    format!(
        "{{ \"p50\": {:.2}, \"p95\": {:.2}, \"p99\": {:.2}, \"max\": {:.2} }}",
        percentile(0.50),
        percentile(0.95),
        percentile(0.99),
        samples[samples.len() - 1],
    )
}
