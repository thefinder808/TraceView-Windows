//! The severity chips and event histogram above the grid, as in the Mac app.
//! Both cover the whole log and come from the index alone, counted on a
//! background thread when a log opens and at most once a second while live
//! tail adds rows.

use super::row_cache::SharedLog;
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tv_engine::overview::{self, Histogram, SpikeMemory, TimeRange};
use tv_engine::time::Stamp;
use tv_engine::{Control, Level, LevelCounts};
use tv_platform::Waker;
use tv_ui::chip_bar::Chip;
use tv_ui::{Align, Canvas, Color, FontRole, Rect, TextStyle, Theme};

/// Buckets in the histogram, as in the Mac app.
pub const BUCKETS: usize = 60;
pub const BAR_HEIGHT: f32 = 28.0;
pub const LABEL_HEIGHT: f32 = 16.0;
/// Live tail recounts at most this often.
const RECOUNT: Duration = Duration::from_secs(1);
/// The levels with chips, most severe first.
pub const CHIP_LEVELS: [Level; 6] = [
    Level::Critical,
    Level::Error,
    Level::Warning,
    Level::Notice,
    Level::Info,
    Level::Debug,
];
const LABEL_LEFT: TextStyle = TextStyle::new(FontRole::Ui, Align::Left);
const LABEL_RIGHT: TextStyle = TextStyle::new(FontRole::Ui, Align::Right);

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Counts {
    pub levels: LevelCounts,
    /// `None` when the log has no times, or all of them are the same second.
    pub histogram: Option<Histogram>,
}

/// Counts every file of a tab together. A merged tab's histogram is the sum of
/// one per file over the range they cover together.
fn count(logs: &[SharedLog], control: &Control) -> Result<Counts, String> {
    let guards = logs
        .iter()
        .map(|log| log.read())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "A log is unavailable".to_string())?;
    let mut levels = LevelCounts::default();
    let mut range: Option<TimeRange> = None;
    for log in &guards {
        levels.absorb(
            &overview::level_counts(log, None, control).map_err(|error| error.to_string())?,
        );
        if let Some(found) =
            overview::time_range(log, None, control).map_err(|error| error.to_string())?
        {
            range = Some(range.map_or(found, |range| range.union(found)));
        }
    }
    let mut histogram: Option<Histogram> = None;
    if let Some(range) = range.filter(|range| range.first < range.last) {
        for log in &guards {
            let part = overview::histogram(log, None, range, BUCKETS, control)
                .map_err(|error| error.to_string())?;
            match &mut histogram {
                Some(total) => total.absorb(&part),
                None => histogram = Some(part),
            }
        }
    }
    Ok(Counts { levels, histogram })
}

/// A count running on another thread.
struct CountJob {
    control: Arc<Control>,
    result: Receiver<Result<Counts, String>>,
}

/// Keeps a tab's counts current.
pub struct Overview {
    pub counts: Counts,
    /// For each bucket, the height an earlier spike reached there, or 0.
    pub shadows: Vec<u64>,
    spikes: SpikeMemory,
    job: Option<CountJob>,
    stale: bool,
    counted_at: Option<Instant>,
    /// A wake is already set for when a recount is allowed.
    wake_set: bool,
}

impl Default for Overview {
    fn default() -> Self {
        Self {
            counts: Counts::default(),
            shadows: Vec::new(),
            spikes: SpikeMemory::default(),
            job: None,
            stale: true,
            counted_at: None,
            wake_set: false,
        }
    }
}

