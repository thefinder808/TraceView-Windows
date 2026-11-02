//! What the sidebar lists besides open tabs: the usual Windows log folders that
//! exist on this machine, and the logs in a folder. A folder can be a slow
//! network share, so everything here runs on background threads.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::time::SystemTime;
use tv_platform::Waker;

/// A folder listing stops here, so a folder of thousands of files stays usable.
const MOST_LOGS_PER_FOLDER: usize = 500;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub name: &'static str,
    pub folder: PathBuf,
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
        Place {
            name: "ConfigMgr client",
            folder: windows.join("CCM").join("Logs"),
        },
        Place {
            name: "ConfigMgr setup",
            folder: windows.join("ccmsetup").join("Logs"),
        },
        Place {
            name: "Intune",
            folder: data
                .join("Microsoft")
                .join("IntuneManagementExtension")
                .join("Logs"),
        },
        Place {
            name: "CBS",
            folder: windows.join("Logs").join("CBS"),
        },
        Place {
            name: "DISM",
            folder: windows.join("Logs").join("DISM"),
        },
        Place {
            name: "Windows setup",
            folder: windows.join("Panther"),
        },
        Place {
            name: "Device setup",
            folder: windows.join("INF"),
        },
        Place {
            name: "Windows Update",
            folder: windows.join("SoftwareDistribution"),
        },
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
    }
}
