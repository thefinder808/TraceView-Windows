//! When a log, or several logs, had rows: stretches of time, and the gaps of a
//! day or more between them, such as a PC that was off or asleep for weeks.
//!
//! Rows less than a day apart are one stretch. Stretches are only sorted and
//! joined when needed, so a row written out of time order adds a stretch of its
//! own rather than a false gap, and one log's stretches join another's.

use crate::time::Stamp;
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;

/// The shortest gap listed: rows closer together than this are one stretch.
pub const MIN_GAP_SECONDS: u32 = 86_400;
/// Gaps listed: the longest ones, shown in time order.
pub const MAX_GAPS: usize = 5;
/// Stretches held before they're joined, which bounds memory for a log whose
/// times jump back and forth.
const JOIN_AT: usize = 4096;

/// Whether `seconds` is less than a day from the stretch `(first, last)`.
fn near((first, last): (u32, u32), seconds: u32) -> bool {
    let day = u64::from(MIN_GAP_SECONDS);
    u64::from(seconds) + day > u64::from(first) && u64::from(seconds) < u64::from(last) + day
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Activity {
    /// Stretches of time with rows, as their first and last seconds. They may
    /// overlap or be out of order until joined.
    stretches: Vec<(u32, u32)>,
}

impl Activity {
    /// Notes a row written at `seconds`.
    pub fn add(&mut self, seconds: u32) {
        if let Some(stretch) = self.stretches.last_mut() {
            if near(*stretch, seconds) {
                stretch.0 = stretch.0.min(seconds);
                stretch.1 = stretch.1.max(seconds);
                return;
            }
        }
        self.stretches.push((seconds, seconds));
        if self.stretches.len() >= JOIN_AT {
            self.join();
        }
    }

    /// Adds another log's rows, for a summary of several logs.
    pub fn absorb(&mut self, other: &Activity) {
        self.stretches.extend_from_slice(&other.stretches);
        self.join();
    }

    /// Sorts the stretches and joins those less than a day apart.
    fn join(&mut self) {
        self.stretches.sort_unstable();
        let mut joined: Vec<(u32, u32)> = Vec::with_capacity(self.stretches.len());
        for &(first, last) in &self.stretches {
            match joined.last_mut() {
                Some(previous) if near(*previous, first) => previous.1 = previous.1.max(last),
                _ => joined.push((first, last)),
            }
        }
        self.stretches = joined;
    }

    /// The longest gaps of a day or more between rows, at most [`MAX_GAPS`] of
    /// them, in time order, and how many more there were.
    pub fn gaps(&self) -> (Vec<Gap>, usize) {
        let mut joined = self.clone();
        joined.join();
        let mut gaps: Vec<(u32, u32)> = joined
            .stretches
            .windows(2)
            .map(|pair| (pair[0].1, pair[1].0))
            .collect();
        let total = gaps.len();
        // A stable sort, so equal gaps keep their time order.
        gaps.sort_by_key(|(from, to)| Reverse(to - from));
        gaps.truncate(MAX_GAPS);
        gaps.sort_unstable();
        let listed = gaps.into_iter().map(Gap::new).collect::<Vec<_>>();
        let more = total - listed.len();
        (listed, more)
    }
}

/// A day or more with no rows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Gap {
    /// The time of the last row before it.
    pub from: String,
    /// The time of the first row after it.
    pub to: String,
    /// How long it was, to a tenth of a day.
    pub days: f64,
}

impl Gap {
    fn new((from, to): (u32, u32)) -> Self {
        Self {
            from: Stamp::from_seconds(from).to_string(),
            to: Stamp::from_seconds(to).to_string(),
            days: (f64::from(to - from) / 8_640.0).round() / 10.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u32 = MIN_GAP_SECONDS;

    fn activity(times: &[u32]) -> Activity {
        let mut activity = Activity::default();
        for time in times {
            activity.add(*time);
        }
        activity
    }

    fn spans(activity: &Activity) -> Vec<(u32, u32)> {
        let (gaps, _) = activity.gaps();
        gaps.iter()
            .map(|gap| {
                let seconds =
                    |text: &str| crate::time::parse_user_time(&text[..19]).unwrap().seconds;
                (seconds(&gap.from), seconds(&gap.to))
            })
            .collect()
    }

    #[test]
    fn rows_less_than_a_day_apart_leave_no_gap() {
        let nights = activity(&[0, 3600, DAY - 1, 2 * DAY - 2, 3 * DAY - 3]);
        assert_eq!(nights.gaps(), (Vec::new(), 0));
    }

    #[test]
    fn a_day_or_more_without_rows_is_a_gap() {
        let start = 1_000_000;
        let found = activity(&[start, start + 60, start + 60 + DAY, start + 20 * DAY]);
        assert_eq!(
            spans(&found),
            [
                (start + 60, start + 60 + DAY),
                (start + 60 + DAY, start + 20 * DAY)
            ]
        );
        let (gaps, more) = found.gaps();
        assert_eq!(more, 0);
        assert_eq!(gaps[0].days, 1.0);
        assert_eq!(gaps[1].days, 19.0);
        assert!(gaps[1].from.ends_with(".000"), "{}", gaps[1].from);
    }

    #[test]
    fn a_row_out_of_time_order_makes_no_false_gap() {
        // Rows on days 0 and 30, a stray from day 15 between them, then day 30
        // again: the gaps are 0 to 15 and 15 to 30, not 15 to 30 twice.
        let found = activity(&[0, 30 * DAY, 15 * DAY, 30 * DAY + 5]);
        assert_eq!(spans(&found), [(0, 15 * DAY), (15 * DAY, 30 * DAY)]);
        // A stray inside a stretch changes nothing.
        let inside = activity(&[0, 600, 300, 900]);
        assert_eq!(inside.gaps(), (Vec::new(), 0));
    }

    #[test]
    fn several_logs_only_have_gaps_none_of_them_filled() {
        let mut first = activity(&[0, 10 * DAY]);
        let second = activity(&[5 * DAY, 6 * DAY - 1, 20 * DAY]);
        first.absorb(&second);
        // The second log fills days 5 to 6, so the first log's gap is split.
        assert_eq!(
            spans(&first),
            [(0, 5 * DAY), (6 * DAY - 1, 10 * DAY), (10 * DAY, 20 * DAY)]
        );
    }

    #[test]
    fn only_the_longest_gaps_are_listed_in_time_order() {
        // Gaps of 2, 9, 3, 8, 4, 7 and 5 days.
        let mut at = 0;
        let mut times = vec![0];
        for days in [2, 9, 3, 8, 4, 7, 5] {
            at += days * DAY;
            times.push(at);
        }
        let (gaps, more) = activity(&times).gaps();
        assert_eq!(more, 2);
        let lengths: Vec<f64> = gaps.iter().map(|gap| gap.days).collect();
        assert_eq!(lengths, [9.0, 8.0, 4.0, 7.0, 5.0]);
    }

    #[test]
    fn times_jumping_back_and_forth_stay_bounded() {
        let mut found = Activity::default();
        for row in 0..100_000u32 {
            let base = if row % 2 == 0 { 0 } else { 100 * DAY };
            found.add(base + row);
        }
        assert!(found.stretches.len() < JOIN_AT);
        assert_eq!(found.gaps().0.len(), 1);
    }
}
