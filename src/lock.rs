use crate::config::Config;
use crate::errors::{MeldError, Result};
use std::io::Write;
use std::path::PathBuf;

/// Advisory single-instance lock. Created with O_EXCL; removed on drop.
/// Stale locks (process no longer alive) are reclaimed automatically.
pub struct Lock {
    path: PathBuf,
}

impl Lock {
    pub fn acquire(cfg: &Config) -> Result<Lock> {
        let path = cfg.lock_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut f) => {
                let _ = write!(f, "{}", std::process::id());
                Ok(Lock { path })
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if lock_is_stale(&path) {
                    std::fs::remove_file(&path)?;
                    return Lock::acquire(cfg);
                }
                Err(MeldError::Locked(path.display().to_string()).into())
            }
            Err(e) => Err(e.into()),
        }
    }
}

fn lock_is_stale(path: &std::path::Path) -> bool {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return true;
    };
    let Ok(pid) = raw.trim().parse::<u32>() else {
        return true;
    };
    !process_alive(pid)
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    // kill(pid, 0) checks process existence without signaling.
    unsafe extern "C" {
        #[link_name = "kill"]
        fn libc_kill(pid: i32, sig: i32) -> i32;
    }
    unsafe { libc_kill(pid as i32, 0) == 0 }
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut std::ffi::c_void;
        fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
    }
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return false;
        }
        CloseHandle(h);
        true
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
