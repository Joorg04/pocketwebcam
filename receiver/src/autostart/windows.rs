//! Windows: a value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.

use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{
    RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ,
};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE: &str = "Pocket Webcam";

fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(Some(0)).collect()
}

pub fn is_enabled() -> bool {
    let (key, value) = (wide(RUN_KEY), wide(VALUE));
    let mut size = 0u32;
    // SAFETY: a size query; no buffer is written.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        )
    };
    status == ERROR_SUCCESS
}

/// Adds or removes the login entry. `exe` is what runs at login (with `--minimized`).
pub fn set_enabled(enabled: bool, exe: &Path) -> io::Result<()> {
    let (key, value) = (wide(RUN_KEY), wide(VALUE));
    // SAFETY: all strings are NUL-terminated UTF-16 that outlive the calls.
    let status = unsafe {
        if enabled {
            let command = wide(command_line(exe));
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                value.as_ptr(),
                REG_SZ,
                command.as_ptr().cast(),
                (command.len() * 2) as u32,
            )
        } else {
            match RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr()) {
                windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND => ERROR_SUCCESS, // already off
                other => other,
            }
        }
    };
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(status as i32))
    }
}

fn command_line(exe: &Path) -> String {
    format!("\"{}\" --minimized", exe.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_starts_minimized_with_quoted_path() {
        assert_eq!(
            command_line(Path::new(r"C:\Program Files\Pocket Webcam\pocketwebcam-app.exe")),
            r#""C:\Program Files\Pocket Webcam\pocketwebcam-app.exe" --minimized"#
        );
    }
}
