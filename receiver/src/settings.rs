//! Settings the desktop app remembers between runs (`~/.config/pocketwebcam/settings.json`).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Deserializer, Serialize};

use crate::capture::Container;
use crate::effects::Effects;
use crate::scenes::{default_scenes, lenient_scenes, Scene};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Virtual camera size. Phone video is scaled and letterboxed to fit.
    pub width: u32,
    pub height: u32,
    /// Frame rate the virtual camera advertises to apps.
    pub fps: u32,
    /// Decode on the GPU via VA-API (falls back to software if unavailable).
    pub hw_decode: bool,
    /// v4l2loopback device (Linux).
    pub device: String,
    /// Connect to a phone as soon as one shows up.
    pub auto_connect: bool,
    /// Name of the phone that connects by itself; other phones wait to be picked.
    pub last_phone: Option<String>,
    /// That phone's own identifier, which survives renaming it. `None` for a phone app
    /// from before identifiers; such a phone is recognised by its name.
    pub last_phone_id: Option<String>,
    /// Closing the window keeps the app running in the tray.
    pub close_to_tray: bool,
    /// The first-run guide has been shown (or skipped).
    pub onboarded: bool,
    /// Look and picture adjustments applied to the virtual camera. A broken value here
    /// gives the defaults instead of discarding the whole file.
    #[serde(deserialize_with = "lenient_effects")]
    pub effects: Effects,
    /// Folder for recordings and snapshots; `None` uses Videos / Pictures › Pocket Webcam.
    pub captures_dir: Option<String>,
    /// File type for recordings (the video inside is the phone's codec).
    pub capture_format: Container,
    /// The scene slots (`scenes::SLOTS` of them; `None` is an empty slot).
    #[serde(deserialize_with = "lenient_scenes")]
    pub scenes: Vec<Option<Scene>>,
    /// Turn the phone's microphone on when it connects.
    pub mic_auto: bool,
    /// Extra loudness for the phone's microphone, in dB (0 to 24).
    pub mic_gain_db: f32,
}

fn lenient_effects<'de, D: Deserializer<'de>>(d: D) -> Result<Effects, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value::<Effects>(value).unwrap_or_default().sanitized())
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            width: 1920,
            height: 1080,
            fps: 30,
            hw_decode: false,
            device: "/dev/video10".into(),
            auto_connect: true,
            last_phone: None,
            last_phone_id: None,
            close_to_tray: true,
            onboarded: false,
            effects: Effects::default(),
            captures_dir: None,
            capture_format: Container::default(),
            scenes: default_scenes(),
            mic_auto: true,
            mic_gain_db: 0.0,
        }
    }
}

impl Settings {
    fn path() -> Option<PathBuf> {
        Some(crate::paths::config_dir()?.join("settings.json"))
    }

    /// Reads the saved settings; a missing or broken file gives the defaults.
    pub fn load() -> Self {
        Self::path().map(|p| Self::load_from(&p)).unwrap_or_default()
    }

    fn load_from(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(path) = Self::path() else { return };
        self.save_to(&path);
    }

    /// Writes to a temp file next to `path` and renames it over, so a crash mid-write
    /// never leaves a truncated settings file. Each save uses its own temp name, so
    /// saves from different threads don't write into the same file.
    fn save_to(&self, path: &Path) {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let Ok(json) = serde_json::to_vec_pretty(self) else { return };
        // Write through a symlinked settings.json (dotfile managers) instead of replacing the link.
        let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let mut tmp = target.as_os_str().to_owned();
        tmp.push(format!(".{}.{}.tmp", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed)));
        let tmp = PathBuf::from(tmp);
        // create_new refuses an existing file or symlink at the temp path.
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        let Ok(mut f) = opts.open(&tmp) else { return };
        let written = f.write_all(&json).and_then(|()| f.sync_all());
        drop(f);
        let saved = written.and_then(|()| std::fs::rename(&tmp, &target)).is_ok();
        if !saved {
            let _ = std::fs::remove_file(&tmp);
        }
        // Make the rename itself durable across a power loss.
        #[cfg(unix)]
        if let (true, Some(dir)) = (saved, target.parent()) {
            let _ = std::fs::File::open(dir).and_then(|d| d.sync_all());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::{Background, Look};

    #[test]
    fn old_file_without_effects_loads() {
        let s: Settings = serde_json::from_str(r#"{"width":1280,"height":720,"fps":60,"onboarded":true}"#).unwrap();
        assert_eq!((s.width, s.height, s.fps, s.onboarded), (1280, 720, 60, true));
        assert_eq!(s.effects, Effects::default());
    }

    #[test]
    fn garbage_effects_keep_other_settings() {
        let s: Settings =
            serde_json::from_str(r#"{"fps":24,"effects":{"look":"zzz","brightness":0.5}}"#).unwrap();
        assert_eq!(s.fps, 24);
        assert_eq!(s.effects.look, Look::None);
        assert_eq!(s.effects.brightness, 0.5);

        let s: Settings =
            serde_json::from_str(r#"{"effects":{"brightness":5.0,"contrast":-9.0}}"#).unwrap();
        assert_eq!(s.effects.brightness, 1.0);
        assert_eq!(s.effects.contrast, -1.0);

        let s: Settings = serde_json::from_str(r#"{"fps":24,"effects":"nonsense"}"#).unwrap();
        assert_eq!(s.fps, 24);
        assert_eq!(s.effects, Effects::default());
    }

    #[test]
    fn garbage_background_keeps_other_settings() {
        let s: Settings = serde_json::from_str(
            r#"{"fps":24,"onboarded":true,"effects":{"look":"warm","contrast":0.3,"background":{"mode":"zzz"}}}"#,
        )
        .unwrap();
        assert_eq!((s.fps, s.onboarded), (24, true));
        assert_eq!(s.effects.look, Look::Warm);
        assert_eq!(s.effects.contrast, 0.3);
        assert_eq!(s.effects.background, Background::Off);

        let s: Settings = serde_json::from_str(
            r#"{"fps":24,"effects":{"background":{"mode":"image","path":"builtin:office"}}}"#,
        )
        .unwrap();
        assert_eq!(s.effects.background, Background::Image { path: "builtin:office".into() });
    }

    #[test]
    fn effects_round_trip() {
        let mut s = Settings::default();
        s.effects.look = Look::Vivid;
        s.effects.contrast = 0.25;
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn save_then_load_round_trips_without_leftovers() {
        let dir = std::env::temp_dir().join(format!("pocketwebcam-settings-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("pocketwebcam").join("settings.json");

        let mut s = Settings { fps: 24, last_phone: Some("Pixel".into()), ..Settings::default() };
        s.effects.look = Look::Vivid;
        s.save_to(&path);
        assert_eq!(Settings::load_from(&path), s);

        s.fps = 60;
        s.save_to(&path);
        assert_eq!(Settings::load_from(&path), s);

        let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("settings.json")]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn save_writes_through_a_symlink() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("pocketwebcam-settings-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("dotfiles")).unwrap();
        let real = dir.join("dotfiles").join("settings.json");
        std::fs::write(&real, "{}").unwrap();
        let link = dir.join("settings.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let s = Settings { fps: 15, ..Settings::default() };
        s.save_to(&link);
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(Settings::load_from(&real), s);
        assert_eq!(std::fs::metadata(&real).unwrap().permissions().mode() & 0o777, 0o600);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
