//! Logs to both stderr and a persistent file, and installs a panic hook that writes to
//! the same file. A GUI app's stderr is easy to lose — closed terminal, launched from a
//! desktop shortcut — which is exactly the situation where "the app just vanished, why"
//! needs an answer; a persistent log file (and a panic message captured in it) survives
//! that even when nobody was watching a terminal at the time.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use log::{Level, LevelFilter, Log, Metadata, Record};

struct FileAndStderrLogger {
    file: Mutex<File>,
}

impl Log for FileAndStderrLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Info
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!(
            "[{}] {:5} {}: {}",
            unix_timestamp(),
            record.level(),
            record.target(),
            record.args()
        );
        eprintln!("{line}");
        if let Ok(mut file) = self.file.lock() {
            let _ = writeln!(file, "{line}");
        }
    }

    fn flush(&self) {
        if let Ok(mut file) = self.file.lock() {
            let _ = file.flush();
        }
    }
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `$XDG_STATE_HOME/blueboard/blueboard.log`, falling back to `~/.local/state/...`, then
/// the system temp dir if neither `XDG_STATE_HOME` nor `HOME` is set.
pub fn log_file_path() -> PathBuf {
    let state_dir = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".local/state")))
        .unwrap_or_else(std::env::temp_dir);
    state_dir.join("blueboard").join("blueboard.log")
}

fn open_log_file(path: &Path) -> std::io::Result<File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    OpenOptions::new().create(true).append(true).open(path)
}

/// Installs a panic hook that appends the panic message to the log file before running
/// the default hook (which still prints to stderr as usual) — so a crash is recorded even
/// if nobody sees the terminal at the moment it happens.
fn install_panic_hook(path: PathBuf) {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Ok(mut file) = open_log_file(&path) {
            let _ = writeln!(file, "[{}] PANIC: {info}", unix_timestamp());
        }
        default_hook(info);
    }));
}

/// Sets up logging (stderr + file) and the panic hook, and returns the log file's path so
/// the caller can tell the user where to find it.
pub fn init() -> PathBuf {
    let path = log_file_path();
    let file = open_log_file(&path).expect("failed to open log file");

    log::set_boxed_logger(Box::new(FileAndStderrLogger {
        file: Mutex::new(file),
    }))
    .expect("logger already initialized");
    log::set_max_level(LevelFilter::Info);

    install_panic_hook(path.clone());
    path
}
