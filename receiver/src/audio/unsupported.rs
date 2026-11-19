//! Systems without a virtual microphone yet: the phone's audio has nowhere to go.

use anyhow::{bail, Result};

use super::MicStats;

pub struct VirtualMic;

impl VirtualMic {
    pub fn create() -> Result<VirtualMic> {
        bail!("the phone's microphone is not available on this system yet")
    }

    pub fn set_gain_db(&self, _db: f32) {}

    pub fn push(&self, _samples: &[u8]) {}

    pub fn stats(&self) -> MicStats {
        MicStats::default()
    }
}
