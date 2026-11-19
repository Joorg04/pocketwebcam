//! Where the app keeps its files on each system.

use std::path::PathBuf;

/// Settings and other small files: `~/.config/pocketwebcam` on Linux,
/// `%APPDATA%\Pocket Webcam` on Windows.
pub fn config_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        return std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("Pocket Webcam"));
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home().map(|h| h.join(".config")))?;
    Some(base.join("pocketwebcam"))
}

/// Files that are kept between runs but aren't settings (the log):
/// `~/.local/state/pocketwebcam` on Linux, `%LOCALAPPDATA%\Pocket Webcam` on Windows.
pub fn state_dir() -> PathBuf {
    let dir = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Pocket Webcam"))
    } else {
        std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .filter(|d| d.is_absolute())
            .or_else(|| home().map(|h| h.join(".local").join("state")))
            .map(|d| d.join("pocketwebcam"))
    };
    dir.unwrap_or_else(|| std::env::temp_dir().join("pocketwebcam"))
}

/// Short-lived files such as the single-instance lock. Not shared between users.
pub fn runtime_dir() -> PathBuf {
    let dir = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Pocket Webcam"))
    } else {
        std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from)
    };
    match dir {
        Some(d) if std::fs::create_dir_all(&d).is_ok() => d,
        _ => std::env::temp_dir(),
    }
}

/// Where the distro packages (.deb, .rpm, AUR) put the virtual camera setup script.
const SYSTEM_SETUP_SCRIPT: &str = "/usr/share/pocketwebcam/setup-v4l2loopback.sh";

/// The command that sets up the Linux virtual camera, for hints. The tarball installer puts
/// the script in `~/.local/share` and wins if both are installed (its app is first on PATH).
pub fn loopback_setup_command() -> String {
    let user = home().map(|h| h.join(".local/share/pocketwebcam/setup-v4l2loopback.sh"));
    if user.is_some_and(|p| !p.exists()) && std::path::Path::new(SYSTEM_SETUP_SCRIPT).exists() {
        format!("sudo {SYSTEM_SETUP_SCRIPT}")
    } else {
        "sudo ~/.local/share/pocketwebcam/setup-v4l2loopback.sh".into()
    }
}

/// The user's Videos or Pictures folder.
#[derive(Debug, Clone, Copy)]
pub enum UserDir {
    Videos,
    Pictures,
}

pub fn user_dir(kind: UserDir) -> PathBuf {
    let (key, name) = match kind {
        UserDir::Videos => ("VIDEOS", "Videos"),
        UserDir::Pictures => ("PICTURES", "Pictures"),
    };
    #[cfg(windows)]
    if let Some(dir) = known_folder(kind) {
        return dir;
    }
    let home = home().unwrap_or_else(|| PathBuf::from("."));
    if cfg!(windows) {
        return home.join(name);
    }
    // XDG user folders, from `~/.config/user-dirs.dirs`, else `~/<name>`.
    let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config"));
    let wanted = format!("XDG_{key}_DIR=");
    std::fs::read_to_string(config.join("user-dirs.dirs"))
        .ok()
        .and_then(|text| {
            text.lines().find_map(|line| {
                let value = line.trim().strip_prefix(&wanted)?.trim().trim_matches('"');
                Some(PathBuf::from(value.replacen("$HOME", &home.to_string_lossy(), 1)))
            })
        })
        .unwrap_or_else(|| home.join(name))
}

/// The folder as Windows has it, which may be redirected (OneDrive, another drive).
#[cfg(windows)]
fn known_folder(kind: UserDir) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{FOLDERID_Pictures, FOLDERID_Videos, SHGetKnownFolderPath, KF_FLAG_DEFAULT};

    let id = match kind {
        UserDir::Videos => &FOLDERID_Videos,
        UserDir::Pictures => &FOLDERID_Pictures,
    };
    let mut raw: windows_sys::core::PWSTR = std::ptr::null_mut();
    // SAFETY: on success Windows allocates `raw` (a NUL-terminated string); we free it.
    unsafe {
        let ok = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT as u32, std::ptr::null_mut(), &mut raw) == 0;
        let path = ok.then(|| {
            let len = (0..).take_while(|&i| *raw.add(i) != 0).count();
            PathBuf::from(std::ffi::OsString::from_wide(std::slice::from_raw_parts(raw, len)))
        });
        CoTaskMemFree(raw as *const _);
        path
    }
}

fn home() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}

/// Points GStreamer at the copy bundled next to the program, if there is one (the Windows
/// zip and installer ship it as `gstreamer/plugins` plus `gst-plugin-scanner.exe` beside
/// the `.exe`). Without one, the system's GStreamer is used. Call before `gst::init()` and
/// before starting any threads.
pub fn use_bundled_gstreamer() {
    let Some(dir) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(PathBuf::from)) else { return };
    let plugins = dir.join("gstreamer").join("plugins");
    if !plugins.is_dir() {
        return;
    }
    std::env::set_var("GST_PLUGIN_SYSTEM_PATH_1_0", &plugins);
    let scanner = dir.join(if cfg!(windows) { "gst-plugin-scanner.exe" } else { "gst-plugin-scanner" });
    if scanner.exists() {
        std::env::set_var("GST_PLUGIN_SCANNER_1_0", scanner);
    }
    // Keep the plugin cache out of the program folder (Program Files isn't writable).
    std::env::set_var("GST_REGISTRY_1_0", runtime_dir().join("gstreamer-registry.bin"));
}
