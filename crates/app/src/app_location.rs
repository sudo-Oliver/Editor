//! Moving the app to Applications. The app is not notarized, so people often
//! start it straight from the dmg or from Downloads, where macOS runs it from
//! a hidden, read-only copy. From there it cannot update itself and is gone
//! once the dmg is. On such a start a bar offers one button that copies the
//! app to Applications, starts it from there and ejects the dmg.

use std::{
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::OnceLock,
};

use gpui::{AnyElement, App, Context, Entity, EventEmitter, SharedString, prelude::*};

use crate::updater;

/// Where the running app is, when that is not a good place to stay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    /// On a mounted disk image, mounted at `mount`.
    DiskImage { mount: PathBuf },
    Downloads,
    /// macOS runs a copy from `/private/var/folders/…/AppTranslocation/`,
    /// since the app came from the internet and was never moved.
    Translocated,
}

/// Where `bundle` runs from, given the home folder and the mount points of
/// the disk images, or `None` when it is fine where it is.
pub fn classify(bundle: &Path, home: &Path, images: &[PathBuf]) -> Option<Place> {
    let translocation = (bundle.starts_with("/private/var/folders") || bundle.starts_with("/var/folders"))
        && bundle.components().any(|component| component.as_os_str() == "AppTranslocation");
    if translocation {
        return Some(Place::Translocated);
    }
    if let Some(mount) = images.iter().find(|mount| bundle.starts_with(mount)) {
        return Some(Place::DiskImage { mount: mount.clone() });
    }
    bundle.starts_with(home.join("Downloads")).then_some(Place::Downloads)
}

/// The mount points in the output of `hdiutil info`, one per mounted volume.
fn mount_points(info: &str) -> Vec<PathBuf> {
    info.lines()
        .filter(|line| line.starts_with("/dev/"))
        .filter_map(|line| line.split('\t').map(str::trim).next_back().filter(|field| field.starts_with('/')).map(PathBuf::from))
        .collect()
}

/// The mount points of the disk images. Blocks for a moment.
fn disk_images() -> Vec<PathBuf> {
    Command::new("hdiutil")
        .arg("info")
        .stdin(Stdio::null())
        .output()
        .map(|output| mount_points(&String::from_utf8_lossy(&output.stdout)))
        .unwrap_or_default()
}

/// The running Editor.app and its place, when it should move. Looked up once.
pub fn running() -> Option<&'static (PathBuf, Place)> {
    static RUNNING: OnceLock<Option<(PathBuf, Place)>> = OnceLock::new();
    RUNNING
        .get_or_init(|| {
            let bundle = updater::running_bundle()?;
            let home = PathBuf::from(std::env::var_os("HOME")?);
            // Asking hdiutil takes a moment, and only a volume can be a disk image.
            let images = if bundle.starts_with("/Volumes") { disk_images() } else { Vec::new() };
            let place = classify(&bundle, &home, &images)?;
            Some((bundle, place))
        })
        .as_ref()
}

/// Whether the app runs from somewhere it cannot replace itself.
pub fn read_only() -> bool {
    matches!(running(), Some((_, Place::DiskImage { .. } | Place::Translocated)))
}

/// /Applications, or ~/Applications for someone who may not write there.
fn applications() -> Result<PathBuf, String> {
    let system = PathBuf::from("/Applications");
    if updater::writable(&system) {
        return Ok(system);
    }
    let home = std::env::var_os("HOME").ok_or("Cannot find your home folder.")?;
    let own = PathBuf::from(home).join("Applications");
    std::fs::create_dir_all(&own).map_err(|error| format!("Cannot make {}: {error}", own.display()))?;
    Ok(own)
}

/// Copies the app into Applications, over an older Editor there. Returns the
/// new app. Blocks.
fn copy_to_applications(bundle: &Path) -> Result<PathBuf, String> {
    let folder = applications()?;
    let target = folder.join("Editor.app");
    updater::put_in_place(bundle, &target, &folder.join(".Editor-move.app"), &folder.join(".Editor-old.app"))?;
    Ok(target)
}

/// Once this process has ended: ejects `mount`, if any, and starts `bundle`.
fn relaunch_and_eject(bundle: &Path, mount: Option<&Path>) {
    Command::new("/bin/sh")
        .args([
            "-c",
            "while kill -0 \"$1\" 2>/dev/null; do sleep 0.2; done; [ -n \"$3\" ] && /usr/bin/hdiutil detach -quiet \"$3\"; exec /usr/bin/open \"$2\"",
            "sh",
        ])
        .arg(std::process::id().to_string())
        .arg(bundle)
        .arg(mount.map(Path::as_os_str).unwrap_or_default())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .ok();
}

pub enum MoverEvent {
    /// "Don't ask again": the workspace keeps it in the settings.
    Declined,
}

enum State {
    Hidden,
    Offered,
    Moving,
    Failed(String),
}

/// The offer to move the app, shown as a bar like the one for updates.
pub struct Mover {
    state: State,
    running: Option<&'static (PathBuf, Place)>,
}

impl EventEmitter<MoverEvent> for Mover {}

impl Mover {
    pub fn new(declined: bool) -> Self {
        let running = running();
        let state = if running.is_some() && !declined { State::Offered } else { State::Hidden };
        Self { state, running }
    }

