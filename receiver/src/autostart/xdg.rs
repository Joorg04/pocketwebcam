//! Linux and other XDG desktops: an autostart entry (`~/.config/autostart/pocketwebcam.desktop`).
//! The file's existence is the setting.

use std::io;
use std::path::{Path, PathBuf};

const FILE_NAME: &str = "pocketwebcam.desktop";

fn path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("autostart").join(FILE_NAME))
}

pub fn is_enabled() -> bool {
    path().is_some_and(|p| p.exists())
}

/// Adds or removes the autostart entry. `exe` is what runs at login (with `--minimized`).
pub fn set_enabled(enabled: bool, exe: &Path) -> io::Result<()> {
    let path = path().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no home directory"))?;
    if !enabled {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, desktop_entry(exe))
}

/// Quotes a path for the `Exec=` key of a desktop entry.
fn exec_quote(path: &Path) -> String {
    let mut out = String::from("\"");
    for c in path.to_string_lossy().chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

fn desktop_entry(exe: &Path) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Pocket Webcam\n\
         Comment=Use your phone as a webcam\n\
         Exec={} --minimized\n\
         Icon=pocketwebcam\n\
         Terminal=false\n\
         X-GNOME-Autostart-enabled=true\n",
        exec_quote(exe)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_starts_minimized_with_quoted_path() {
        let entry = desktop_entry(Path::new("/home/a b/.local/bin/pocketwebcam-app"));
        assert!(entry.contains("Exec=\"/home/a b/.local/bin/pocketwebcam-app\" --minimized\n"));
    }

    #[test]
    fn enable_then_disable_round_trip() {
        let dir = std::env::temp_dir().join(format!("pwc-autostart-test-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        assert!(!is_enabled());
        set_enabled(true, Path::new("/usr/bin/pocketwebcam-app")).unwrap();
        assert!(is_enabled());
        assert!(dir.join("autostart/pocketwebcam.desktop").exists());
        set_enabled(false, Path::new("/usr/bin/pocketwebcam-app")).unwrap();
        assert!(!is_enabled());
        set_enabled(false, Path::new("/usr/bin/pocketwebcam-app")).unwrap(); // already off: fine
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn quoting_escapes_special_characters() {
        assert_eq!(exec_quote(Path::new("/x/$y\"z")), "\"/x/\\$y\\\"z\"");
    }
}
