use std::fs::{File, OpenOptions};
use std::mem::ManuallyDrop;
use std::path::Path;

use crate::error::{Error, Result};

/// Exclusive OS file lock held for the lifetime of apply/destroy.
///
/// Implemented with `flock(LOCK_EX | LOCK_NB)` so a second apply in the same
/// directory fails with [`Error::Locked`], and so a crash or kill cannot leave
/// a permanent lock (the kernel releases the flock when the process dies).
///
/// `flock` is per open-file-description. A failed acquire must close that fd,
/// and [`Drop`] must `LOCK_UN` then close so a later acquire in the same
/// process can succeed.
pub struct Lock {
    file: ManuallyDrop<File>,
}

impl Lock {
    pub fn acquire(root: &Path) -> Result<Self> {
        let dir = root.join(".tofy");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&path)?;
        if let Err(e) = try_lock_exclusive(&file) {
            // Never leave a half-used fd open. LOCK_UN is a no-op if this
            // description never held the lock.
            unlock(&file);
            drop(file);
            return Err(e);
        }
        Ok(Self {
            file: ManuallyDrop::new(file),
        })
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        unlock(&self.file);
        unsafe {
            ManuallyDrop::drop(&mut self.file);
        }
    }
}

#[cfg(unix)]
fn try_lock_exclusive(file: &File) -> Result<()> {
    use std::os::unix::io::AsRawFd;
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        return Ok(());
    }
    let err = std::io::Error::last_os_error();
    match err.raw_os_error() {
        Some(code) if code == libc::EWOULDBLOCK || code == libc::EAGAIN => Err(Error::Locked),
        _ => Err(err.into()),
    }
}

#[cfg(unix)]
fn unlock(file: &File) {
    use std::os::unix::io::AsRawFd;
    let _ = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
}

#[cfg(not(unix))]
fn try_lock_exclusive(_file: &File) -> Result<()> {
    Err(Error::Engine(
        "apply lock requires an exclusive OS file lock (flock)".into(),
    ))
}

#[cfg(not(unix))]
fn unlock(_file: &File) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclusive_lock_rejects_second_apply() {
        let dir = tempfile::tempdir().unwrap();
        let first = Lock::acquire(dir.path()).unwrap();
        let second = Lock::acquire(dir.path());
        assert!(matches!(second, Err(Error::Locked)));
        drop(first);
        Lock::acquire(dir.path()).unwrap();
    }

    #[test]
    fn two_failed_acquires_then_drop_can_reacquire() {
        let dir = tempfile::tempdir().unwrap();
        let first = Lock::acquire(dir.path()).unwrap();
        assert!(matches!(Lock::acquire(dir.path()), Err(Error::Locked)));
        assert!(matches!(Lock::acquire(dir.path()), Err(Error::Locked)));
        drop(first);
        Lock::acquire(dir.path()).expect("failed acquires must not leak a flock fd");
    }

    #[cfg(unix)]
    #[test]
    fn leftover_lock_file_is_not_stale() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".tofy").join("lock");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "999999").unwrap();
        Lock::acquire(dir.path()).expect("a leftover lock file without a holder is not Locked");
    }

    #[cfg(unix)]
    #[test]
    fn lock_releases_after_process_death() {
        use std::io::Read;
        use std::process::{Command, Stdio};

        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join(".tofy").join("lock");
        std::fs::create_dir_all(lock_path.parent().unwrap()).unwrap();
        let mut child = Command::new("python3")
            .args([
                "-c",
                "import fcntl, os, sys, time\n\
                 path = sys.argv[1]\n\
                 fd = os.open(path, os.O_RDWR | os.O_CREAT, 0o644)\n\
                 fcntl.flock(fd, fcntl.LOCK_EX)\n\
                 sys.stdout.write('held\\n')\n\
                 sys.stdout.flush()\n\
                 time.sleep(60)\n",
                lock_path.to_str().unwrap(),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("python3 is required to prove flock releases on process death");
        let mut buf = [0u8; 8];
        child
            .stdout
            .as_mut()
            .unwrap()
            .read_exact(&mut buf[..5])
            .expect("child should print held");
        assert_eq!(&buf[..5], b"held\n");
        assert!(
            matches!(Lock::acquire(dir.path()), Err(Error::Locked)),
            "child still holds flock"
        );
        let _ = child.kill();
        let _ = child.wait();
        Lock::acquire(dir.path())
            .expect("flock must not be stale after the holding process is killed");
    }
}