impl Drop for Overview {
    fn drop(&mut self) {
        if let Some(job) = &self.job {
            job.control
                .cancel
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

impl Overview {
    /// The log changed. A replaced file forgets earlier spikes.
    pub fn changed(&mut self, replaced: bool) {
        self.stale = true;
        if replaced {
            self.spikes = SpikeMemory::default();
        }
    }

    /// Starts a count if the log changed and one is allowed now. Otherwise
    /// it arranges a wake for when it will be.
    pub fn update(&mut self, logs: &[SharedLog], waker: &Waker) {
        if self.job.is_some() || !self.stale {
            return;
        }
        if let Some(wait) = self
            .counted_at
            .and_then(|at| RECOUNT.checked_sub(at.elapsed()))
        {
            if !self.wake_set {
                self.wake_set = true;
                let waker = waker.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(wait);
                    waker.wake();
                });
            }
            return;
        }
        let control = Arc::new(Control::default());
        let (sender, result) = channel();
        let (job_control, logs, waker) = (control.clone(), logs.to_vec(), waker.clone());
        std::thread::spawn(move || {
            let _ = sender.send(count(&logs, &job_control));
            waker.wake();
        });
        self.job = Some(CountJob { control, result });
        self.stale = false;
        self.wake_set = false;
        self.counted_at = Some(Instant::now());
    }

    /// Picks up a finished count. Returns true if the counts changed.
    pub fn poll(&mut self) -> bool {
        let Some(job) = &self.job else {
            return false;
        };
        let outcome = match job.result.try_recv() {
            Ok(outcome) => outcome,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => Err("The count stopped".to_string()),
        };
        self.job = None;
        let Ok(counts) = outcome else { return false };
        if let Some(old) = &self.counts.histogram {
            self.spikes.remember(old);
        }
        self.shadows = counts
            .histogram
            .as_ref()
            .map_or_else(Vec::new, |histogram| self.spikes.shadows(histogram));
        self.counts = counts;
        true
    }
}

fn level_label(level: Level) -> &'static str {
    match level {
        Level::Critical => "Critical",
        Level::Error => "Error",
        Level::Warning => "Warning",
        Level::Notice => "Notice",
        Level::Info => "Info",
        Level::Debug => "Debug",
    }
}

fn level_color(level: Level, theme: &Theme) -> Color {
    match level {
        Level::Critical | Level::Error => theme.histogram_error,
        Level::Warning => theme.histogram_warning,
        Level::Notice => theme.notice_text,
        Level::Debug => theme.debug_text,
        Level::Info => theme.primary_text,
    }
}

/// The chips to show: All, then each level the log has, plus the active one.
/// Returns the chips and the level each one filters to.
pub fn chips(
    levels: &LevelCounts,
    active: Option<Level>,
    theme: &Theme,
) -> (Vec<Chip>, Vec<Option<Level>>) {
    let mut chips = vec![Chip {
        label: "All".to_string(),
        count: levels.total() as usize,
        color: theme.primary_text,
        active: active.is_none(),
    }];
    let mut filters = vec![None];
    for level in CHIP_LEVELS {
        let count = levels.get(level) as usize;
        if count > 0 || active == Some(level) {
            chips.push(Chip {
                label: level_label(level).to_string(),
                count,
                color: level_color(level, theme),
                active: active == Some(level),
            });
            filters.push(Some(level));
        }
    }
    (chips, filters)
}

/// Mixes `color` into `background`, `amount` of the way.
fn mix(color: Color, background: Color, amount: f32) -> Color {
    let channel = |top: u8, bottom: u8| {
        (f32::from(bottom) + (f32::from(top) - f32::from(bottom)) * amount).round() as u8
    };
    Color {
        r: channel(color.r, background.r),
        g: channel(color.g, background.g),
        b: channel(color.b, background.b),
    }
}

/// Where the bars go within the histogram's area, leaving room for labels.
pub fn bars_rect(area: Rect) -> Rect {
    Rect::new(area.left, area.top, area.right, area.top + BAR_HEIGHT)
}

/// The bucket under an x position, if any.
pub fn bucket_at(area: Rect, buckets: usize, x: f32) -> Option<usize> {
    let bars = bars_rect(area);
    if buckets == 0 || x < bars.left || x >= bars.right {
        return None;
    }
    Some((((x - bars.left) / bars.width() * buckets as f32) as usize).min(buckets - 1))
}

