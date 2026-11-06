//! What the sidebar and start page list besides open tabs: the usual Windows
//! log folders that exist on this machine, their key logs, and the logs in a
//! folder. A folder can be a slow network share, so everything here runs on
//! background threads.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::time::SystemTime;
use tv_platform::Waker;

/// A folder listing stops here, so a folder of thousands of files stays usable.
pub const MOST_LOGS_PER_FOLDER: usize = 500;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub name: &'static str,
    pub folder: PathBuf,
    /// The logs here most worth opening that exist, filled in by [`PlacesJob`].
    pub key_logs: Vec<PathBuf>,
}

/// Each place's logs most worth opening, for the start page to offer.
const KEY_LOGS: [(&str, &[&str]); 8] = [
    (
        "ConfigMgr client",
        &[
            "AppEnforce.log",
            "AppDiscovery.log",
            "CAS.log",
            "ContentTransferManager.log",
            "DataTransferService.log",
            "LocationServices.log",
            "PolicyAgent.log",
            "execmgr.log",
            "UpdatesDeployment.log",
            "WUAHandler.log",
        ],
    ),
    ("ConfigMgr setup", &["ccmsetup.log"]),
    (
        "Intune",
        &[
            "IntuneManagementExtension.log",
            "AgentExecutor.log",
            "AppWorkload.log",
        ],
    ),
    ("CBS", &["CBS.log"]),
    ("DISM", &["dism.log"]),
    ("Windows setup", &["setupact.log", "setuperr.log"]),
    ("Device setup", &["setupapi.dev.log"]),
    ("Windows Update", &["ReportingEvents.log"]),
];

fn place(name: &'static str, folder: PathBuf) -> Place {
    Place {
        name,
        folder,
        key_logs: Vec::new(),
    }
}

/// The key logs of a place that exist. This reads the disk.
fn key_logs_of(place: &Place) -> Vec<PathBuf> {
    KEY_LOGS
        .iter()
        .filter(|(name, _)| *name == place.name)
        .flat_map(|(_, logs)| logs.iter())
        .map(|log| place.folder.join(log))
        .filter(|path| path.is_file())
        .collect()
}

/// A log in a folder listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogEntry {
    pub path: PathBuf,
    pub modified: Option<SystemTime>,
    pub size: u64,
}

fn folder_from(variable: &str, fallback: &str) -> PathBuf {
    std::env::var_os(variable).map_or_else(|| PathBuf::from(fallback), PathBuf::from)
}

/// The usual log folders on Windows, whether or not they exist here.
fn known_places() -> Vec<Place> {
    let windows = folder_from("WINDIR", "C:\\Windows");
    let data = folder_from("ProgramData", "C:\\ProgramData");
    vec![
        place("ConfigMgr client", windows.join("CCM").join("Logs")),
        place("ConfigMgr setup", windows.join("ccmsetup").join("Logs")),
        place(
            "Intune",
            data.join("Microsoft")
                .join("IntuneManagementExtension")
                .join("Logs"),
        ),
        place("CBS", windows.join("Logs").join("CBS")),
        place("DISM", windows.join("Logs").join("DISM")),
        place("Windows setup", windows.join("Panther")),
        place("Device setup", windows.join("INF")),
        place("Windows Update", windows.join("SoftwareDistribution")),
    ]
}

/// Logs are text files with these extensions. `.lo_` is a rotated SCCM log.
fn is_log(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            ["log", "lo_", "txt"]
                .iter()
                .any(|log| extension.eq_ignore_ascii_case(log))
        })
}

/// The end of `text` after removing `suffix` in any case, if it ends with it.
fn strip_suffix_ignoring_case<'a>(text: &'a str, suffix: &str) -> Option<&'a str> {
    let start = text.len().checked_sub(suffix.len())?;
    text.get(start..)
        .filter(|end| end.eq_ignore_ascii_case(suffix))
        .map(|_| &text[..start])
}

