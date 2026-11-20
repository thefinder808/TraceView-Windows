//! Auto-updates. Once a day, and from Help > Check for updates, the viewer
//! looks for a newer full release on GitHub. The copy `traceview setup`
//! installed is replaced in the background, and the new version starts the
//! next time TraceView does. The command line never checks, so a command an
//! agent is running is never touched.
//!
//! A download replaces anything only once it matches the release's
//! `SHA256SUMS.txt`, Windows trusts its signature, it's signed by the same
//! publisher, with the same root, as the copy that's running, and the
//! version inside it is the one its name says. Only a newer version is ever
//! taken. A copy that isn't signed, such as one built from source, never
//! checks, and a signed copy kept anywhere else is only told about a new
//! version. A setting and an administrator's policy turn checks off.
//!
//! Anything running as the user could already replace `traceview.exe`, so
//! these checks guard the download, not the folder.

use crate::setup;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tv_platform::updates::{self as platform, Signer};

const HOST: &str = "github.com";
const REPOSITORY: &str = "/thefinder808/TraceView-Windows";
/// A release's checksums are a few hundred bytes, and its EXE a few MB.
const MOST_SUMS_BYTES: usize = 64 * 1024;
const MOST_EXE_BYTES: usize = 64 * 1024 * 1024;
/// A little under a day, so opening TraceView at about the same time each
/// day still checks daily.
const CHECK_EVERY: Duration = Duration::from_secs(20 * 60 * 60);
/// Other processes may read a file being checked, or rename it, but not
/// change it.
const FILE_SHARE_READ: u32 = 0x1;
const FILE_SHARE_DELETE: u32 = 0x4;

#[cfg(target_arch = "aarch64")]
const ARCH: &str = "arm64";
#[cfg(not(target_arch = "aarch64"))]
const ARCH: &str = "x64";

/// How a check went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing to say, as when an automatic check wasn't due, was turned off
    /// or couldn't reach GitHub.
    Quiet,
    UpToDate,
    /// This version was installed, and starts next time.
    Installed(String),
    /// This version is out, for a copy that updates don't replace.
    Available(String),
    TurnedOffByPolicy,
    /// This copy isn't a signed release, so it doesn't update itself.
    NotSigned,
    Failed(String),
}

/// What the latest release lists for this PC.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    pub version: [u16; 3],
    pub file_name: String,
    pub sha256: String,
}

impl Listed {
    pub fn version_text(&self) -> String {
        let [major, minor, patch] = self.version;
        format!("{major}.{minor}.{patch}")
    }
}

/// A release's page on GitHub, which has its notes.
pub fn release_page(version: &str) -> String {
    format!("https://{HOST}{REPOSITORY}/releases/tag/v{version}")
}

/// `0.13.1` as numbers.
pub fn parse_version(text: &str) -> Option<[u16; 3]> {
    let mut parts = text.split('.');
    // Digits only, as `parse` would also take `+13`.
    let mut next = || {
        let part = parts.next()?;
        let digits = !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
        digits.then(|| part.parse::<u16>().ok()).flatten()
    };
    let version = [next()?, next()?, next()?];
    parts.next().is_none().then_some(version)
}

/// This PC's EXE in a release's `SHA256SUMS.txt`, whose lines read
/// `<sha256>  TraceView-0.13.0-x64.exe`.
pub fn listed_exe(sums: &str, arch: &str) -> Option<Listed> {
    sums.lines().find_map(|line| {
        let (sha256, file_name) = line.trim().split_once("  ")?;
        let version = file_name
            .strip_prefix("TraceView-")?
            .strip_suffix(&format!("-{arch}.exe"))?;
        let is_hex = sha256.len() == 64 && sha256.bytes().all(|byte| byte.is_ascii_hexdigit());
        Some(Listed {
            version: parse_version(version)?,
            file_name: file_name.to_string(),
            sha256: is_hex.then(|| sha256.to_ascii_lowercase())?,
        })
    })
}

/// Downloads come only from GitHub, or the servers it redirects release
/// downloads to.
pub fn from_github(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = host.to_ascii_lowercase();
    host == HOST || host.ends_with(".githubusercontent.com")
}

