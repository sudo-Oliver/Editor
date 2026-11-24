//! Earlier versions of every file the editor writes or replaces, outside the
//! project. A save, a reload after another program changed the file, or taking
//! the disk version over local edits first keeps what is about to go away, so a
//! deleted paragraph or a bad merge can always be undone, also without git.
//!
//! Saves can come every second while compiling on type, so older versions thin
//! out: every minute for the last hour, every hour for the last day, every day
//! for a month. Versions are compressed, each file's versions have a size
//! limit, and all of them together have one too, so the history never fills a
//! disk. Folders of files that are gone for a month go away.

use std::{
    fs, io,
    hash::{DefaultHasher, Hasher},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const PATH_FILE: &str = "path";
const COMPRESSED: &str = ".gz";
/// What the versions of one file may take, compressed. A thesis chapter
/// compresses to tens of KB, so this only bites for very large files.
const FILE_LIMIT: u64 = 16 * 1024 * 1024;
/// What the whole history may take.
pub const TOTAL_LIMIT: u64 = 256 * 1024 * 1024;
/// The newest versions of a file stay, whatever the limits say.
const NEWEST_KEPT: usize = 3;
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

/// Named by a hash that stays the same across versions of the app.
fn folder_for(directory: &Path, path: &Path) -> PathBuf {
    directory.join(stable_hash(path))
}

pub(crate) fn stable_hash(path: &Path) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(path.as_os_str().as_encoded_bytes());
    digest[..8].iter().map(|byte| format!("{byte:02x}")).collect()
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
    if versions.first().is_some_and(|(_, name)| name.trim_end_matches(COMPRESSED).ends_with(&hash)) {
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
    let mut encoder = flate2::write::GzEncoder::new(io::BufWriter::new(temp.as_file()), flate2::Compression::fast());
    io::Write::write_all(&mut encoder, bytes)?;
    encoder.finish()?.into_inner().map_err(io::IntoInnerError::into_error)?;
    temp.persist(folder.join(format!("{millis:013}-{hash}{COMPRESSED}"))).map_err(|error| error.error)?;
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

/// The bytes of a version as the file had them.
pub fn read(file: &Path) -> io::Result<Vec<u8>> {
    let stored = fs::read(file)?;
    if !file.to_string_lossy().ends_with(COMPRESSED) {
        return Ok(stored);
    }
    let mut bytes = Vec::new();
    io::Read::read_to_end(&mut flate2::read::GzDecoder::new(stored.as_slice()), &mut bytes)?;
    Ok(bytes)
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
    let dropped = to_drop(&times, now);
    for &index in &dropped {
        fs::remove_file(folder.join(&names[index].1)).ok();
    }
    // Then the oldest go until the rest fits.
    let mut total = 0;
    for (index, (_, name)) in names.iter().enumerate().filter(|(index, _)| !dropped.contains(index)) {
        let file = folder.join(name);
        total += fs::metadata(&file).map_or(0, |metadata| metadata.len());
        if total > FILE_LIMIT && index >= NEWEST_KEPT {
            fs::remove_file(file).ok();
        }
    }
}

/// What the history takes on disk.
pub fn size(directory: &Path) -> u64 {
    folders(directory).iter().flat_map(|folder| fs::read_dir(folder).into_iter().flatten().flatten()).filter_map(|entry| entry.metadata().ok()).map(|metadata| metadata.len()).sum()
}

fn folders(directory: &Path) -> Vec<PathBuf> {
    fs::read_dir(directory).into_iter().flatten().flatten().map(|entry| entry.path()).filter(|path| path.is_dir()).collect()
}

/// Tidies the whole history, once per start and away from typing: thins out
/// every file's versions, removes folders of files gone for a month, and then
/// drops the oldest versions anywhere until the history fits its limit.
pub fn sweep(directory: &Path) {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO).as_secs();
    let mut all = Vec::new();
    for folder in folders(directory) {
        prune(&folder, now);
        let names = names(&folder);
        let newest = names.first().map_or(0, |(millis, _)| (*millis / 1000) as u64);
        let gone = fs::read(folder.join(PATH_FILE))
            .ok()
            .is_none_or(|path| !Path::new(&*String::from_utf8_lossy(&path)).exists());
        if names.is_empty() || (gone && now.saturating_sub(newest) > MONTH) {
            fs::remove_dir_all(&folder).ok();
            continue;
        }
        for (index, (millis, name)) in names.into_iter().enumerate() {
            let file = folder.join(name);
            let size = fs::metadata(&file).map_or(0, |metadata| metadata.len());
            all.push((millis, index, file, size));
        }
    }
    let mut total: u64 = all.iter().map(|(.., size)| size).sum();
    all.sort_by_key(|(millis, ..)| *millis);
    for (_, index, file, size) in all {
        if total <= TOTAL_LIMIT {
            break;
        }
        if index >= NEWEST_KEPT && fs::remove_file(&file).is_ok() {
            total -= size;
        }
    }
}

/// Removes the whole history, for the storage settings.
pub fn clear(directory: &Path) -> io::Result<()> {
    match fs::remove_dir_all(directory) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

fn create_private(directory: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    // On Windows the folder inherits the rights of the user's data folder.
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(directory)
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
        let contents: Vec<Vec<u8>> = kept.iter().map(|version| read(&version.file).unwrap()).collect();
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
    fn versions_are_compressed() {
        let directory = tempfile::tempdir().unwrap();
        let path = Path::new("/project/chapter.tex");
        let text = "Ein Absatz über Messungen. ".repeat(4000);
        keep(directory.path(), path, text.as_bytes()).unwrap();
        let version = &versions(directory.path(), path)[0];
        assert!(version.size * 10 < text.len() as u64, "{} bytes on disk", version.size);
        assert_eq!(read(&version.file).unwrap(), text.as_bytes());
    }

    #[test]
    fn the_sweep_removes_gone_files_and_keeps_existing_ones() {
        let directory = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let here = project.path().join("here.tex");
        fs::write(&here, "x").unwrap();
        let gone = project.path().join("gone.tex");
        keep(directory.path(), &here, b"old").unwrap();
        keep(directory.path(), &gone, b"old").unwrap();
        // The version of the gone file is two months old.
        let folder = folder_for(directory.path(), &gone);
        let (_, name) = names(&folder).remove(0);
        let old = now_millis() - u128::from(2 * MONTH) * 1000;
        fs::rename(folder.join(&name), folder.join(format!("{old:013}-{}", name.split_once('-').unwrap().1))).unwrap();
        sweep(directory.path());
        assert_eq!(versions(directory.path(), &here).len(), 1);
        assert!(!folder.exists());
        assert!(size(directory.path()) > 0);
        clear(directory.path()).unwrap();
        assert_eq!(size(directory.path()), 0);
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
