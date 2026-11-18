//! Signs of trouble that ConfigMgr logs write at info level, so a summary's
//! problem patterns, which are warnings and errors, never show them. Each was
//! the clue to a real failure an AI agent troubleshot, as in
//! `Assignment(...) already in progress state (AssignmentStateDetecting)`.
//!
//! They're for `summarize --symptoms` only, as some are common in healthy
//! logs too. Add a sign only from text seen in real logs.

use regex::RegexSetBuilder;
use serde::Serialize;
use tv_engine::query::{self, Control, Matcher, Query};
use tv_engine::{LogFile, QueryError};

/// A sign: a pattern to find, and what it can mean, in our own words.
pub struct Sign {
    /// A short name for it, as lines show it.
    pub name: &'static str,
    /// A regular expression, matched in any case.
    pub pattern: &'static str,
    pub meaning: &'static str,
}

pub const SIGNS: [Sign; 4] = [
    Sign {
        name: "assignment already in progress",
        pattern: r"already in progress state",
        meaning: "An update assignment is still in an earlier state, so it isn't evaluated again. Repeated, the assignment may be stuck.",
    },
    Sign {
        name: "no actionable updates",
        pattern: r"total actionable updates = 0\b",
        meaning: "An evaluation found nothing to install. That's often normal, but if UpdatesStore.log lists an update as Missing, the deployment isn't reaching it.",
    },
    Sign {
        name: "job waiting for execution",
        pattern: r"is waiting for execution",
        meaning: "A job is queued behind others. Repeated for the same job, the queue may be stuck.",
    },
    Sign {
        name: "reconnected to existing job",
        pattern: r"reconnected to the existing job",
        meaning: "A download reconnected to a job already running. Repeated, the download may not be progressing.",
    },
];

/// A sign found in a log.
#[derive(Serialize)]
pub struct Found {
    pub sign: &'static str,
    pub meaning: &'static str,
    /// The log, with its rotated copies counted as part of it.
    pub file: String,
    pub count: u64,
    pub first_seen: Option<String>,
    pub last_seen: Option<String>,
    /// The first row with it, cut to its first line and 500 characters.
    pub example: String,
}

/// Looks for every sign in each log, within the time range `range` gives,
/// and returns those found, most often first. `logs` pairs each log with the
/// name to report it by, so rotated copies can share their log's name.
pub fn find(logs: &[(String, &LogFile)], range: &Query) -> Result<Vec<Found>, QueryError> {
    let patterns: Vec<String> = SIGNS.iter().map(|sign| sign.pattern.to_string()).collect();
    // Each log is scanned once for any sign, and only the rows found are read
    // and sorted into their signs.
    let matcher = Matcher::new(&Query {
        text: Some(query::any_of(&patterns)),
        regex: true,
        from: range.from.clone(),
        to: range.to.clone(),
        ..Query::default()
    })?;
    let each = RegexSetBuilder::new(&patterns)
        .case_insensitive(true)
        .build()
        .expect("the signs' patterns are valid");
    let control = Control::default();
    let mut found: Vec<Found> = Vec::new();
    for (file, log) in logs {
        let rows = query::matching_rows(log, &matcher, &control)?;
        for record in query::read_records(log, &rows)? {
            for index in each.matches(&record.message).iter() {
                let sign = &SIGNS[index];
                let time = record.timestamp.clone();
                match found
                    .iter_mut()
                    .find(|known| known.sign == sign.name && &known.file == file)
                {
                    Some(known) => {
                        known.count += 1;
                        // Times print so that text order is time order.
                        if let Some(time) = time {
                            if known.first_seen.as_ref().is_none_or(|was| time < *was) {
                                known.first_seen = Some(time.clone());
                            }
                            if known.last_seen.as_ref().is_none_or(|was| time > *was) {
                                known.last_seen = Some(time);
                            }
                        }
                    }
                    None => found.push(Found {
                        sign: sign.name,
                        meaning: sign.meaning,
                        file: file.clone(),
                        count: 1,
                        first_seen: time.clone(),
                        last_seen: time,
                        example: String::from_utf8_lossy(query::first_line(
                            record.message.as_bytes(),
                        ))
                        .chars()
                        .take(500)
                        .collect(),
                    }),
                }
            }
        }
    }
    found.sort_by(|a, b| b.count.cmp(&a.count).then(a.file.cmp(&b.file)));
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    fn log_of(text: &str) -> (tempfile::TempDir, LogFile) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("UpdatesDeployment.log");
        std::fs::write(&path, text).unwrap();
        let (log, _) = LogFile::open_with_cache(&path, None, &AtomicU64::new(0)).unwrap();
        (dir, log)
    }

    #[test]
    fn each_sign_finds_the_text_it_was_seen_in() {
        // Rows as the agent saw them, then near misses that aren't signs.
        let (_dir, log) = log_of(concat!(
            "2026-11-16 08:00:00 Assignment({8B24B027}) already in progress state (AssignmentStateDetecting). No need to evaluate\n",
            "2026-11-16 08:01:00 EnumerateUpdates for action (UpdateActionInstall) - Total actionable updates = 0\n",
            "2026-11-16 08:02:00 Job {AB12-CD34} is waiting for execution\n",
            "2026-11-16 08:03:00 CTM job {AB12} reconnected to the existing job\n",
            "2026-11-16 08:04:00 EnumerateUpdates for action (UpdateActionInstall) - Total actionable updates = 03\n",
            "2026-11-16 08:05:00 Assignment evaluation started\n",
        ));
        let found = find(
            &[("UpdatesDeployment.log".to_string(), &log)],
            &Query::default(),
        )
        .unwrap();
        let mut signs: Vec<&str> = found.iter().map(|found| found.sign).collect();
        signs.sort_unstable();
        let mut expected: Vec<&str> = SIGNS.iter().map(|sign| sign.name).collect();
        expected.sort_unstable();
        assert_eq!(signs, expected, "each sign once, and = 03 isn't = 0");
        assert!(found.iter().all(|found| found.count == 1));
        let waiting = found
            .iter()
            .find(|found| found.sign == "job waiting for execution")
            .unwrap();
        assert_eq!(waiting.example, "Job {AB12-CD34} is waiting for execution");
        assert_eq!(
            waiting.last_seen.as_deref(),
            Some("2026-11-16T08:02:00.000")
        );

        // A time range only looks within it.
        let late = Query {
            from: Some("2026-11-16T08:02".to_string()),
            ..Query::default()
        };
        let found = find(&[("UpdatesDeployment.log".to_string(), &log)], &late).unwrap();
        assert_eq!(found.len(), 2);
    }
}
