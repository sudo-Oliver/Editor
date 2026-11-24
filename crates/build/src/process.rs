//! A program and everything it starts, ended together: latexmk runs pdflatex
//! and biber, an add-on may run helpers. On Unix that is a process group; on
//! Windows a job object, which also ends them all if the editor itself dies.
//! Programs never open a console window of their own on Windows.

use std::io;

/// Makes `command` start its own group, without a console window on Windows.
pub fn prepare(command: &mut std::process::Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(imp::CREATE_NO_WINDOW);
    }
}

/// The same for a tokio command.
pub fn prepare_async(command: &mut tokio::process::Command) {
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(windows)]
    command.creation_flags(imp::CREATE_NO_WINDOW);
}

/// No console window on Windows, for a program that needs no group of its own.
pub fn no_window(command: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(imp::CREATE_NO_WINDOW);
    }
    command
}

/// The group of a started program, given by its process id.
pub struct Group(imp::Group);

impl Group {
    /// Takes in the program with `pid`, which `prepare` made ready.
    pub fn of(pid: u32) -> io::Result<Self> {
        imp::Group::of(pid).map(Self)
    }

    /// Asks every program of the group to end. Windows has no polite way for
    /// programs without a window, so there it ends them at once.
    pub fn terminate(&self) {
        self.0.signal(false);
    }

    pub fn kill(&self) {
        self.0.signal(true);
    }

    /// Whether any program of the group still runs.
    pub fn alive(&self) -> bool {
        self.0.alive()
    }
}

/// Whether a process with `pid` runs, such as another editor holding a lock.
pub fn running(pid: u32) -> bool {
    imp::running(pid)
}

#[cfg(unix)]
mod imp {
    use std::io;

    pub struct Group(libc::pid_t);

    impl Group {
        pub fn of(pid: u32) -> io::Result<Self> {
            Ok(Self(pid as libc::pid_t))
        }

        pub fn signal(&self, hard: bool) {
            // SAFETY: killpg only reads its two integer arguments.
            unsafe {
                libc::killpg(self.0, if hard { libc::SIGKILL } else { libc::SIGTERM });
            }
        }

        pub fn alive(&self) -> bool {
            // SAFETY: as above; signal 0 only checks that the group exists.
            unsafe { libc::killpg(self.0, 0) == 0 }
        }
    }

    pub fn running(pid: u32) -> bool {
        // SAFETY: kill with signal 0 only checks that the process exists.
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }
}

#[cfg(windows)]
mod imp {
    use std::{io, mem, ptr};

    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE, STILL_ACTIVE},
        System::{
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
                QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
            },
            Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA, PROCESS_TERMINATE},
        },
    };

    pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    pub struct Group(HANDLE);

    // SAFETY: a job handle may be used from any thread.
    unsafe impl Send for Group {}
    unsafe impl Sync for Group {}

    impl Group {
        pub fn of(pid: u32) -> io::Result<Self> {
            // SAFETY: plain Win32 calls on handles this function owns and closes.
            unsafe {
                let job = CreateJobObjectW(ptr::null(), ptr::null());
                if job.is_null() {
                    return Err(io::Error::last_os_error());
                }
                let group = Self(job);
                let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = mem::zeroed();
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                SetInformationJobObject(job, JobObjectExtendedLimitInformation, (&raw const limits).cast(), mem::size_of_val(&limits) as u32);
                let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
                if process.is_null() {
                    return Err(io::Error::last_os_error());
                }
                let assigned = AssignProcessToJobObject(job, process);
                CloseHandle(process);
                if assigned == 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(group)
            }
        }

        pub fn signal(&self, _hard: bool) {
            // SAFETY: the job handle lives as long as `self`.
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }

        pub fn alive(&self) -> bool {
            // SAFETY: as above, into a struct of the size the call expects.
            unsafe {
                let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = mem::zeroed();
                let read = QueryInformationJobObject(self.0, JobObjectBasicAccountingInformation, (&raw mut info).cast(), mem::size_of_val(&info) as u32, ptr::null_mut());
                read != 0 && info.ActiveProcesses > 0
            }
        }
    }

    impl Drop for Group {
        fn drop(&mut self) {
            // SAFETY: the handle is ours; closing it ends what is left in the job.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    pub fn running(pid: u32) -> bool {
        // SAFETY: a handle opened and closed here.
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if process.is_null() {
                return false;
            }
            let mut code = 0u32;
            let read = GetExitCodeProcess(process, &mut code);
            CloseHandle(process);
            read != 0 && code == STILL_ACTIVE as u32
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_group_ends_with_its_children() {
        #[cfg(unix)]
        let mut command = {
            let mut command = std::process::Command::new("sh");
            command.args(["-c", "sleep 30 & sleep 30"]);
            command
        };
        #[cfg(windows)]
        let mut command = {
            let mut command = std::process::Command::new("cmd");
            command.args(["/C", "ping -n 30 127.0.0.1 >NUL"]);
            command
        };
        prepare(&mut command);
        let mut child = command.spawn().unwrap();
        let group = Group::of(child.id()).unwrap();
        assert!(group.alive());
        assert!(running(child.id()));
        group.kill();
        child.wait().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while group.alive() {
            assert!(std::time::Instant::now() < deadline, "the group outlived the kill");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}