/// Labels a time as the histogram does: the time of day when the log spans a
/// day or less, otherwise the date and minute.
fn time_label(seconds: u32, short: bool) -> String {
    let label = Stamp::from_seconds(seconds).label();
    if short {
        label[11..].to_string()
    } else {
        label[..16].to_string()
    }
}

/// Draws the bars, stacked with errors on top and warnings below, and behind
/// them the shadows of earlier spikes. The start and end times go underneath.
pub fn draw_histogram(
    canvas: &mut dyn Canvas,
    area: Rect,
    histogram: &Histogram,
    shadows: &[u64],
    hovered: Option<usize>,
    theme: &Theme,
) {
    let bars = bars_rect(area);
    canvas.fill_rect(area, theme.panel_background);
    canvas.fill_rect(bars, theme.histogram_background);
    let buckets = &histogram.buckets;
    let tallest = buckets
        .iter()
        .map(LevelCounts::total)
        .chain(shadows.iter().copied())
        .max()
        .unwrap_or(0)
        .max(1) as f32;
    let width = bars.width() / buckets.len().max(1) as f32;
    let height = |count: u64| {
        (count as f32 / tallest * (bars.height() - 2.0)).max(if count > 0 { 1.0 } else { 0.0 })
    };
    let shadow_color = mix(theme.histogram_error, theme.histogram_background, 0.25);
    for (index, bucket) in buckets.iter().enumerate() {
        let left = bars.left + index as f32 * width;
        let right = left + (width - 1.0).max(1.0);
        if hovered == Some(index) {
            canvas.fill_rect(
                Rect::new(left, bars.top, left + width, bars.bottom),
                theme.histogram_window,
            );
        }
        if let Some(shadow) = shadows
            .get(index)
            .copied()
            .filter(|shadow| *shadow > bucket.total())
        {
            canvas.fill_rect(
                Rect::new(left, bars.bottom - height(shadow), right, bars.bottom),
                shadow_color,
            );
        }
        let errors = bucket.get(Level::Error) + bucket.get(Level::Critical);
        let warnings = bucket.get(Level::Warning);
        let rest = bucket.total() - errors - warnings;
        let mut bottom = bars.bottom;
        for (count, color) in [
            (rest, theme.histogram_info),
            (warnings, theme.histogram_warning),
            (errors, theme.histogram_error),
        ] {
            if count > 0 {
                let top = bottom - height(count);
                canvas.fill_rect(Rect::new(left, top, right, bottom), color);
                bottom = top;
            }
        }
    }
    // The trailing edge, where new rows arrive.
    canvas.fill_rect(
        Rect::new(bars.right - 1.5, bars.top, bars.right, bars.bottom),
        theme.accent,
    );

    let range = histogram.range;
    let short = range.last - range.first <= 86_400;
    let labels = Rect::new(area.left + 4.0, bars.bottom, area.right - 4.0, area.bottom);
    canvas.text(
        &time_label(range.first, short),
        labels,
        LABEL_LEFT,
        theme.secondary_text,
    );
    canvas.text(
        &time_label(range.last, short),
        labels,
        LABEL_RIGHT,
        theme.secondary_text,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;
    use std::sync::RwLock;
    use tv_engine::LogFile;
    use tv_ui::recording::RecordingCanvas;

    fn shared(dir: &tempfile::TempDir, name: &str, text: &str) -> SharedLog {
        let path = dir.path().join(name);
        std::fs::write(&path, text).unwrap();
        let (log, _) = LogFile::open_with_cache(&path, None, &AtomicU64::new(0)).unwrap();
        Arc::new(RwLock::new(log))
    }

    fn timed(minutes: u32, level: &str) -> String {
        (0..minutes)
            .map(|minute| format!("2026-01-01 10:{minute:02}:00 host app[1]: {level}: message\n"))
            .collect()
    }

    fn wait(overview: &mut Overview) {
        let started = Instant::now();
        while !overview.poll() {
            assert!(started.elapsed() < Duration::from_secs(5), "never counted");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn counts_cover_every_file_of_a_tab() {
        let dir = tempfile::tempdir().unwrap();
        let logs = [
            shared(&dir, "a.log", &timed(30, "ERROR")),
            shared(&dir, "b.log", &timed(10, "INFO")),
        ];
        let mut overview = Overview::default();
        overview.update(&logs, &Waker::detached());
        wait(&mut overview);
        let counts = &overview.counts;
        assert_eq!(
            (
                counts.levels.get(Level::Error),
                counts.levels.get(Level::Info)
            ),
            (30, 10)
        );
        let histogram = counts.histogram.as_ref().unwrap();
        let total: u64 = histogram.buckets.iter().map(LevelCounts::total).sum();
        assert_eq!(total, 40);
        assert!(histogram.buckets.len() <= BUCKETS);
    }

    #[test]
    fn a_log_without_times_has_counts_but_no_histogram() {
        let dir = tempfile::tempdir().unwrap();
        let logs = [shared(&dir, "a.log", "one\ntwo error\n")];
        let mut overview = Overview::default();
        overview.update(&logs, &Waker::detached());
        wait(&mut overview);
        assert_eq!(overview.counts.levels.total(), 2);
        assert_eq!(overview.counts.histogram, None);
    }

    #[test]
    fn recounts_wait_for_a_second_to_pass() {
        let dir = tempfile::tempdir().unwrap();
        let logs = [shared(&dir, "a.log", &timed(5, "INFO"))];
        let mut overview = Overview::default();
        overview.update(&logs, &Waker::detached());
        wait(&mut overview);
        overview.changed(false);
        overview.update(&logs, &Waker::detached());
        assert!(overview.job.is_none(), "too soon");
        std::thread::sleep(RECOUNT);
        overview.update(&logs, &Waker::detached());
        assert!(overview.job.is_some());
    }

    #[test]
    fn chips_show_all_and_each_level_present() {
        let mut levels = LevelCounts::default();
        for level in [Level::Error, Level::Error, Level::Info] {
            levels.add(level);
        }
        let theme = &Theme::CLASSIC;
        let (chips, filters) = chips(&levels, Some(Level::Warning), theme);
        let labels: Vec<_> = chips
            .iter()
            .map(|chip| (chip.label.as_str(), chip.count, chip.active))
            .collect();
        assert_eq!(
            labels,
            [
                ("All", 3, false),
                ("Error", 2, false),
                ("Warning", 0, true),
                ("Info", 1, false)
            ]
        );
        assert_eq!(
            filters,
            [
                None,
                Some(Level::Error),
                Some(Level::Warning),
                Some(Level::Info)
            ]
        );
    }

    #[test]
    fn buckets_are_found_under_the_mouse() {
        let area = Rect::new(100.0, 0.0, 700.0, BAR_HEIGHT + LABEL_HEIGHT);
        assert_eq!(bucket_at(area, 60, 100.0), Some(0));
        assert_eq!(bucket_at(area, 60, 699.0), Some(59));
        assert_eq!(bucket_at(area, 60, 99.0), None);
        assert_eq!(bucket_at(area, 0, 300.0), None);
    }

    #[test]
    fn the_histogram_draws_its_times_and_balances_clips() {
        let mut buckets = vec![LevelCounts::default(); 60];
        buckets[10].add(Level::Error);
        let histogram = Histogram {
            range: TimeRange {
                first: 1000,
                last: 4600,
            },
            buckets,
        };
        let mut canvas = RecordingCanvas::new();
        let area = Rect::new(0.0, 0.0, 600.0, BAR_HEIGHT + LABEL_HEIGHT);
        draw_histogram(
            &mut canvas,
            area,
            &histogram,
            &[0; 60],
            Some(10),
            &Theme::CLASSIC,
        );
        assert_eq!(canvas.texts().len(), 2);
        assert!(canvas.texts()[0].contains(':'));
        assert!(canvas.clips_balance());
    }
}
