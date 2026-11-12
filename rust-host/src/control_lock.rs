use std::env;
#[cfg(test)]
use std::fs;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

const LOCK_EX: i32 = 2;
const LOCK_NB: i32 = 4;
const PR_SET_PDEATHSIG: i32 = 1;
const SIGTERM: i32 = 15;

unsafe extern "C" {
    fn flock(fd: i32, operation: i32) -> i32;
    fn geteuid() -> u32;
    fn getppid() -> i32;
    fn prctl(option: i32, ...) -> i32;
}

pub(crate) fn arm_parent_death_signal() -> io::Result<()> {
    let parent = unsafe { getppid() };
    // SAFETY: PR_SET_PDEATHSIG expects an integer signal number.
    if unsafe { prctl(PR_SET_PDEATHSIG, SIGTERM) } == -1 {
        return Err(io::Error::last_os_error());
    }
    // Close the race where the parent exits between getppid() and prctl().
    if unsafe { getppid() } != parent {
        return Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "parent process exited while WayTouch control was starting",
        ));
    }
    Ok(())
}

/// Process-wide guard: a second CLI or GUI controller must not capture the
/// same physical devices while this controller owns EVIOCGRAB.
#[derive(Debug)]
pub(crate) struct ControlLock {
    _file: File,
}

impl ControlLock {
    pub(crate) fn acquire() -> io::Result<Self> {
        let path = lock_path();
        Self::acquire_at(&path)
    }

    fn acquire_at(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path)?;

        // SAFETY: flock operates on the live file descriptor and does not
        // retain the pointer or mutate Rust-managed memory.
        if unsafe { flock(file.as_raw_fd(), LOCK_EX | LOCK_NB) } == -1 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::WouldBlock {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "another WayTouch control process already owns the input devices",
                ));
            }
            return Err(error);
        }

        Ok(Self { _file: file })
    }
}

fn lock_path() -> PathBuf {
    if let Some(runtime_dir) = env::var_os("XDG_RUNTIME_DIR") {
        let dir = PathBuf::from(runtime_dir);
        if dir.is_dir() {
            return dir.join("waytouch-control.lock");
        }
    }

    let uid = unsafe { geteuid() };
    env::temp_dir().join(format!("waytouch-control-{uid}.lock"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn only_one_control_owner_can_hold_the_lock() {
        let suffix = NEXT_TEST.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!(
            "waytouch-lock-test-{}-{suffix}.lock",
            std::process::id()
        ));
        let first = ControlLock::acquire_at(&path).unwrap();
        let second = ControlLock::acquire_at(&path).unwrap_err();
        assert_eq!(second.kind(), io::ErrorKind::AlreadyExists);
        drop(first);

        let third = ControlLock::acquire_at(&path).unwrap();
        drop(third);
        let _ = fs::remove_file(path);
    }
}