    pub fn start(&mut self, cx: &mut Context<Self>) {
        let Some((bundle, place)) = self.running.cloned() else {
            return;
        };
        if matches!(self.state, State::Moving) {
            return;
        }
        self.state = State::Moving;
        cx.notify();
        let copy = cx.background_spawn(async move {
            let target = copy_to_applications(&bundle)?;
            let mount = match place {
                Place::DiskImage { mount } => Some(mount),
                // macOS hides where the copy came from: the dmg with an
                // Editor.app is the best guess.
                Place::Translocated => disk_images().into_iter().find(|mount| mount.join("Editor.app").is_dir()),
                Place::Downloads => None,
            };
            Ok::<_, String>((target, mount))
        });
        cx.spawn(async move |this, cx| {
            let result = copy.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok((target, mount)) => {
                        relaunch_and_eject(&target, mount.as_deref());
                        // Quitting saves every open file first.
                        cx.quit();
                    }
                    Err(error) => this.state = State::Failed(error),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn not_now(&mut self, cx: &mut Context<Self>) {
        self.state = State::Hidden;
        cx.notify();
    }

    fn never(&mut self, cx: &mut Context<Self>) {
        self.state = State::Hidden;
        cx.emit(MoverEvent::Declined);
        cx.notify();
    }
}

/// The bar that offers the move, or `None` when there is nothing to say.
pub fn banner(mover: &Entity<Mover>, cx: &App) -> Option<AnyElement> {
    let state = mover.read(cx);
    let place = &state.running.as_ref()?.1;
    let (move_now, not_now, never) = (mover.clone(), mover.clone(), mover.clone());
    let (text, buttons): (SharedString, Vec<AnyElement>) = match &state.state {
        State::Hidden => return None,
        State::Offered => (
            match place {
                Place::DiskImage { .. } => "Editor runs from the disk image. Move it to Applications so it stays after the image is gone and can update itself.",
                Place::Downloads => "Editor runs from Downloads. Move it to Applications, where it can update itself.",
                Place::Translocated => "macOS runs Editor from a hidden copy. Move it to Applications, where it can update itself.",
            }
            .into(),
            vec![
                updater::bar_button("move-never", "Don't ask again", false, move |cx| never.update(cx, |mover, cx| mover.never(cx))).into_any_element(),
                updater::bar_button("move-later", "Not now", false, move |cx| not_now.update(cx, |mover, cx| mover.not_now(cx))).into_any_element(),
                updater::bar_button("move-now", "Move to Applications", true, move |cx| move_now.update(cx, |mover, cx| mover.start(cx))).into_any_element(),
            ],
        ),
        State::Moving => ("Moving Editor to Applications…".into(), Vec::new()),
        State::Failed(error) => (
            format!("Editor could not move. {error}").into(),
            vec![
                updater::bar_button("move-later", "Not now", false, move |cx| not_now.update(cx, |mover, cx| mover.not_now(cx))).into_any_element(),
                updater::bar_button("move-again", "Try again", true, move |cx| move_now.update(cx, |mover, cx| mover.start(cx))).into_any_element(),
            ],
        ),
    };
    Some(updater::bar(text, buttons))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(bundle: &str) -> Option<Place> {
        classify(Path::new(bundle), Path::new("/Users/ada"), &[PathBuf::from("/Volumes/Editor")])
    }

    #[test]
    fn the_dmg_downloads_and_translocation_ask_to_move() {
        assert_eq!(place("/Volumes/Editor/Editor.app"), Some(Place::DiskImage { mount: PathBuf::from("/Volumes/Editor") }));
        assert_eq!(place("/Users/ada/Downloads/Editor.app"), Some(Place::Downloads));
        assert_eq!(place("/Users/ada/Downloads/Editor 2/Editor.app"), Some(Place::Downloads));
        assert_eq!(place("/private/var/folders/x1/abc123/T/AppTranslocation/0F1E-22/d/Editor.app"), Some(Place::Translocated));
        assert_eq!(place("/var/folders/x1/abc123/T/AppTranslocation/0F1E-22/d/Editor.app"), Some(Place::Translocated));
    }

    #[test]
    fn other_places_stay_quiet() {
        assert_eq!(place("/Applications/Editor.app"), None);
        assert_eq!(place("/Users/ada/Applications/Editor.app"), None);
        assert_eq!(place("/Users/ada/Desktop/Editor.app"), None);
        // An external disk that is not a disk image.
        assert_eq!(place("/Volumes/Backup/Editor.app"), None);
        // Only the folder itself, not one named alike.
        assert_eq!(place("/Users/ada/Downloads-old/Editor.app"), None);
        assert_eq!(place("/Volumes/Editor 1/Editor.app"), None);
        assert_eq!(place("/private/var/folders/x1/abc123/T/Editor.app"), None);
    }

    #[test]
    fn mount_points_come_from_hdiutil() {
        let info = "framework       : 685\n================================================\nimage-path      : /Users/ada/Downloads/Editor-0.2.0.dmg\nimage-alias     : /Users/ada/Downloads/Editor-0.2.0.dmg\n/dev/disk4          \tGUID_partition_scheme          \t\n/dev/disk4s1        \tApple_HFS                      \t/Volumes/Editor\n================================================\nimage-path      : /tmp/b.dmg\n/dev/disk5s1        \tApple_HFS                      \t/Volumes/Editor 1\n";
        assert_eq!(mount_points(info), vec![PathBuf::from("/Volumes/Editor"), PathBuf::from("/Volumes/Editor 1")]);
    }
}
