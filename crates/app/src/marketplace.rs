//! The catalog of add-ons that can be downloaded. It is a JSON file in a public
//! repository. The editor reaches for it only while the add-ons panel shows.
//!
//! A checksum protects the download. It does not say who wrote the add-on, so
//! installing from here means trusting that repository.

use std::{
    collections::HashMap,
    path::{Component, Path},
    process::Command,
};

use addon_api::{API_VERSION, Manifest, Permission};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::addons::{self, Installed};

const CATALOG_URL: &str = "https://raw.githubusercontent.com/sudo-Oliver/editor-addons/main/catalog.json";
/// Replaces the catalog for testing, and then also allows `file:` addresses.
const CATALOG_OVERRIDE: &str = "EDITOR_ADDON_CATALOG";
const CATALOG_SECONDS: &str = "15";
const DOWNLOAD_SECONDS: &str = "300";
const MAX_DOWNLOAD_BYTES: &str = "209715200";

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Download {
    pub url: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct CatalogEntry {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub api: u32,
    pub permissions: Vec<Permission>,
    /// By platform, such as `darwin-arm64`.
    pub downloads: HashMap<String, Download>,
}

impl CatalogEntry {
    pub fn download(&self) -> Option<&Download> {
        self.downloads.get(&platform())
    }

    pub fn permission_text(&self) -> String {
        permission_text(&self.permissions)
    }
}

pub fn permission_text(permissions: &[Permission]) -> String {
    if permissions.is_empty() {
        return "Asks for no permissions.".to_string();
    }
    permissions.iter().map(|permission| permission.description()).collect::<Vec<_>>().join(", ")
}

pub fn platform() -> String {
    let os = if cfg!(target_os = "macos") { "darwin" } else { std::env::consts::OS };
    let arch = if cfg!(target_arch = "aarch64") { "arm64" } else { std::env::consts::ARCH };
    format!("{os}-{arch}")
}

#[derive(Deserialize)]
struct Catalog {
    addons: Vec<serde_json::Value>,
}

/// What runs here. An entry that a newer editor would need, or that this version
/// cannot read, does not take the rest of the catalog with it.
pub fn parse_catalog(text: &str) -> Result<Vec<CatalogEntry>, String> {
    let catalog: Catalog = serde_json::from_str(text).map_err(|error| format!("The catalog is broken: {error}"))?;
    let mut entries: Vec<CatalogEntry> = catalog
        .addons
        .into_iter()
        .filter_map(|entry| serde_json::from_value::<CatalogEntry>(entry).ok())
        .filter(|entry| (addon_api::OLDEST_API_VERSION..=API_VERSION).contains(&entry.api) && entry.download().is_some())
        .collect();
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(entries)
}

pub(crate) fn curl(seconds: &str) -> Command {
    let protocols = if cfg!(test) || std::env::var_os(CATALOG_OVERRIDE).is_some() { "=https,file" } else { "=https" };
    let mut command = Command::new("curl");
    command.args(["--fail", "--silent", "--show-error", "--location", "--max-time", seconds]);
    command.args(["--proto", protocols, "--proto-redir", protocols]);
    command
}

pub(crate) fn failure(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).trim().to_string()
}

/// Blocks.
pub fn fetch() -> Result<Vec<CatalogEntry>, String> {
    let url = std::env::var(CATALOG_OVERRIDE).unwrap_or_else(|_| CATALOG_URL.to_string());
    let output = curl(CATALOG_SECONDS).arg(&url).output().map_err(|error| format!("Cannot run curl: {error}"))?;
    if !output.status.success() {
        return Err(failure(&output));
    }
    parse_catalog(&String::from_utf8_lossy(&output.stdout))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) fn archive_paths_stay_inside(listing: &str) -> bool {
    listing.lines().all(|line| Path::new(line).components().all(|part| matches!(part, Component::Normal(_) | Component::CurDir)))
}

/// The folder must hold what the catalog promised, because the catalog is what
/// the user read before choosing to install.
fn check_against_catalog(manifest: &Manifest, entry: &CatalogEntry) -> Result<(), String> {
    if manifest.id != entry.id || manifest.version != entry.version {
        return Err(format!("The download of {} holds {} {} instead.", entry.name, manifest.id, manifest.version));
    }
    let same = manifest.permissions.len() == entry.permissions.len() && manifest.permissions.iter().all(|permission| entry.permissions.contains(permission));
    if !same {
        return Err(format!("The download of {} asks for other permissions than the catalog says.", entry.name));
    }
    Ok(())
}