/// When the last automatic check started, as this file's time, so a check
/// that fails isn't tried again at every launch.
fn marker() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")?;
    Some(PathBuf::from(base).join("TraceView").join("update-checked"))
}

/// Whether a check is due, from when the last one started.
pub fn is_due(last: Option<SystemTime>, now: SystemTime) -> bool {
    match last {
        // A time in the future, from a changed clock, doesn't hold checks off.
        Some(last) => now
            .duration_since(last)
            .map_or(true, |since| since >= CHECK_EVERY),
        None => true,
    }
}

fn last_check() -> Option<SystemTime> {
    fs::metadata(marker()?).ok()?.modified().ok()
}

fn mark_checked() {
    if let Some(marker) = marker() {
        if let Some(folder) = marker.parent() {
            let _ = fs::create_dir_all(folder);
        }
        let _ = fs::write(marker, b"");
    }
}

/// Removes what an earlier update left beside the installed copy: the copy
/// it replaced, which may still have been running, and any download it
/// didn't finish.
pub fn clean_up(installed: &Path) {
    let Some(folder) = installed.parent() else {
        return;
    };
    let _ = fs::remove_file(folder.join("traceview.exe.old"));
    if let Ok(entries) = fs::read_dir(folder) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if name.starts_with("traceview.exe.") && name.ends_with(".download") {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

/// Checks for a newer version, and installs it where it can. `manual` is a
/// check someone asked for, which says how it went whatever happens. An
/// automatic one runs at most once a day, only while `automatic` is on, and
/// says only what was installed or is out.
pub fn check(manual: bool, automatic: bool) -> Outcome {
    let quiet_unless = |outcome: Outcome| if manual { outcome } else { Outcome::Quiet };
    if platform::turned_off_by_policy() {
        return quiet_unless(Outcome::TurnedOffByPolicy);
    }
    if !manual && (!automatic || !is_due(last_check(), SystemTime::now())) {
        return Outcome::Quiet;
    }
    let running = match std::env::current_exe() {
        Ok(running) => running,
        Err(error) => return quiet_unless(Outcome::Failed(error.to_string())),
    };
    // Checked first, so a copy that never updates, such as one built from
    // source or a test, changes nothing on disk.
    let Ok(signed_by) = platform::signer(&running) else {
        return quiet_unless(Outcome::NotSigned);
    };
    let installed = setup::Places::current()
        .ok()
        .map(|places| places.installed_exe());
    if let Some(installed) = &installed {
        clean_up(installed);
    }
    if !manual {
        mark_checked();
    }
    let replaceable = installed.filter(|installed| setup::same_file(installed, &running));
    match check_github(replaceable.as_deref(), &signed_by) {
        Ok(Outcome::UpToDate) => quiet_unless(Outcome::UpToDate),
        Ok(outcome) => outcome,
        Err(message) => quiet_unless(Outcome::Failed(message)),
    }
}

fn user_agent() -> String {
    format!("TraceView/{}", env!("CARGO_PKG_VERSION"))
}

fn download(path: &str, most_bytes: usize) -> Result<Vec<u8>, String> {
    let download = platform::https_get(HOST, path, &user_agent(), most_bytes)
        .map_err(|error| format!("couldn't reach GitHub: {error}"))?;
    if !from_github(&download.final_url) {
        return Err(format!(
            "the download was sent somewhere other than GitHub: {}",
            download.final_url
        ));
    }
    Ok(download.bytes)
}

/// Looks for a newer release, and installs it over `installed` if that's
/// the copy running.
fn check_github(installed: Option<&Path>, signed_by: &Signer) -> Result<Outcome, String> {
    let sums = download(
        &format!("{REPOSITORY}/releases/latest/download/SHA256SUMS.txt"),
        MOST_SUMS_BYTES,
    )?;
    let sums = String::from_utf8_lossy(&sums);
    let listed =
        listed_exe(&sums, ARCH).ok_or("the latest release has no TraceView for this PC")?;
    let running = parse_version(env!("CARGO_PKG_VERSION")).unwrap_or_default();
    if listed.version <= running {
        return Ok(Outcome::UpToDate);
    }
    let version = listed.version_text();
    let Some(installed) = installed else {
        return Ok(Outcome::Available(version));
    };
    // Already put in place by an earlier check while this one kept running.
    if platform::file_version(installed).is_some_and(|there| there >= listed.version) {
        return Ok(Outcome::Installed(version));
    }
    let bytes = download(
        &format!(
            "{REPOSITORY}/releases/download/v{version}/{}",
            listed.file_name
        ),
        MOST_EXE_BYTES,
    )?;
    install(
        &bytes,
        &listed,
        installed,
        signed_by,
        platform::signer,
        platform::file_version,
    )
    .map_err(|error| format!("TraceView {version} wasn't installed, as {error}"))?;
    Ok(Outcome::Installed(version))
}

fn sha256(bytes: &[u8]) -> Result<String, String> {
    platform::sha256_hex(bytes)
}

/// Puts a downloaded EXE in place of `installed`, once it's checked. It's
/// written beside it, then kept open so nothing else can change it while
/// it's checked and renamed into place. The copy it replaces is renamed
/// aside, as Windows lets a running EXE be renamed but not replaced, and is
/// deleted at a later launch.
pub fn install(
    bytes: &[u8],
    listed: &Listed,
    installed: &Path,
    signed_by: &Signer,
    signer_of: impl Fn(&Path) -> Result<Signer, String>,
    version_of: impl Fn(&Path) -> Option<[u16; 3]>,
) -> Result<(), String> {
    if sha256(bytes)? != listed.sha256 {
        return Err("the download doesn't match the release's SHA256SUMS.txt".to_string());
    }
    let folder = installed
        .parent()
        .ok_or("the installed copy has no folder")?;
    let fresh = folder.join(format!("traceview.exe.{}.download", listed.version_text()));
    let _ = fs::remove_file(&fresh);
    let result = write_and_swap(
        bytes,
        listed,
        installed,
        &fresh,
        signed_by,
        &signer_of,
        &version_of,
    );
    if result.is_err() {
        let _ = fs::remove_file(&fresh);
    }
    result
}

fn write_and_swap(
    bytes: &[u8],
    listed: &Listed,
    installed: &Path,
    fresh: &Path,
    signed_by: &Signer,
    signer_of: &impl Fn(&Path) -> Result<Signer, String>,
    version_of: &impl Fn(&Path) -> Option<[u16; 3]>,
) -> Result<(), String> {
    let couldnt = |what: &str, error: std::io::Error| format!("couldn't {what}: {error}");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(fresh)
        .map_err(|error| couldnt("save the download", error))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| couldnt("save the download", error))?;
    drop(file);

    // Held until the swap, so the file checked is the file put in place.
    let mut held = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
        .open(fresh)
        .map_err(|error| couldnt("open the download", error))?;
    let mut saved = Vec::with_capacity(bytes.len());
    held.read_to_end(&mut saved)
        .map_err(|error| couldnt("read the download back", error))?;
    if sha256(&saved)? != listed.sha256 {
        return Err("the saved download changed before it was checked".to_string());
    }
    let signer = signer_of(fresh).map_err(|error| format!("its signature failed: {error}"))?;
    if signer != *signed_by {
        return Err(format!(
            "it's signed by {}, not by this copy's publisher",
            signer.subject
        ));
    }
    if version_of(fresh) != Some(listed.version) {
        return Err(format!(
            "the version inside it isn't {}",
            listed.version_text()
        ));
    }
    swap(installed, fresh, &held)
}

/// Renames the installed copy aside, then the checked download into its
/// place, putting the old one back if that fails.
fn swap(installed: &Path, fresh: &Path, _held: &File) -> Result<(), String> {
    let folder = installed
        .parent()
        .ok_or("the installed copy has no folder")?;
    let old = folder.join("traceview.exe.old");
    let _ = fs::remove_file(&old);
    fs::rename(installed, &old)
        .map_err(|error| format!("the installed copy couldn't be moved aside: {error}"))?;
    if let Err(error) = fs::rename(fresh, installed) {
        let _ = fs::rename(&old, installed);
        return Err(format!("it couldn't be put in place: {error}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_read_as_three_numbers() {
        assert_eq!(parse_version("0.13.1"), Some([0, 13, 1]));
        assert_eq!(parse_version("1.0"), None);
        assert_eq!(parse_version("1.0.0.0"), None);
        assert_eq!(parse_version("1.0.x"), None);
        assert_eq!(parse_version("+0.13.0"), None);
        assert_eq!(parse_version("0..1"), None);
        assert!(parse_version("0.13.0") > parse_version("0.12.9"));
        assert!(parse_version("0.10.0") > parse_version("0.9.0"));
    }

    const SUMS: &str = "\
e1326283af37115c521199a835e6f9d0c4a3a934860f36c493b9089c011281aa  TraceView-0.13.0-x64.zip
FB554ACD8C2837757844EC85D75BD83224C70AD19EB45E1CA911FF0F79946D14  TraceView-0.13.0-x64.exe
3f5d35397bfc29b5be3d6a205d9a675e44a3e9a9b69c478e04706ab113531317  TraceView-0.13.0-arm64.zip
74577918b2037056561303b28f5a676811f87c31052a8ad2fc2b56f7ceff3d2a  TraceView-0.13.0-arm64.exe
";

    #[test]
    fn a_releases_sums_list_this_pcs_exe() {
        let x64 = listed_exe(SUMS, "x64").unwrap();
        assert_eq!(x64.version, [0, 13, 0]);
        assert_eq!(x64.file_name, "TraceView-0.13.0-x64.exe");
        assert_eq!(
            x64.sha256,
            "fb554acd8c2837757844ec85d75bd83224c70ad19eb45e1ca911ff0f79946d14"
        );
        assert_eq!(
            listed_exe(SUMS, "arm64").unwrap().file_name,
            "TraceView-0.13.0-arm64.exe"
        );
        // Releases before 0.13 had only zips.
        assert_eq!(listed_exe(&SUMS.replace(".exe", ".bin"), "x64"), None);
        assert_eq!(listed_exe("nothing here", "x64"), None);
        assert_eq!(listed_exe("abc  TraceView-0.13.0-x64.exe", "x64"), None);
        assert_eq!(
            listed_exe(&SUMS.replace("0.13.0-x64.exe", "0.13-x64.exe"), "x64"),
            None
        );
    }

    #[test]
    fn downloads_come_only_from_github() {
        assert!(from_github(
            "https://github.com/thefinder808/TraceView-Windows/releases"
        ));
        assert!(from_github(
            "https://release-assets.githubusercontent.com/github-production-release-asset/1?x=1"
        ));
        assert!(from_github("https://objects.githubusercontent.com/a"));
        assert!(!from_github("http://github.com/a"));
        assert!(!from_github("https://github.com.example.com/a"));
        assert!(!from_github("https://githubusercontent.com.evil/a"));
        assert!(!from_github("https://evilgithubusercontent.com/a"));
        assert!(!from_github("https://example.com/github.com"));
    }

    #[test]
    fn a_check_is_due_once_a_day() {
        let now = SystemTime::now();
        assert!(is_due(None, now));
        assert!(!is_due(Some(now - Duration::from_secs(60 * 60)), now));
        assert!(is_due(Some(now - Duration::from_secs(21 * 60 * 60)), now));
        // A clock set back doesn't hold checks off.
        assert!(is_due(Some(now + Duration::from_secs(60 * 60)), now));
    }

    fn signer(subject: &str) -> Signer {
        Signer {
            subject: subject.to_string(),
            root: "CN=Root".to_string(),
        }
    }

    /// An installed copy, and a download listed with its true checksum.
    fn setup(new: &[u8]) -> (tempfile::TempDir, PathBuf, Listed) {
        let dir = tempfile::tempdir().unwrap();
        let installed = dir.path().join("traceview.exe");
        fs::write(&installed, b"old version").unwrap();
        let listed = Listed {
            version: [0, 13, 0],
            file_name: "TraceView-0.13.0-x64.exe".to_string(),
            sha256: sha256(new).unwrap(),
        };
        (dir, installed, listed)
    }

    fn leftovers(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_checked_download_takes_the_installed_copys_place() {
        let (dir, installed, listed) = setup(b"new version");
        let ours = signer("CN=Publisher");
        install(
            b"new version",
            &listed,
            &installed,
            &ours,
            |_| Ok(signer("CN=Publisher")),
            |_| Some([0, 13, 0]),
        )
        .unwrap();
        assert_eq!(fs::read(&installed).unwrap(), b"new version");
        assert_eq!(
            fs::read(dir.path().join("traceview.exe.old")).unwrap(),
            b"old version"
        );
        // A later launch removes the old copy.
        clean_up(&installed);
        assert_eq!(leftovers(dir.path()), ["traceview.exe"]);
    }

    #[test]
    fn a_download_that_fails_a_check_changes_nothing() {
        let ours = signer("CN=Publisher");
        let refused = |bytes: &[u8],
                       signed: Result<Signer, String>,
                       version: Option<[u16; 3]>,
                       listed_sha: Option<&str>| {
            let (dir, installed, mut listed) = setup(b"new version");
            if let Some(sha) = listed_sha {
                listed.sha256 = sha.to_string();
            }
            let error = install(
                bytes,
                &listed,
                &installed,
                &ours,
                |_| signed.clone(),
                |_| version,
            )
            .unwrap_err();
            assert_eq!(fs::read(&installed).unwrap(), b"old version", "{error}");
            assert_eq!(leftovers(dir.path()), ["traceview.exe"], "{error}");
            error
        };
        let good = Ok(signer("CN=Publisher"));
        let version = Some([0, 13, 0]);
        assert!(refused(b"tampered", good.clone(), version, None).contains("SHA256SUMS"));
        let other = refused(b"new version", Ok(signer("CN=Someone else")), version, None);
        assert!(other.contains("CN=Someone else"), "{other}");
        let other_root = Ok(Signer {
            subject: "CN=Publisher".to_string(),
            root: "CN=Another root".to_string(),
        });
        assert!(refused(b"new version", other_root, version, None).contains("publisher"));
        assert!(
            refused(b"new version", Err("it isn't signed".into()), version, None)
                .contains("it isn't signed")
        );
        assert!(refused(b"new version", good.clone(), Some([0, 12, 0]), None).contains("0.13.0"));
        assert!(refused(b"new version", good, None, Some(&"0".repeat(64))).contains("SHA256SUMS"));
    }

    /// Windows' own checks, and the rename, work on the download while it's
    /// held open against changes. A Windows EXE signed inside the file
    /// stands in for a new TraceView.
    #[test]
    fn a_signed_exe_passes_windows_checks_while_held() {
        let system32 = PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32");
        let mut exes: Vec<PathBuf> = fs::read_dir(system32)
            .unwrap()
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
            })
            .collect();
        exes.sort();
        let (signed, signed_by, version) = exes
            .iter()
            .find_map(|path| {
                let signed_by = platform::signer(path).ok()?;
                let version = platform::file_version(path)?;
                Some((path, signed_by, version))
            })
            .expect("System32 has an EXE signed inside the file");
        let bytes = fs::read(signed).unwrap();
        let (dir, installed, mut listed) = setup(&bytes);
        listed.version = version;
        install(
            &bytes,
            &listed,
            &installed,
            &signed_by,
            platform::signer,
            platform::file_version,
        )
        .unwrap();
        assert_eq!(fs::read(&installed).unwrap(), bytes);
        assert_eq!(
            leftovers(dir.path()),
            ["traceview.exe", "traceview.exe.old"]
        );
    }

    /// Reaches GitHub, so it only runs when asked for, with `--ignored`.
    #[test]
    #[ignore]
    fn the_latest_releases_checksums_come_from_github() {
        let sums = download(
            &format!("{REPOSITORY}/releases/latest/download/SHA256SUMS.txt"),
            MOST_SUMS_BYTES,
        )
        .unwrap();
        let sums = String::from_utf8(sums).unwrap();
        assert!(sums.contains(&format!("-{ARCH}.zip")), "{sums}");
    }

    #[test]
    fn leftovers_from_an_unfinished_update_are_removed() {
        let dir = tempfile::tempdir().unwrap();
        let installed = dir.path().join("traceview.exe");
        for name in [
            "traceview.exe",
            "traceview.exe.0.13.0.download",
            "traceview.exe.old",
            "notes.txt",
        ] {
            fs::write(dir.path().join(name), b"x").unwrap();
        }
        clean_up(&installed);
        assert_eq!(leftovers(dir.path()), ["notes.txt", "traceview.exe"]);
    }
}
