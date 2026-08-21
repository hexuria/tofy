use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Exclusive lock held for the lifetime of apply/destroy.
pub struct Lock {
    path: PathBuf,
    _file: File,
}

impl Lock {
    pub fn acquire(root: &Path) -> Result<Self> {
        let dir = root.join(".tofy");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("lock");
        remove_stale(&path)?;

        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::AlreadyExists {
                    Error::Locked
                } else {
                    e.into()
                }
            })?;
        write!(file, "{}", std::process::id())?;
        file.flush()?;
        Ok(Self { path, _file: file })
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn remove_stale(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let raw = std::fs::read_to_string(path).unwrap_or_default();
    if let Ok(pid) = raw.trim().parse::<u32>() {
        if pid_alive(pid) {
            return Ok(());
        }
    }
    let _ = std::fs::remove_file(path);
    Ok(())
}

fn pid_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        Path::new(&format!("/proc/{pid}")).exists()
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

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
}
