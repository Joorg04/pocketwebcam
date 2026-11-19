//! Where log messages go: the terminal, and for the desktop app also a file, so there is
//! something to look at (and send along) when it misbehaves. The app has no terminal when
//! it's started from the launcher, and on Windows it has none at all.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// A log bigger than this is set aside when the app starts, and a new one begins.
const MAX_BYTES: u64 = 1024 * 1024;
pub const FILE_NAME: &str = "pocketwebcam.log";
/// The log before the current one (one is kept).
pub const OLD_FILE_NAME: &str = "pocketwebcam.old.log";

/// The folder with the desktop app's log: `~/.local/state/pocketwebcam` on Linux,
/// `%LOCALAPPDATA%\Pocket Webcam` on Windows.
pub fn log_dir() -> PathBuf {
    crate::paths::state_dir()
}

/// Starts logging. `to_file` also writes everything to `pocketwebcam.log` in `log_dir()`
/// and records crashes there. Set `RUST_LOG=debug` for more detail.
pub fn init(to_file: bool) {
    let mut builder = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"));
    builder.format_target(false);
    let file = if to_file { open(&log_dir()) } else { None };
    let logging_to_file = file.is_some();
    if let Some(file) = file {
        builder.target(env_logger::Target::Pipe(Box::new(Tee { file })));
    }
    if builder.try_init().is_err() || !logging_to_file {
        return;
    }
    log::info!(
        "Pocket Webcam {} on {} ({})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    // A crash on a background thread would otherwise leave no trace outside a terminal.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!("the app hit a bug and may stop working: {info}");
        default_hook(info);
    }));
}

/// Opens the log in `dir` for adding to, first setting a big one aside.
fn open(dir: &Path) -> Option<File> {
    std::fs::create_dir_all(dir).ok()?;
    let path = dir.join(FILE_NAME);
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_BYTES) {
        // Windows doesn't rename over an existing file.
        let old = dir.join(OLD_FILE_NAME);
        let _ = std::fs::remove_file(&old);
        let _ = std::fs::rename(&path, &old);
    }
    OpenOptions::new().create(true).append(true).open(&path).ok()
}

/// Writes log lines to the terminal and the file.
struct Tee {
    file: File,
}

impl Write for Tee {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // Neither may stop the app: there may be no terminal, and the disk may be full.
        let _ = io::stderr().write_all(buf);
        let _ = self.file.write_all(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let _ = io::stderr().flush();
        let _ = self.file.flush();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_and_sets_a_big_log_aside() {
        let dir = std::env::temp_dir().join(format!("pocketwebcam-log-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let mut tee = Tee { file: open(&dir).unwrap() };
        tee.write_all(b"first run\n").unwrap();
        drop(tee);
        let mut tee = Tee { file: open(&dir).unwrap() };
        tee.write_all(b"second run\n").unwrap();
        drop(tee);
        assert_eq!(std::fs::read_to_string(dir.join(FILE_NAME)).unwrap(), "first run\nsecond run\n");
        assert!(!dir.join(OLD_FILE_NAME).exists());

        // Grown past the limit (twice, so an older log is already there): the big one is
        // set aside, replacing the older one, and the next start begins a new file.
        for marker in ["older", "newer"] {
            std::fs::write(dir.join(FILE_NAME), format!("{marker}{}", "x".repeat(MAX_BYTES as usize))).unwrap();
            drop(open(&dir).unwrap());
        }
        assert_eq!(std::fs::metadata(dir.join(FILE_NAME)).unwrap().len(), 0);
        assert!(std::fs::read_to_string(dir.join(OLD_FILE_NAME)).unwrap().starts_with("newer"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
