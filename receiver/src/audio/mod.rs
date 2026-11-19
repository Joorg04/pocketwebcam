//! The phone's microphone as a microphone other apps can pick ("Pocket Webcam Microphone").
//!
//! Each system has its own way to make one; they all take 10 ms chunks of 48 kHz mono
//! s16le from the session and report the same stats.

#[cfg(target_os = "linux")]
mod pipe_source;
#[cfg(target_os = "linux")]
pub use pipe_source::VirtualMic;

#[cfg(not(target_os = "linux"))]
mod unsupported;
#[cfg(not(target_os = "linux"))]
pub use unsupported::VirtualMic;

pub const DESCRIPTION: &str = "Pocket Webcam Microphone";
pub const SAMPLE_RATE: u32 = 48_000;

/// What the window shows about the virtual microphone.
#[derive(Debug, Clone, PartialEq)]
pub enum MicStatus {
    Ready,
    Unavailable { reason: String },
}

/// Live numbers for the level meter and the stats popover.
#[derive(Debug, Clone, Copy, Default)]
pub struct MicStats {
    /// Loudest sample of the last chunk, 0.0 to 1.0 (after gain).
    pub peak: f32,
    /// Seconds since the last chunk arrived from the phone.
    pub idle_s: f32,
    /// Audio currently waiting in the pipe, in milliseconds.
    pub buffered_ms: f32,
    /// 10 ms chunks dropped because the pipe was full (bursts, clock drift).
    pub dropped: u64,
    /// Times the pipe ran dry while audio was coming in (late packets).
    pub underruns: u64,
}
