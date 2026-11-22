//! Earlier versions of every file the editor writes or replaces, outside the
//! project. A save, a reload after another program changed the file, or taking
//! the disk version over local edits first keeps what is about to go away, so a
//! deleted paragraph or a bad merge can always be undone, also without git.
//!
//! Saves can come every second while compiling on type, so older versions thin
//! out: every minute for the last hour, every hour for the last day, every day
//! for a month.

use std::{
    fs, io,
    hash::{DefaultHasher, Hash, Hasher},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const PATH_FILE: &str = "path";
/// Kept however old they are, so a file left alone for a year still has some.
const ALWAYS_KEPT: usize = 10;
const MINUTE: u64 = 60;
const HOUR: u64 = 3600;
const DAY: u64 = 86_400;
const MONTH: u64 = 30 * DAY;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    pub file: PathBuf,
    /// Seconds since 1970.
    pub time: u64,
    pub size: u64,
}

pub fn directory() -> Option<PathBuf> {
    Some(crate::settings::default_path()?.parent()?.join("history"))
}

fn folder_for(directory: &Path, path: &Path) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    directory.join(format!("{:016x}", hasher.finish()))
}

fn content_hash(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    hasher.write(bytes);
    hasher.finish()
}

fn now_millis() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO).as_millis()
}

/// Stores `bytes` as a version of `path`, unless the newest version holds them already.
pub fn keep(directory: &Path, path: &Path, bytes: &[u8]) -> io::Result<()> {
    if bytes.is_empty() {
        return Ok(());
    }
    let folder = folder_for(directory, path);
    create_private(&folder)?;
    let hash = format!("{:016x}", content_hash(bytes));
    let versions = names(&folder);
    if versions.first().is_some_and(|(_, name)| name.ends_with(&hash)) {
        return Ok(());
    }
    // The path is stored once, so a folder can be traced back to its file.
    let path_file = folder.join(PATH_FILE);
    if !path_file.exists() {
        fs::write(&path_file, path.as_os_str().as_encoded_bytes())?;
    }
    let mut millis = now_millis();
    // Two versions in one millisecond still get their own names.
    while versions.iter().any(|(time, _)| *time == millis) {
        millis += 1;
    }
    let temp = tempfile::NamedTempFile::new_in(&folder)?;
    fs::write(temp.path(), bytes)?;
    temp.persist(folder.join(format!("{millis:013}-{hash}"))).map_err(|error| error.error)?;
    prune(&folder, (millis / 1000) as u64);
    Ok(())
}

/// Keeps what is on disk at `path` right now. A missing file has nothing to keep.
pub fn keep_disk(directory: &Path, path: &Path) -> io::Result<()> {
    match fs::read(path) {
        Ok(bytes) => keep(directory, path, &bytes),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Every version of `path`, newest first.
pub fn versions(directory: &Path, path: &Path) -> Vec<Version> {
    let folder = folder_for(directory, path);
    names(&folder)
        .into_iter()
        .map(|(millis, name)| {
            let file = folder.join(name);
            let size = fs::metadata(&file).map_or(0, |metadata| metadata.len());
            Version { file, time: (millis / 1000) as u64, size }
        })
        .collect()
}

/// Version files as (milliseconds, name), newest first. Temp files and the path
/// file have no number in front and do not count.
fn names(folder: &Path) -> Vec<(u128, String)> {
    let Ok(entries) = fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut names: Vec<(u128, String)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let millis = name.split_once('-')?.0.parse().ok()?;
            Some((millis, name))
        })
        .collect();
    names.sort_by(|left, right| right.cmp(left));
    names
}

/// The versions to drop: of each minute in the last hour, each hour in the last
/// day and each day in the last month only the newest stays. Older ones go,
/// except the newest few overall.
fn to_drop(times: &[u64], now: u64) -> Vec<usize> {
    let mut drop = Vec::new();
    let mut last_bucket = None;
    for (index, &time) in times.iter().enumerate() {
        let age = now.saturating_sub(time);
        // The last minute stays whole, with each version on its own.
        if age < MINUTE {
            continue;
        }
        if age > MONTH {
            if index >= ALWAYS_KEPT {
                drop.push(index);
            }
            continue;
        }
        let current = bucket(time, now);
        if last_bucket == Some(current) {
            drop.push(index);
        } else {
            last_bucket = Some(current);
        }
    }
    drop
}

/// Which slot a version older than a minute falls in, by its age.
fn bucket(time: u64, now: u64) -> (u8, u64) {
    let age = now.saturating_sub(time);
    match age {
        0..HOUR => (1, time / MINUTE),
        HOUR..DAY => (2, time / HOUR),
        _ => (3, time / DAY),
    }
}

fn prune(folder: &Path, now: u64) {
    let names = names(folder);
    let times: Vec<u64> = names.iter().map(|(millis, _)| (*millis / 1000) as u64).collect();
    for index in to_drop(&times, now) {
        fs::remove_file(folder.join(&names[index].1)).ok();
    }
}

fn create_private(directory: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new().recursive(true).mode(0o700).create(directory)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_each_new_content_once_newest_first() {
        let directory = tempfile::tempdir().unwrap();
        let path = Path::new("/project/main.tex");
        keep(directory.path(), path, b"one").unwrap();
        keep(directory.path(), path, b"one").unwrap();
        keep(directory.path(), path, b"two").unwrap();
        keep(directory.path(), path, b"").unwrap();
        let kept = versions(directory.path(), path);
        let contents: Vec<Vec<u8>> = kept.iter().map(|version| fs::read(&version.file).unwrap()).collect();
        assert_eq!(contents, [b"two".to_vec(), b"one".to_vec()]);
        assert!(versions(directory.path(), Path::new("/project/other.tex")).is_empty());
    }

    #[test]
    fn a_missing_file_keeps_nothing() {
        let directory = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let path = project.path().join("gone.tex");
        keep_disk(directory.path(), &path).unwrap();
        assert!(versions(directory.path(), &path).is_empty());
        fs::write(&path, "text").unwrap();
        keep_disk(directory.path(), &path).unwrap();
        assert_eq!(versions(directory.path(), &path).len(), 1);
    }

    #[test]
    fn older_versions_thin_out() {
        let now = 100 * DAY;
        // One save per second over the last two minutes.
        let mut times: Vec<u64> = (0..120).map(|second| now - second).collect();
        // One save per minute over the last two hours.
        times.extend((3..120).map(|minute| now - minute * MINUTE));
        // Two months ago.
        times.extend((0..20).map(|day| now - 2 * MONTH - day * DAY));
        times.sort_by(|left, right| right.cmp(left));
        let drop = to_drop(&times, now);
        let kept: Vec<u64> = times.iter().enumerate().filter(|(index, _)| !drop.contains(index)).map(|(_, time)| *time).collect();
        // The last minute stays whole.
        assert!((0..60).all(|second| kept.contains(&(now - second))));
        // One per minute in the last hour, one per hour after that.
        assert!(kept.len() < 60 + 60 + 2 + 10);
        // Nothing older than a month is kept beyond the newest few.
        assert!(kept.iter().all(|time| now - time <= MONTH));
        assert!(kept.contains(&times[0]));
    }

    #[test]
    fn a_long_untouched_file_keeps_its_newest_versions() {
        let now = 1000 * DAY;
        let times: Vec<u64> = (0..30).map(|day| now - 400 * DAY - day * DAY).collect();
        let drop = to_drop(&times, now);
        assert_eq!(times.len() - drop.len(), ALWAYS_KEPT);
        assert!(!drop.contains(&0));
    }
}