pub(crate) fn run_tar(command: &mut Command) -> Result<std::process::Output, String> {
    let output = command.output().map_err(|error| format!("Cannot run tar: {error}"))?;
    if output.status.success() { Ok(output) } else { Err(format!("Cannot unpack the download: {}", failure(&output))) }
}

/// Whether `candidate` is a later version than `installed`, part by part, so that
/// 0.10 follows 0.9 and 1.0 equals 1.0.0. A version that is not made of numbers
/// never counts as later, because nobody can say what it follows.
pub fn is_newer(candidate: &str, installed: &str) -> bool {
    let parts = |version: &str| version.split('.').map(|part| part.parse::<u64>().ok()).collect::<Option<Vec<u64>>>();
    let (Some(candidate), Some(installed)) = (parts(candidate), parts(installed)) else {
        return false;
    };
    for index in 0..candidate.len().max(installed.len()) {
        let (new, old) = (candidate.get(index).unwrap_or(&0), installed.get(index).unwrap_or(&0));
        if new != old {
            return new > old;
        }
    }
    false
}

/// An update stays on only when it asks for nothing new.
pub fn stays_enabled(previous: &[Permission], next: &[Permission]) -> bool {
    next.iter().all(|permission| previous.contains(permission))
}

/// Downloads, checks and installs. Blocks. Returns whether it is turned on.
pub fn install(entry: &CatalogEntry, directory: &Path, previous: Option<&Installed>) -> Result<bool, String> {
    let download = entry.download().ok_or_else(|| format!("{} has no download for this computer.", entry.name))?;
    let workspace = tempfile::Builder::new().prefix("editor-addon-").tempdir().map_err(|error| format!("Cannot prepare the download: {error}"))?;
    let archive = workspace.path().join("download.tar.gz");
    let output = curl(DOWNLOAD_SECONDS)
        .args(["--max-filesize", MAX_DOWNLOAD_BYTES, "--output"])
        .arg(&archive)
        .arg(&download.url)
        .output()
        .map_err(|error| format!("Cannot run curl: {error}"))?;
    if !output.status.success() {
        return Err(format!("Cannot download {}: {}", entry.name, failure(&output)));
    }
    let bytes = std::fs::read(&archive).map_err(|error| format!("Cannot read the download: {error}"))?;
    if sha256_hex(&bytes) != download.sha256.to_ascii_lowercase() {
        return Err(format!("The download of {} does not match its checksum, so it was not installed.", entry.name));
    }
    let listing = run_tar(Command::new("tar").arg("-tzf").arg(&archive))?;
    if !archive_paths_stay_inside(&String::from_utf8_lossy(&listing.stdout)) {
        return Err(format!("The download of {} leads outside its folder, so it was not installed.", entry.name));
    }
    let unpacked = workspace.path().join("unpacked");
    std::fs::create_dir(&unpacked).map_err(|error| format!("Cannot unpack the download: {error}"))?;
    run_tar(Command::new("tar").arg("-xzf").arg(&archive).arg("--no-same-owner").arg("-C").arg(&unpacked))?;
    let manifest = addons::read_manifest(&unpacked)?;
    check_against_catalog(&manifest, entry)?;
    if !unpacked.join(&manifest.program).is_file() {
        return Err(format!("The download of {} has no program.", entry.name));
    }
    addons::install_from(&unpacked, directory)?;
    let enabled = previous.is_some_and(|old| old.enabled && stays_enabled(&old.manifest.permissions, &manifest.permissions));
    if enabled {
        std::fs::write(directory.join(&manifest.id).join(addons::ENABLED_MARKER), "").map_err(|error| format!("Cannot turn on {}: {error}", entry.name))?;
    }
    Ok(enabled)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    const MANIFEST: &str = "id = \"a\"\nname = \"A\"\nversion = \"2\"\nauthor = \"B\"\napi = 1\nprogram = \"run\"\npermissions = [\"read_project\"]\n";

    fn entry(url: String, sha256: String) -> CatalogEntry {
        CatalogEntry {
            id: "a".to_string(),
            name: "A".to_string(),
            version: "2".to_string(),
            author: "B".to_string(),
            description: String::new(),
            api: API_VERSION,
            permissions: vec![Permission::ReadProject],
            downloads: [(platform(), Download { url, sha256 })].into(),
        }
    }

    /// A packed add-on and its checksum.
    fn package(directory: &Path, manifest: &str) -> (String, String) {
        let source = directory.join("source");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("addon.toml"), manifest).unwrap();
        fs::write(source.join("run"), "#!/bin/sh\n").unwrap();
        let archive = directory.join("a.tar.gz");
        let status = Command::new("tar").arg("-czf").arg(&archive).arg("-C").arg(&source).args(["addon.toml", "run"]).status().unwrap();
        assert!(status.success());
        (format!("file://{}", archive.display()), sha256_hex(&fs::read(&archive).unwrap()))
    }

    #[test]
    fn catalog_skips_what_cannot_run_here() {
        let platform = platform();
        let row = |id: &str, api: u32, on: &str, permission: &str| {
            format!(
                r#"{{"id":"{id}","name":"{id}","version":"1","author":"x","description":"d","api":{api},"permissions":["{permission}"],"downloads":{{"{on}":{{"url":"https://x","sha256":"0"}}}}}}"#
            )
        };
        let text = format!(
            r#"{{"addons":[{},{},{},{},{{"nonsense":true}}]}}"#,
            row("good", API_VERSION, &platform, "read_project"),
            row("newer", 999, &platform, "read_project"),
            row("elsewhere", API_VERSION, "plan9-mips", "read_project"),
            row("unknown-permission", API_VERSION, &platform, "read_minds"),
        );
        let entries = parse_catalog(&text).unwrap();
        assert_eq!(entries.iter().map(|entry| entry.id.as_str()).collect::<Vec<_>>(), ["good"]);
        assert!(parse_catalog("nonsense").is_err());
    }

    #[test]
    fn checksums_read_as_lowercase_hex() {
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn archives_cannot_lead_outside() {
        assert!(archive_paths_stay_inside("addon.toml\nbin/run\n./x\n"));
        assert!(!archive_paths_stay_inside("addon.toml\n../evil\n"));
        assert!(!archive_paths_stay_inside("/etc/passwd\n"));
        assert!(!archive_paths_stay_inside("a/../../evil\n"));
    }

    #[test]
    fn new_permissions_turn_an_update_off() {
        let (read, edit) = (Permission::ReadProject, Permission::EditDocuments);
        assert!(stays_enabled(&[read, edit], &[read]));
        assert!(stays_enabled(&[read], &[read]));
        assert!(!stays_enabled(&[read], &[read, edit]));
    }

    #[test]
    fn only_later_versions_count_as_updates() {
        assert!(is_newer("0.3.0", "0.2.0"));
        assert!(is_newer("0.10.0", "0.9.0"));
        assert!(is_newer("2", "1.9"));
        assert!(!is_newer("0.2.0", "0.3.0"));
        assert!(!is_newer("1.0", "1.0.0"));
        assert!(!is_newer("1.0-beta", "0.9"));
    }

    #[test]
    fn install_checks_the_download_and_starts_turned_off() {
        let directory = tempdir().unwrap();
        let addons_directory = directory.path().join("addons");
        let (url, sha256) = package(directory.path(), MANIFEST);

        let wrong = entry(url.clone(), "0".repeat(64));
        assert!(install(&wrong, &addons_directory, None).unwrap_err().contains("checksum"));
        assert!(!addons_directory.join("a").exists());

        let good = entry(url, sha256);
        assert!(!install(&good, &addons_directory, None).unwrap());
        let (installed, _) = addons::load_all(&addons_directory);
        assert_eq!(installed[0].manifest.version, "2");
        assert!(!installed[0].enabled);
    }

    #[test]
    fn update_keeps_an_add_on_on_unless_it_wants_more() {
        let directory = tempdir().unwrap();
        let addons_directory = directory.path().join("addons");
        let (url, sha256) = package(directory.path(), MANIFEST);
        let good = entry(url, sha256);
        install(&good, &addons_directory, None).unwrap();
        let mut old = addons::load_all(&addons_directory).0.remove(0);
        old.enabled = true;

        assert!(install(&good, &addons_directory, Some(&old)).unwrap());
        assert!(addons::load_all(&addons_directory).0[0].enabled);

        old.manifest.permissions.clear();
        assert!(!install(&good, &addons_directory, Some(&old)).unwrap());
        assert!(!addons::load_all(&addons_directory).0[0].enabled);
    }

    #[test]
    fn a_download_that_differs_from_the_catalog_is_refused() {
        let directory = tempdir().unwrap();
        let addons_directory = directory.path().join("addons");
        let manifest = MANIFEST.replace("[\"read_project\"]", "[\"read_project\", \"secrets\"]");
        let (url, sha256) = package(directory.path(), &manifest);
        assert!(install(&entry(url, sha256), &addons_directory, None).unwrap_err().contains("other permissions"));
        assert!(!addons_directory.join("a").exists());
    }

    #[test]
    fn a_failed_download_says_so() {
        let directory = tempdir().unwrap();
        let missing = entry(format!("file://{}/none", directory.path().display()), String::new());
        assert!(install(&missing, directory.path(), None).unwrap_err().contains("Cannot download"));
    }
}
