//! The gateway's log, `~/.minerve/message-apps.log`. Inside minerve's TUI stderr would draw over
//! the screen, so every line goes to this file instead.
//!
//! Past 1 MB the log moves to `message-apps.log.1` (replacing the older one) and starts over, so
//! it never takes more than about 2 MB.

use std::io::Write;
use std::path::{Path, PathBuf};

const MAX_LOG_BYTES: u64 = 1024 * 1024;

pub fn log_path() -> PathBuf {
    dirs::home_dir().expect("no home directory").join(".minerve/message-apps.log")
}

pub fn log(line: impl AsRef<str>) {
    log_to(&log_path(), line.as_ref(), MAX_LOG_BYTES);
}

fn log_to(path: &Path, line: &str, max_bytes: u64) {
    if std::fs::metadata(path).is_ok_and(|meta| meta.len() >= max_bytes) {
        let _ = std::fs::rename(path, rotated_path(path));
    }
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{} {}", seconds_since_epoch(), line);
    }
}

fn rotated_path(path: &Path) -> PathBuf {
    let mut rotated = path.as_os_str().to_owned();
    rotated.push(".1");
    PathBuf::from(rotated)
}

/// Enough to order lines without a date crate.
fn seconds_since_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotates_once_past_the_limit_keeping_one_old_file() {
        let dir = std::env::temp_dir().join(format!("gw-log-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("message-apps.log");
        log_to(&path, "first", 20);
        log_to(&path, "second", 20);
        // "first" alone was under 20 bytes, so both lines share the file; it is now over the limit.
        log_to(&path, "third", 20);
        let current = std::fs::read_to_string(&path).unwrap();
        let old = std::fs::read_to_string(rotated_path(&path)).unwrap();
        assert!(current.ends_with(" third\n") && !current.contains("first"), "{current}");
        assert!(old.contains("first") && old.contains("second"), "{old}");
        log_to(&path, &"x".repeat(30), 20);
        log_to(&path, "fifth", 20);
        // The older rotated file is replaced, never stacked up.
        assert!(!std::fs::read_to_string(rotated_path(&path)).unwrap().contains("first"));
        assert!(!dir.join("message-apps.log.2").exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
