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
        time::{Duration, Instant},
    };

    use windows_sys::Win32::{
        Foundation::{ERROR_ALREADY_EXISTS, GetLastError, INVALID_HANDLE_VALUE},
        Storage::FileSystem::PIPE_ACCESS_DUPLEX,
        System::{
            Pipes::{ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT, WaitNamedPipeW},
            Threading::CreateMutexW,
        },
        UI::WindowsAndMessaging::AllowSetForegroundWindow,
    };

    const ERROR_PIPE_CONNECTED: u32 = 535;
    const ERROR_PIPE_BUSY: i32 = 231;
    /// How long a second start waits for the first editor's pipe, as when
    /// several files open at once or the first one is still starting.
    const PATIENCE: Duration = Duration::from_secs(5);

    /// One editor per user, so that two people on one PC each have their own.
    fn user() -> String {
        std::env::var("USERNAME").unwrap_or_default().chars().filter(char::is_ascii_alphanumeric).collect()
    }

    fn pipe_name() -> String {
        format!(r"\\.\pipe\editor-{}", user())
    }

    fn wide(text: &str) -> Vec<u16> {
        OsStr::new(text).encode_wide().chain(Some(0)).collect()
    }

    /// Whether this is the user's first editor. The mutex says so reliably,
    /// also while the first one has not opened its pipe yet, and it stays as
    /// long as this process runs.
    fn first() -> bool {
        let name = wide(&format!(r"Local\editor-{}", user()));
        // SAFETY: a nul-terminated name; the handle is kept on purpose.
        unsafe {
            let mutex = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
            mutex.is_null() || GetLastError() != ERROR_ALREADY_EXISTS
        }
    }

    /// The first editor's pipe, waited for while it is busy or not there yet.
    fn connect() -> Option<File> {
        let name = pipe_name();
        let started = Instant::now();
        loop {
            match std::fs::OpenOptions::new().read(true).write(true).open(&name) {
                Ok(pipe) => return Some(pipe),
                Err(_) if started.elapsed() >= PATIENCE => return None,
                Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                    // SAFETY: a nul-terminated name.
                    unsafe {
                        WaitNamedPipeW(wide(&name).as_ptr(), 500);
                    }
                }
                Err(_) => std::thread::sleep(Duration::from_millis(100)),
            }
        }
    }

    /// Hands `path` to an editor that already runs. True when one took it, and
    /// this start should end.
    pub fn forward(path: Option<&Path>) -> bool {
        if first() {
            return false;
        }
        let Some(pipe) = connect() else {
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
        let name = wide(&pipe_name());
        std::thread::Builder::new()
            .name("single-instance".into())
            .spawn(move || {
                loop {
                    // SAFETY: a nul-terminated name; the handle goes into a File, which closes it.
                    let handle = unsafe {
                        CreateNamedPipeW(name.as_ptr(), PIPE_ACCESS_DUPLEX, PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS, PIPE_UNLIMITED_INSTANCES, 4096, 4096, 0, std::ptr::null())
                    };
                    if handle == INVALID_HANDLE_VALUE {
                        return;
                    }
                    // SAFETY: as above.
                    let connected = unsafe { ConnectNamedPipe(handle, std::ptr::null_mut()) } != 0 || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
                    // SAFETY: the handle is a valid pipe that nothing else owns.
                    let pipe = unsafe { File::from_raw_handle(handle as _) };
                    if !connected {
                        continue;
                    }
                    // Each start is answered on a thread of its own, so that the
                    // next pipe is there at once for the start after it.
                    let opened = opened.clone();
                    std::thread::spawn(move || answer(pipe, opened));
                }
            })
            .ok();
    }

    /// Tells a later start who runs, then takes its file.
    fn answer(pipe: File, opened: tokio::sync::mpsc::UnboundedSender<PathBuf>) {
        let mut writer = &pipe;
        let mut reader = BufReader::new(&pipe);
        let mut line = String::new();
        while reader.read_line(&mut line).is_ok_and(|read| read > 0) {
            let request = line.trim_end_matches(['\r', '\n']);
            if request == "pid" {
                writer.write_all(format!("{}\n", std::process::id()).as_bytes()).ok();
            } else if let Some(path) = request.strip_prefix("open ") {
                // An empty path only brings the window to the front.
                opened.send(PathBuf::from(path)).ok();
                return;
            }
            line.clear();
        }
    }
}
