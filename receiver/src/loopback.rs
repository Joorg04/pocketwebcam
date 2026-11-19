//! v4l2loopback device controls (Linux).
//!
//! The device is loaded with `exclusive_caps=1` (Chrome only lists it then), which makes it
//! a camera only while a writer is open. Once our writer closes (app quits, virtual camera
//! stopped) apps like Discord stop seeing it and silently switch to another webcam. With
//! `keep_format` set the device stays a camera, and the timeout image is what readers get
//! while nothing is writing.

use std::fs::OpenOptions;
use std::io;
use std::os::fd::AsRawFd;
use std::path::Path;

// v4l2loopback's private controls (V4L2_CID_USER_BASE + 0xf000 + n).
pub const KEEP_FORMAT: u32 = 0x0098_f900;
pub const TIMEOUT: u32 = 0x0098_f902;
pub const TIMEOUT_IMAGE_IO: u32 = 0x0098_f903;

/// Readers get the timeout image after this long without a frame from a writer.
pub const TIMEOUT_MS: i32 = 1500;

#[repr(C)]
struct V4l2Control {
    id: u32,
    value: i32,
}

/// `_IOWR('V', 28, struct v4l2_control)`.
const VIDIOC_S_CTRL: libc::c_ulong = 0xC008_561C;

pub fn set_control(device: &str, id: u32, value: i32) -> io::Result<()> {
    let file = OpenOptions::new().read(true).write(true).open(device)?;
    let mut ctrl = V4l2Control { id, value };
    // SAFETY: VIDIOC_S_CTRL reads and writes one v4l2_control, which `ctrl` is.
    let ret = unsafe { libc::ioctl(file.as_raw_fd(), VIDIOC_S_CTRL as _, &mut ctrl) };
    if ret < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// The device's current frame size, if its format is set (a writer is open or the
/// format is kept).
pub fn current_size(device: &str) -> Option<(u32, u32)> {
    let real = std::fs::canonicalize(device).ok()?;
    let name = real.file_name()?.to_str()?;
    let format = std::fs::read_to_string(Path::new("/sys/class/video4linux").join(name).join("format")).ok()?;
    parse_size(&format)
}

/// "YUYV:1920x1080@30" -> (1920, 1080).
fn parse_size(format: &str) -> Option<(u32, u32)> {
    let size = format.trim().split_once(':')?.1.split('@').next()?;
    let (w, h) = size.split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

/// Before a writer opens the device: while the format is kept the device announces only
/// CAPTURE, so v4l2sink would refuse it. Readers already streaming keep their buffers.
pub fn prepare(device: &str) {
    if let Err(e) = set_control(device, KEEP_FORMAT, 0) {
        log::debug!("could not release the kept format of {device}: {e}");
    }
}

/// Names of the other programs that have `device` open (readers and writers alike;
/// the kernel doesn't say which is which).
pub fn other_users(device: &str) -> Vec<String> {
    let Ok(target) = std::fs::canonicalize(device) else { return Vec::new() };
    let me = std::process::id().to_string();
    let mut names = Vec::new();
    let Ok(procs) = std::fs::read_dir("/proc") else { return names };
    for entry in procs.flatten() {
        let pid = entry.file_name();
        let Some(pid) = pid.to_str() else { continue };
        if pid == me || !pid.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(fds) = std::fs::read_dir(entry.path().join("fd")) else { continue };
        let uses = fds.flatten().any(|fd| std::fs::read_link(fd.path()).is_ok_and(|l| l == target));
        if uses {
            if let Ok(comm) = std::fs::read_to_string(entry.path().join("comm")) {
                let name = comm.trim().to_string();
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sysfs_format() {
        assert_eq!(parse_size("YUYV:1920x1080@300000/10000\n"), Some((1920, 1080)));
        assert_eq!(parse_size("YUYV:640x480@30"), Some((640, 480)));
        assert_eq!(parse_size(""), None);
        assert_eq!(parse_size("YUYV:axb@30"), None);
    }
}
