//! Lock file persistence and process-level state locking.
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};
use crate::model::ModelEntry;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LockFile {
    pub version: u32,
    #[serde(default)]
    pub models: BTreeMap<String, ModelEntry>,
}

impl Default for LockFile {
    fn default() -> Self {
        Self {
            version: 1,
            models: BTreeMap::new(),
        }
    }
}

impl LockFile {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }

        let bytes = fs::read(path).map_err(|source| AppError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let lock = serde_json::from_slice(&bytes).map_err(|source| AppError::ParseJson {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(lock)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| AppError::CreateDir {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        let bytes =
            serde_json::to_vec_pretty(self).map_err(|source| AppError::EncodeJson { source })?;
        let tmp = crate::config::atomic_tmp_path(path)?;
        fs::write(&tmp, bytes).map_err(|source| AppError::Write {
            path: tmp.clone(),
            source,
        })?;
        fs::rename(&tmp, path).map_err(|source| AppError::Rename {
            from: tmp,
            to: path.to_path_buf(),
            source,
        })?;
        Ok(())
    }
}

pub struct StateLock {
    path: PathBuf,
}

impl StateLock {
    pub fn acquire(lock_path: &Path) -> Result<Self> {
        let path = lock_path.with_extension("json.lock");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| AppError::CreateDir {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        for attempt in 0..2 {
            match create_lock_file(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(AppError::LockBusy(_)) if attempt == 0 && remove_stale_lock(&path)? => {
                    continue;
                }
                Err(error) => return Err(error),
            }
        }

        Err(AppError::LockBusy(path))
    }
}

fn create_lock_file(path: &Path) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|source| {
            if source.kind() == std::io::ErrorKind::AlreadyExists {
                AppError::LockBusy(path.to_path_buf())
            } else {
                AppError::Write {
                    path: path.to_path_buf(),
                    source,
                }
            }
        })?;
    writeln!(file, "pid={}", std::process::id()).map_err(|source| AppError::Write {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(())
}

fn remove_stale_lock(path: &Path) -> Result<bool> {
    let Some(pid) = lock_owner_pid(path)? else {
        return Ok(false);
    };
    if process_is_running(pid) {
        return Ok(false);
    }

    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(source) => Err(AppError::Write {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn lock_owner_pid(path: &Path) -> Result<Option<u32>> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(AppError::Read {
                path: path.to_path_buf(),
                source,
            });
        }
    };

    Ok(contents
        .lines()
        .find_map(|line| line.strip_prefix("pid="))
        .and_then(|pid| pid.trim().parse::<u32>().ok()))
}

#[cfg(unix)]
fn process_is_running(pid: u32) -> bool {
    if pid == 0 {
        return true;
    }
    let output = Command::new("kill").arg("-0").arg(pid.to_string()).output();
    match output {
        Ok(output) if output.status.success() => true,
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
            stderr.contains("operation not permitted") || stderr.contains("not permitted")
        }
        Err(_) => true,
    }
}

#[cfg(not(unix))]
fn process_is_running(_pid: u32) -> bool {
    true
}

impl Drop for StateLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::error::AppError;

    use super::{LockFile, StateLock};

    #[test]
    fn missing_lock_file_loads_default() {
        let dir = tempfile::tempdir().unwrap();
        let lock = LockFile::load(&dir.path().join("lock.json")).unwrap();

        assert_eq!(lock.version, 1);
        assert!(lock.models.is_empty());
    }

    #[test]
    fn current_process_lock_is_busy() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join("lock.json");
        let sidecar = lock_path.with_extension("json.lock");
        fs::write(&sidecar, format!("pid={}\n", std::process::id())).unwrap();

        let error = match StateLock::acquire(&lock_path) {
            Ok(_) => panic!("current process lock should be busy"),
            Err(error) => error,
        };

        assert!(matches!(error, AppError::LockBusy(path) if path == sidecar));
    }

    #[test]
    fn stale_process_lock_is_recovered() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join("lock.json");
        let sidecar = lock_path.with_extension("json.lock");
        fs::write(&sidecar, "pid=999999999\n").unwrap();

        let _guard = StateLock::acquire(&lock_path).unwrap();
        let contents = fs::read_to_string(&sidecar).unwrap();

        assert_eq!(contents, format!("pid={}\n", std::process::id()));
    }
}
