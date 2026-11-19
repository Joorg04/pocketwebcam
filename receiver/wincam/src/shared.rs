//! What the Pocket Webcam app and the camera DLL share. The DLL (running inside Windows'
//! Frame Server) creates the shared memory when an app starts using the camera; the
//! Pocket Webcam app opens it and writes every finished frame into it.
//!
//! This one file is compiled into both crates (the app includes it with `#[path]`), so
//! the layout can't drift apart. It has no Windows dependencies; times come in as
//! milliseconds from `GetTickCount64`, which is the same clock in every process.
//!
//! Layout: a 4 KiB header, then `SLOTS` NV12 frames of up to `MAX_WIDTH` × `MAX_HEIGHT`.
//! The writer fills the slot after `latest` and then publishes it, so the reader always
//! copies a finished frame while the next one is being written elsewhere. Each slot has a
//! sequence number that is odd while it's being written (a seqlock).

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// The camera's COM class, registered by the DLL and passed to `MFCreateVirtualCamera`.
pub const CLSID: u128 = 0xf87bc1a7_82ec_4dcf_b475_3f183a2f2f83;
pub const CLSID_STR: &str = "{f87bc1a7-82ec-4dcf-b475-3f183a2f2f83}";
/// The name apps see (Windows appends "(Windows Virtual Camera)").
pub const CAMERA_NAME: &str = "Pocket Webcam";

pub const MAPPING_NAME: &str = r"Global\PocketWebcamCamera";
/// "PWC1": changes whenever the layout does, so an old app and a new DLL don't mix.
pub const MAGIC: u32 = u32::from_le_bytes(*b"PWC1");
pub const MAX_WIDTH: u32 = 1920;
pub const MAX_HEIGHT: u32 = 1080;
pub const SLOTS: usize = 3;
pub const SLOT_BYTES: usize = (MAX_WIDTH * MAX_HEIGHT * 3 / 2) as usize;
pub const HEADER_BYTES: usize = 4096;
pub const TOTAL_BYTES: usize = HEADER_BYTES + SLOTS * SLOT_BYTES;
/// A frame older than this isn't live any more; the camera shows its placeholder.
pub const STALE_MS: u64 = 1500;

#[repr(C)]
pub struct Slot {
    /// Odd while the writer is filling the slot.
    pub seq: AtomicU64,
    pub width: AtomicU32,
    pub height: AtomicU32,
}

#[repr(C)]
pub struct Header {
    /// Set by whoever creates the memory.
    pub magic: AtomicU32,
    /// Index of the newest finished slot.
    pub latest: AtomicU32,
    /// Frames written so far (0 = none yet).
    pub frames: AtomicU64,
    /// When the writer last finished a frame.
    pub written_ms: AtomicU64,
    /// When the camera last took a frame: someone is watching.
    pub read_ms: AtomicU64,
    pub slots: [Slot; SLOTS],
}

const _: () = assert!(std::mem::size_of::<Header>() <= HEADER_BYTES);

/// A mapped view of the shared memory.
pub struct View {
    base: *mut u8,
}

// SAFETY: all shared state is reached through atomics, and frame bytes are guarded by
// the per-slot sequence numbers.
unsafe impl Send for View {}

impl View {
    /// # Safety
    /// `base` must point to at least `TOTAL_BYTES` of mapped, page-aligned memory that
    /// stays mapped for as long as the view is used.
    pub unsafe fn new(base: *mut u8) -> Self {
        View { base }
    }

    pub fn header(&self) -> &Header {
        // SAFETY: page-aligned, large enough, and Header is all atomics.
        unsafe { &*(self.base as *const Header) }
    }

    fn slot_ptr(&self, i: usize) -> *mut u8 {
        // SAFETY: i < SLOTS, so this stays inside the mapping.
        unsafe { self.base.add(HEADER_BYTES + i * SLOT_BYTES) }
    }

    /// Writes one NV12 frame (`width` × `height`, tightly packed) and publishes it.
    /// False if the frame is too big for a slot.
    pub fn write(&self, width: u32, height: u32, nv12: &[u8], now_ms: u64) -> bool {
        let len = (width * height * 3 / 2) as usize;
        if width > MAX_WIDTH || height > MAX_HEIGHT || nv12.len() < len {
            return false;
        }
        let h = self.header();
        let i = (h.latest.load(Ordering::Relaxed) as usize + 1) % SLOTS;
        let slot = &h.slots[i];
        slot.seq.fetch_add(1, Ordering::AcqRel); // odd: writing
        slot.width.store(width, Ordering::Relaxed);
        slot.height.store(height, Ordering::Relaxed);
        // SAFETY: `len` ≤ SLOT_BYTES (checked above), and readers skip odd slots.
        unsafe { std::ptr::copy_nonoverlapping(nv12.as_ptr(), self.slot_ptr(i), len) };
        slot.seq.fetch_add(1, Ordering::Release); // even: done
        h.latest.store(i as u32, Ordering::Release);
        h.frames.fetch_add(1, Ordering::Relaxed);
        h.written_ms.store(now_ms, Ordering::Release);
        true
    }

