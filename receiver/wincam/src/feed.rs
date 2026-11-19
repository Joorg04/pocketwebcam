//! Where the camera's pictures come from: the Pocket Webcam app's frames in shared memory,
//! or a placeholder (a dark frame with the yellow lens logo) while the app isn't running.

use std::time::{Duration, Instant};

use windows::core::{w, HSTRING};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, INVALID_HANDLE_VALUE, LocalFree};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::System::Memory::{
    CreateFileMappingW, MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_READ, FILE_MAP_WRITE,
    MEMORY_MAPPED_VIEW_ADDRESS, PAGE_READWRITE,
};
use windows::Win32::System::SystemInformation::GetTickCount64;

use crate::shared::{self, View};

/// How often to look for the shared memory again while it isn't there.
const RETRY: Duration = Duration::from_secs(1);

pub struct Feed {
    mapping: Option<Mapping>,
    last_try: Option<Instant>,
    /// The placeholder at the current output size.
    placeholder: Option<(usize, usize, Vec<u8>)>,
}

struct Mapping {
    handle: HANDLE,
    address: MEMORY_MAPPED_VIEW_ADDRESS,
    view: View,
}

// SAFETY: the handle and the view are only used through `&mut Feed`.
unsafe impl Send for Mapping {}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: both were created by `Mapping::open_or_create` and are released once.
        unsafe {
            let _ = UnmapViewOfFile(self.address);
            let _ = CloseHandle(self.handle);
        }
    }
}

impl Mapping {
    /// Opens the shared memory, or creates it. Creating a `Global\` object needs the
    /// "create global objects" right, which the Frame Server (LocalService) has; inside an
    /// ordinary app only opening works.
    fn open_or_create() -> Option<Mapping> {
        let name = HSTRING::from(shared::MAPPING_NAME);
        // SAFETY: plain Win32 calls; every handle is closed on the failure paths.
        unsafe {
            let (handle, created) = match OpenFileMappingW((FILE_MAP_READ | FILE_MAP_WRITE).0, false, &name) {
                Ok(h) => (h, false),
                Err(_) => (create(&name)?, true),
            };
            let address = MapViewOfFile(handle, FILE_MAP_READ | FILE_MAP_WRITE, 0, 0, shared::TOTAL_BYTES);
            if address.Value.is_null() {
                let _ = CloseHandle(handle);
                return None;
            }
            let view = View::new(address.Value.cast());
            if created {
                view.header().magic.store(shared::MAGIC, std::sync::atomic::Ordering::Release);
            } else if view.header().magic.load(std::sync::atomic::Ordering::Acquire) != shared::MAGIC {
                let _ = UnmapViewOfFile(address);
                let _ = CloseHandle(handle);
                return None;
            }
            Some(Mapping { handle, address, view })
        }
    }
}

/// Creates the shared memory so the signed-in user's Pocket Webcam app can write to it.
unsafe fn create(name: &HSTRING) -> Option<HANDLE> {
    // Full access for SYSTEM, LocalService and administrators; read/write for the
    // interactive (signed-in) user, which is who runs the app.
    let mut sd = PSECURITY_DESCRIPTOR::default();
    ConvertStringSecurityDescriptorToSecurityDescriptorW(
        w!("D:(A;;GA;;;SY)(A;;GA;;;LS)(A;;GA;;;BA)(A;;GRGW;;;IU)"),
        SDDL_REVISION_1,
        &mut sd,
        None,
    )
    .ok()?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.0,
        bInheritHandle: false.into(),
    };
    let size = shared::TOTAL_BYTES as u64;
    let handle = CreateFileMappingW(
        INVALID_HANDLE_VALUE,
        Some(&attributes),
        PAGE_READWRITE,
        (size >> 32) as u32,
        size as u32,
        name,
    );
    let _ = LocalFree(Some(HLOCAL(sd.0)));
    handle.ok()
}

impl Feed {
    pub fn new() -> Feed {
        Feed { mapping: None, last_try: None, placeholder: None }
    }

    /// Fills an NV12 buffer (`width` × `height`, rows `pitch` bytes apart: the Y plane,
    /// then the UV plane) with the newest picture.
    pub fn fill(&mut self, dst: &mut [u8], width: usize, height: usize, pitch: usize) {
        if self.mapping.is_none() && self.last_try.is_none_or(|t| t.elapsed() >= RETRY) {
            self.last_try = Some(Instant::now());
            self.mapping = Mapping::open_or_create();
        }
        // SAFETY: no preconditions.
        let now = unsafe { GetTickCount64() };
        if let Some(m) = &self.mapping {
            let live = m.view.read(now, |sw, sh, src| {
                shared::scale_nv12(src, sw as usize, sh as usize, dst, width, height, pitch)
            });
            if live {
                return;
            }
        }
        if self.placeholder.as_ref().is_none_or(|p| (p.0, p.1) != (width, height)) {
            self.placeholder = Some((width, height, placeholder(width, height)));
        }
        if let Some((w, h, frame)) = &self.placeholder {
            shared::scale_nv12(frame, *w, *h, dst, width, height, pitch);
        }
    }
}

/// A dark frame with the Pocket Webcam lens logo (yellow ring, dark lens) in the middle.
fn placeholder(width: usize, height: usize) -> Vec<u8> {
    // BT.601 limited range: background #1C1C1E, yellow #FFC800, lens #2C2C2E.
    const BG: (u8, u8, u8) = (40, 129, 128);
    const YELLOW: (u8, u8, u8) = (182, 32, 166);
    const LENS: (u8, u8, u8) = (54, 129, 128);
    let mut frame = vec![0u8; width * height * 3 / 2];
    let (cx, cy) = (width as f32 / 2.0, height as f32 / 2.0);
    let outer = height as f32 / 7.0;
    let inner = outer * 0.62;
    let colour = |x: f32, y: f32| {
        let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
        if d <= inner {
            LENS
        } else if d <= outer {
            YELLOW
        } else {
            BG
        }
    };
    let (luma, chroma) = frame.split_at_mut(width * height);
    for y in 0..height {
        for x in 0..width {
            luma[y * width + x] = colour(x as f32, y as f32).0;
        }
    }
    for y in 0..height / 2 {
        for x in 0..width / 2 {
            let c = colour(x as f32 * 2.0 + 1.0, y as f32 * 2.0 + 1.0);
            chroma[y * width + x * 2] = c.1;
            chroma[y * width + x * 2 + 1] = c.2;
        }
    }
    frame
}
