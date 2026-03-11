use std::path::Path;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};

/// Rotate log files in `dir`: keep last 3 numbered backups.
/// debug.log.3 → deleted
/// debug.log.2 → debug.log.3
/// debug.log.1 → debug.log.2
/// debug.log   → debug.log.1
pub fn rotate_logs(dir: &Path) {
    const KEEP: u32 = 3;
    // Delete oldest
    let _ = std::fs::remove_file(dir.join(format!("debug.log.{}", KEEP)));
    // Shift numbered backups downward (highest first to avoid clobbering)
    for i in (1..KEEP).rev() {
        let from = dir.join(format!("debug.log.{}", i));
        let to = dir.join(format!("debug.log.{}", i + 1));
        let _ = std::fs::rename(from, to);
    }
    // Move current log to .1
    let _ = std::fs::rename(dir.join("debug.log"), dir.join("debug.log.1"));
}

/// Initialise tracing. When `debug` is true:
/// - Rotates `.codescout/debug.log` (keeps last 3)
/// - Adds a file layer at DEBUG level alongside the stderr INFO layer
/// - Returns a `WorkerGuard` that MUST be held for the lifetime of `main`
///   (dropping it flushes the non-blocking writer)
///
/// When `debug` is false, only the stderr INFO layer is installed.
/// Returns `None` in that case.
pub fn init(debug: bool) -> Option<WorkerGuard> {
    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")));

    if debug {
        let log_dir = std::env::current_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."))
            .join(".codescout");

        if let Err(e) = std::fs::create_dir_all(&log_dir) {
            eprintln!("codescout: could not create log directory: {e}");
        }

        rotate_logs(&log_dir);

        match std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(log_dir.join("debug.log"))
        {
            Ok(file) => {
                let (non_blocking, guard) = tracing_appender::non_blocking(file);
                let file_layer = tracing_subscriber::fmt::layer()
                    .with_writer(non_blocking)
                    .with_ansi(false)
                    .with_filter(EnvFilter::new("debug"));

                tracing_subscriber::registry()
                    .with(stderr_layer)
                    .with(file_layer)
                    .try_init()
                    .ok();

                return Some(guard);
            }
            Err(e) => {
                eprintln!("codescout: could not open debug log, falling back to stderr only: {e}");
            }
        }
    }

    tracing_subscriber::registry()
        .with(stderr_layer)
        .try_init()
        .ok();
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotate_keeps_last_3() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();

        // Populate 4 log files with their own name as content (for verification)
        for name in &["debug.log", "debug.log.1", "debug.log.2", "debug.log.3"] {
            std::fs::write(p.join(name), name.as_bytes()).unwrap();
        }

        rotate_logs(p);

        // Original debug.log.3 is deleted — no debug.log.4 should exist
        assert!(!p.join("debug.log.4").exists());
        // debug.log.3 now contains original debug.log.2 content
        assert_eq!(
            std::fs::read_to_string(p.join("debug.log.3")).unwrap(),
            "debug.log.2"
        );
        // debug.log.2 now contains original debug.log.1 content
        assert_eq!(
            std::fs::read_to_string(p.join("debug.log.2")).unwrap(),
            "debug.log.1"
        );
        // debug.log.1 now contains original debug.log content
        assert_eq!(
            std::fs::read_to_string(p.join("debug.log.1")).unwrap(),
            "debug.log"
        );
        // debug.log itself is gone (renamed to .1)
        assert!(!p.join("debug.log").exists());
    }

    #[test]
    fn rotate_works_when_no_files_exist() {
        let dir = tempfile::tempdir().unwrap();
        rotate_logs(dir.path()); // Must not panic
    }

    #[test]
    fn rotate_works_with_only_current_log() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        std::fs::write(p.join("debug.log"), b"hello").unwrap();
        rotate_logs(p);
        assert!(!p.join("debug.log").exists());
        assert_eq!(
            std::fs::read_to_string(p.join("debug.log.1")).unwrap(),
            "hello"
        );
    }
}