/// Whether `text` is digits and separators laid out like `pattern`, where
/// `d` stands for a digit.
fn shaped_like(text: &str, pattern: &str) -> bool {
    text.len() == pattern.len()
        && text.bytes().zip(pattern.bytes()).all(|(byte, wanted)| {
            if wanted == b'd' {
                byte.is_ascii_digit()
            } else {
                byte == wanted
            }
        })
}

/// The name of the current log that a rotated copy was rolled over from, or
/// None for a name that isn't a rotated copy. ConfigMgr and the Intune
/// Management Extension roll `AppEnforce.log` over to `AppEnforce.lo_` or
/// `AppEnforce-20261020-123456.log`, Windows rolls `setupapi.dev.log` over to
/// `setupapi.dev.20261014_095916.log`, and CBS rolls `CBS.log` over to
/// `CbsPersist_20261105172003.log`.
pub fn current_name(file_name: &str) -> Option<String> {
    if let Some(stem) = strip_suffix_ignoring_case(file_name, ".lo_") {
        return Some(format!("{stem}.log"));
    }
    let stem = strip_suffix_ignoring_case(file_name, ".log")?;
    let cbs_stamp = stem
        .get(..11)
        .filter(|start| start.eq_ignore_ascii_case("CbsPersist_"))
        .and(stem.get(11..));
    if cbs_stamp.is_some_and(|stamp| shaped_like(stamp, "dddddddddddddd")) {
        return Some("CBS.log".to_string());
    }
    for stamp in ["-dddddddd-dddddd", ".dddddddd_dddddd"] {
        let Some(start) = stem.len().checked_sub(stamp.len()) else {
            continue;
        };
        if start > 0 && stem.get(start..).is_some_and(|end| shaped_like(end, stamp)) {
            return Some(format!("{}.log", &stem[..start]));
        }
    }
    None
}

/// The logs directly in a folder, newest first.
pub fn logs_in(folder: &Path) -> io::Result<Vec<LogEntry>> {
    let mut logs: Vec<LogEntry> = std::fs::read_dir(folder)?
        .filter_map(Result::ok)
        .filter(|entry| is_log(&entry.path()))
        .filter_map(|entry| {
            let metadata = entry.metadata().ok().filter(std::fs::Metadata::is_file)?;
            Some(LogEntry {
                path: entry.path(),
                modified: metadata.modified().ok(),
                size: metadata.len(),
            })
        })
        .collect();
    logs.sort_by(|first, second| {
        second
            .modified
            .cmp(&first.modified)
            .then_with(|| first.path.cmp(&second.path))
    });
    logs.truncate(MOST_LOGS_PER_FOLDER);
    Ok(logs)
}

/// Finds which known places exist, in the background.
pub struct PlacesJob(Receiver<Vec<Place>>);

impl PlacesJob {
    pub fn start(waker: &Waker) -> Self {
        let (sender, receiver) = channel();
        let waker = waker.clone();
        std::thread::spawn(move || {
            let places = known_places()
                .into_iter()
                .filter(|place| place.folder.is_dir())
                .map(|mut place| {
                    place.key_logs = key_logs_of(&place);
                    place
                })
                .collect();
            let _ = sender.send(places);
            waker.wake();
        });
        Self(receiver)
    }

    pub fn poll(&self) -> Option<Vec<Place>> {
        self.0.try_recv().ok()
    }
}

/// Lists a folder's logs in the background.
pub struct FolderJob {
    result: Receiver<io::Result<Vec<LogEntry>>>,
}

impl FolderJob {
    pub fn start(folder: PathBuf, waker: &Waker) -> Self {
        let (sender, result) = channel();
        let waker = waker.clone();
        std::thread::spawn(move || {
            let _ = sender.send(logs_in(&folder));
            waker.wake();
        });
        Self { result }
    }

