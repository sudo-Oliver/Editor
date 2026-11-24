//! One editor per user on Windows. A second start, as from a double click on
//! a `.tex` in Explorer, hands its file to the running editor and ends. The
//! Mac does this itself through its open events.
//!
//! The second start first asks for the first one's process id and allows it to
//! come to the front: Windows lets only the process the user just started do
//! that, so without it the file would open behind Explorer.

#[cfg(windows)]
pub use imp::{forward, serve};

/// Elsewhere there is nothing to hand over.
#[cfg(not(windows))]
pub fn forward(_path: Option<&std::path::Path>) -> bool {
    false
}

#[cfg(not(windows))]
pub fn serve(_opened: tokio::sync::mpsc::UnboundedSender<std::path::PathBuf>) {}

#[cfg(windows)]
mod imp {
    use std::{
        ffi::OsStr,
        fs::File,
        io::{BufRead, BufReader, Write},
        os::windows::{ffi::OsStrExt, io::FromRawHandle},
        path::{Path, PathBuf},
    };

    use windows_sys::Win32::{
        Foundation::{GetLastError, INVALID_HANDLE_VALUE},
        Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX},
        System::Pipes::{ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT},
        UI::WindowsAndMessaging::AllowSetForegroundWindow,
    };

    const ERROR_PIPE_CONNECTED: u32 = 535;

    /// One pipe per user, so that two people on one PC each have their own editor.
    fn pipe_name() -> String {
        let user: String = std::env::var("USERNAME").unwrap_or_default().chars().filter(char::is_ascii_alphanumeric).collect();
        format!(r"\\.\pipe\editor-{user}")
    }

    /// Hands `path` to an editor that already runs. True when one took it, and
    /// this start should end.
    pub fn forward(path: Option<&Path>) -> bool {
        let Ok(pipe) = std::fs::OpenOptions::new().read(true).write(true).open(pipe_name()) else {
            return false;
        };
        let mut writer = &pipe;
        let mut reader = BufReader::new(&pipe);
        // The running editor says who it is, then may come to the front.
        if writer.write_all(b"pid\n").is_err() {
            return false;
        }
        let mut answer = String::new();
        if reader.read_line(&mut answer).is_err() {
            return false;
        }
        if let Ok(pid) = answer.trim().parse::<u32>() {
            // SAFETY: a plain call with a process id.
            unsafe {
                AllowSetForegroundWindow(pid);
            }
        }
        // Whole, since the running editor has another working folder.
        let line = path.and_then(|path| std::path::absolute(path).ok()).map(|path| path.to_string_lossy().into_owned()).unwrap_or_default();
        writer.write_all(format!("open {line}\n").as_bytes()).is_ok()
    }

    /// Takes in what later starts hand over, as files to open, on a thread of its own.
    pub fn serve(opened: tokio::sync::mpsc::UnboundedSender<PathBuf>) {
        let name: Vec<u16> = OsStr::new(&pipe_name()).encode_wide().chain(Some(0)).collect();
        std::thread::Builder::new()
            .name("single-instance".into())
            .spawn(move || {
                let mut first = true;
                loop {
                    let flags = PIPE_ACCESS_DUPLEX | if first { FILE_FLAG_FIRST_PIPE_INSTANCE } else { 0 };
                    // SAFETY: a nul-terminated name; the handle goes into a File, which closes it.
                    let handle = unsafe {
                        CreateNamedPipeW(name.as_ptr(), flags, PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS, PIPE_UNLIMITED_INSTANCES, 4096, 4096, 0, std::ptr::null())
                    };
                    if handle == INVALID_HANDLE_VALUE {
                        // Another editor serves already, as when two started at once.
                        return;
                    }
                    first = false;
                    // SAFETY: as above.
                    let connected = unsafe { ConnectNamedPipe(handle, std::ptr::null_mut()) } != 0 || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
                    // SAFETY: the handle is a valid pipe that nothing else owns.
                    let pipe = unsafe { File::from_raw_handle(handle as _) };
                    if !connected {
                        continue;
                    }
                    let mut writer = &pipe;
                    let mut reader = BufReader::new(&pipe);
                    let mut line = String::new();
                    while reader.read_line(&mut line).is_ok_and(|read| read > 0) {
                        let request = line.trim_end_matches(['\r', '\n']);
                        if request == "pid" {
                            writer.write_all(format!("{}\n", std::process::id()).as_bytes()).ok();
                        } else if let Some(path) = request.strip_prefix("open ") {
                            // An empty path only brings the window to the front.
                            if opened.send(PathBuf::from(path)).is_err() {
                                return;
                            }
                            break;
                        }
                        line.clear();
                    }
                }
            })
            .ok();
    }
}