    /// Hands the newest live frame (width, height, NV12 bytes) to `use_frame`. False if
    /// there's none (no writer, or nothing new for `STALE_MS`).
    pub fn read(&self, now_ms: u64, use_frame: impl FnOnce(u32, u32, &[u8])) -> bool {
        let h = self.header();
        h.read_ms.store(now_ms, Ordering::Relaxed);
        if h.frames.load(Ordering::Acquire) == 0 || now_ms.saturating_sub(h.written_ms.load(Ordering::Acquire)) > STALE_MS {
            return false;
        }
        let i = h.latest.load(Ordering::Acquire) as usize % SLOTS;
        let slot = &h.slots[i];
        let seq = slot.seq.load(Ordering::Acquire);
        if seq % 2 == 1 {
            return false;
        }
        let (width, height) = (slot.width.load(Ordering::Relaxed), slot.height.load(Ordering::Relaxed));
        if width == 0 || height == 0 || width > MAX_WIDTH || height > MAX_HEIGHT {
            return false;
        }
        let len = (width * height * 3 / 2) as usize;
        // SAFETY: inside the slot; the writer is two slots away from this one.
        let bytes = unsafe { std::slice::from_raw_parts(self.slot_ptr(i), len) };
        use_frame(width, height, bytes);
        true
    }
}

/// Copies an NV12 frame into a (possibly differently sized) NV12 destination with row
/// pitch `pitch`, scaling with nearest-neighbour sampling. `dst` holds the Y plane
/// (`pitch` × `dh`) followed by the UV plane (`pitch` × `dh / 2`).
pub fn scale_nv12(src: &[u8], sw: usize, sh: usize, dst: &mut [u8], dw: usize, dh: usize, pitch: usize) {
    let (src_y, src_uv) = src.split_at(sw * sh);
    let (dst_y, dst_uv) = dst.split_at_mut(pitch * dh);
    if (sw, sh) == (dw, dh) {
        for y in 0..dh {
            dst_y[y * pitch..y * pitch + dw].copy_from_slice(&src_y[y * sw..(y + 1) * sw]);
        }
        for y in 0..dh / 2 {
            dst_uv[y * pitch..y * pitch + dw].copy_from_slice(&src_uv[y * sw..(y + 1) * sw]);
        }
        return;
    }
    let xs: Vec<usize> = (0..dw).map(|x| x * sw / dw).collect();
    for y in 0..dh {
        let row = &src_y[(y * sh / dh) * sw..];
        let out = &mut dst_y[y * pitch..y * pitch + dw];
        for (o, &sx) in out.iter_mut().zip(&xs) {
            *o = row[sx];
        }
    }
    // UV pairs: sample whole pairs so U and V stay together.
    for y in 0..dh / 2 {
        let row = &src_uv[(y * (sh / 2) / (dh / 2).max(1)) * sw..];
        let out = &mut dst_uv[y * pitch..y * pitch + dw];
        for x in 0..dw / 2 {
            let sx = (x * (sw / 2) / (dw / 2).max(1)) * 2;
            out[x * 2] = row[sx];
            out[x * 2 + 1] = row[sx + 1];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(w: usize, h: usize, y: u8, uv: u8) -> Vec<u8> {
        let mut v = vec![y; w * h];
        v.extend(std::iter::repeat_n(uv, w * h / 2));
        v
    }

    #[test]
    fn write_then_read_newest_frame() {
        let mut mem = vec![0u64; TOTAL_BYTES / 8]; // 8-byte aligned, like a mapping
        let view = unsafe { View::new(mem.as_mut_ptr().cast()) };
        assert!(!view.read(1000, |_, _, _| panic!("nothing written yet")));
        assert!(view.write(4, 2, &frame(4, 2, 7, 9), 1000));
        assert!(view.write(4, 2, &frame(4, 2, 8, 9), 1010));
        let mut got = None;
        assert!(view.read(1020, |w, h, b| got = Some((w, h, b[0], b.len()))));
        assert_eq!(got, Some((4, 2, 8, 12)));
        assert!(!view.read(1010 + STALE_MS + 1, |_, _, _| panic!("stale frame")));
        assert!(!view.write(MAX_WIDTH + 2, 2, &[0; 64], 0));
    }

    #[test]
    fn scaling_keeps_planes_and_uv_pairs() {
        let mut src = frame(4, 4, 0, 0);
        for (i, b) in src[..16].iter_mut().enumerate() {
            *b = i as u8; // Y = index
        }
        src[16..].copy_from_slice(&[100, 200, 101, 201, 102, 202, 103, 203]); // two UV rows
        let pitch = 4;
        let mut dst = vec![0u8; pitch * 2 * 3 / 2];
        scale_nv12(&src, 4, 4, &mut dst, 2, 2, pitch);
        assert_eq!(&dst[..2], &[0, 2]);
        assert_eq!(&dst[4..6], &[8, 10]);
        assert_eq!(&dst[8..10], &[100, 200], "U and V stay paired");
        let mut same = vec![0u8; 4 * 4 * 3 / 2];
        scale_nv12(&src, 4, 4, &mut same, 4, 4, 4);
        assert_eq!(same, src);
    }
}