    pub fn poll(&self) -> Option<io::Result<Vec<LogEntry>>> {
        match self.result.try_recv() {
            Ok(outcome) => Some(outcome),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                Some(Err(io::Error::other("the folder listing stopped")))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn wait<T>(poll: impl Fn() -> Option<T>) -> T {
        let started = Instant::now();
        loop {
            if let Some(value) = poll() {
                return value;
            }
            assert!(started.elapsed() < Duration::from_secs(5), "never finished");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn only_logs_are_listed_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        for (name, age) in [
            ("old.log", 30),
            ("PolicyAgent.LOG", 10),
            ("rotated.lo_", 20),
            ("notes.txt", 5),
            ("driver.inf", 1),
        ] {
            let path = dir.path().join(name);
            std::fs::write(&path, "x").unwrap();
            let modified = SystemTime::now() - Duration::from_secs(age * 60);
            std::fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(modified)
                .unwrap();
        }
        std::fs::create_dir(dir.path().join("folder.log")).unwrap();
        let job = FolderJob::start(dir.path().to_path_buf(), &Waker::detached());
        let logs = wait(|| job.poll()).unwrap();
        let names: Vec<_> = logs
            .iter()
            .map(|log| log.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            ["notes.txt", "PolicyAgent.LOG", "rotated.lo_", "old.log"]
        );
        assert_eq!(logs[0].size, 1);
    }

    #[test]
    fn a_missing_folder_is_an_error_not_a_crash() {
        let job = FolderJob::start(PathBuf::from("Z:\\does\\not\\exist"), &Waker::detached());
        assert!(wait(|| job.poll()).is_err());
    }

    #[test]
    fn rotated_copies_name_their_current_log() {
        for (rotated, current) in [
            ("AppEnforce.lo_", "AppEnforce.log"),
            ("StateMessage.LO_", "StateMessage.log"),
            ("AppEnforce-20261020-123456.log", "AppEnforce.log"),
            (
                "IntuneManagementExtension-20261101-080910.log",
                "IntuneManagementExtension.log",
            ),
            ("setupapi.dev.20261014_095916.log", "setupapi.dev.log"),
            ("CbsPersist_20261105172003.log", "CBS.log"),
            ("cbspersist_20261105172003.LOG", "CBS.log"),
        ] {
            assert_eq!(current_name(rotated).as_deref(), Some(current), "{rotated}");
        }
    }

    #[test]
    fn current_logs_and_lookalikes_are_not_rotated_copies() {
        for name in [
            "AppEnforce.log",
            "CBS.log",
            "setupapi.dev.log",
            "notes.txt",
            "-20261020-123456.log",
            "AppEnforce-2026102-123456.log",
            "AppEnforce_20261020_123456.log",
            "CbsPersist_2026.log",
            "Ünïcode-ㅤ.log",
        ] {
            assert_eq!(current_name(name), None, "{name}");
        }
    }

    #[test]
    fn huge_folders_are_capped() {
        let dir = tempfile::tempdir().unwrap();
        for number in 0..(MOST_LOGS_PER_FOLDER + 20) {
            std::fs::write(dir.path().join(format!("{number}.log")), "").unwrap();
        }
        assert_eq!(logs_in(dir.path()).unwrap().len(), MOST_LOGS_PER_FOLDER);
    }

    #[test]
    fn known_places_that_exist_are_found() {
        let job = PlacesJob::start(&Waker::detached());
        let places = wait(|| job.poll());
        // Every Windows install has an INF folder.
        assert!(
            places.iter().any(|place| place.name == "Device setup"),
            "{places:?}"
        );
        assert!(places.iter().all(|place| place.folder.is_dir()));
        assert!(places
            .iter()
            .flat_map(|place| &place.key_logs)
            .all(|log| log.is_file()));
    }

    #[test]
    fn every_place_has_key_logs_named() {
        for place in known_places() {
            assert!(
                KEY_LOGS.iter().any(|(name, _)| *name == place.name),
                "{}",
                place.name
            );
        }
    }
}
