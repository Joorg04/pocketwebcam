//! Windows: the "Pocket Webcam" virtual camera.
//!
//! The camera itself is a DLL (the `wincam` crate) that Windows loads whenever an app opens
//! the camera. This side registers the camera with Windows and writes the finished frames
//! into the shared memory the DLL reads from.

use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use log::info;
use windows::core::HSTRING;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Media::MediaFoundation::{
    MFCreateVirtualCamera, MFStartup, MFVirtualCameraAccess_CurrentUser, MFVirtualCameraLifetime_System,
    MFVirtualCameraType_SoftwareCameraSource, MFSTARTUP_FULL, MF_VERSION,
};
use windows::Win32::System::Memory::{
    MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_READ, FILE_MAP_WRITE, MEMORY_MAPPED_VIEW_ADDRESS,
};
use windows::Win32::System::SystemInformation::GetTickCount64;

// The DLL's reading half of this file isn't used here.
#[allow(dead_code)]
#[path = "../wincam/src/shared.rs"]
mod shared;

pub use shared::{MAX_HEIGHT, MAX_WIDTH};

/// How often to look for the shared memory while no app is using the camera.
const RETRY: Duration = Duration::from_secs(1);

/// Makes sure Windows knows the camera. With lifetime "System" it stays registered (and in
/// every app's camera list) after the app exits, so this only changes anything the first
/// time; it needs no administrator rights. Fails if the camera DLL isn't installed.
pub fn register_camera() -> Result<()> {
    // SAFETY: plain Media Foundation calls; the camera object is released at the end.
    unsafe {
        MFStartup(MF_VERSION, MFSTARTUP_FULL)?;
        let camera = MFCreateVirtualCamera(
            MFVirtualCameraType_SoftwareCameraSource,
            MFVirtualCameraLifetime_System,
            MFVirtualCameraAccess_CurrentUser,
            &HSTRING::from(shared::CAMERA_NAME),
            &HSTRING::from(shared::CLSID_STR),
            None,
        )
        .map_err(|e| anyhow!("Windows refused the virtual camera: {e}"))?;
        camera
            .Start(None)
            .map_err(|e| anyhow!("the Pocket Webcam camera isn't installed on this PC ({e})"))?;
    }
    info!("virtual camera \"{}\" registered", shared::CAMERA_NAME);
    Ok(())
}

/// Takes the camera out of every app's camera list again. The uninstaller runs this before
/// it removes the DLL; otherwise a dead "Pocket Webcam" camera would stay listed.
pub fn remove_camera() -> Result<()> {
    // SAFETY: plain Media Foundation calls; the camera object is released at the end.
    unsafe {
        MFStartup(MF_VERSION, MFSTARTUP_FULL)?;
        let camera = MFCreateVirtualCamera(
            MFVirtualCameraType_SoftwareCameraSource,
            MFVirtualCameraLifetime_System,
            MFVirtualCameraAccess_CurrentUser,
            &HSTRING::from(shared::CAMERA_NAME),
            &HSTRING::from(shared::CLSID_STR),
            None,
        )
        .map_err(|e| anyhow!("Windows refused the virtual camera: {e}"))?;
        camera.Remove().map_err(|e| anyhow!("removing the virtual camera failed: {e}"))?;
    }
    info!("virtual camera \"{}\" removed", shared::CAMERA_NAME);
    Ok(())
}

/// Writes frames for the camera. Cheap while no app uses the camera: the shared memory
/// only exists then, and until it does, frames are dropped.
#[derive(Default)]
pub struct Writer {
    mapping: Option<Mapping>,
    last_try: Option<Instant>,
}

struct Mapping {
    handle: HANDLE,
    address: MEMORY_MAPPED_VIEW_ADDRESS,
    view: shared::View,
}

// SAFETY: only used through `&mut Writer`.
unsafe impl Send for Mapping {}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: created in `open`, released once.
        unsafe {
            let _ = UnmapViewOfFile(self.address);
            let _ = CloseHandle(self.handle);
        }
    }
}

fn open() -> Option<Mapping> {
    // SAFETY: plain Win32 calls; the handle is closed on failure.
    unsafe {
        let handle = OpenFileMappingW((FILE_MAP_READ | FILE_MAP_WRITE).0, false, &HSTRING::from(shared::MAPPING_NAME)).ok()?;
        let address = MapViewOfFile(handle, FILE_MAP_READ | FILE_MAP_WRITE, 0, 0, shared::TOTAL_BYTES);
        if address.Value.is_null() {
            let _ = CloseHandle(handle);
            return None;
        }
        let view = shared::View::new(address.Value.cast());
        if view.header().magic.load(std::sync::atomic::Ordering::Acquire) != shared::MAGIC {
            log::warn!("the installed camera doesn't match this app version; reinstall Pocket Webcam");
            let _ = UnmapViewOfFile(address);
            let _ = CloseHandle(handle);
            return None;
        }
        info!("an app opened the Pocket Webcam camera");
        Some(Mapping { handle, address, view })
    }
}

impl Writer {
    /// Hands one NV12 frame to the camera.
    pub fn write(&mut self, width: u32, height: u32, nv12: &[u8]) {
        if self.mapping.is_none() && self.last_try.is_none_or(|t| t.elapsed() >= RETRY) {
            self.last_try = Some(Instant::now());
            self.mapping = open();
        }
        if let Some(m) = &self.mapping {
            // SAFETY: no preconditions.
            let now = unsafe { GetTickCount64() };
            m.view.write(width, height, nv12, now);
        }
    }
}
