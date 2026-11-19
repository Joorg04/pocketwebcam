//! Linux: the virtual microphone as a PipeWire / PulseAudio pipe source.
//!
//! `pactl load-module module-pipe-source` makes a microphone that reads raw PCM from a
//! FIFO; we write the phone's samples into it. This works the same on PipeWire
//! (pipewire-pulse) and plain PulseAudio. (`pipewiresink` as an `Audio/Source` crashes
//! on PipeWire 1.6, see docs/PHASE4_MICROPHONE.md.)
//!
//! The microphone stays until logout, like the virtual camera: quitting or restarting
//! the app never removes it, so Discord, Zoom and browsers don't lose it and quietly
//! switch to another mic (they don't switch back when it reappears). While the app isn't
//! running, the module plays silence. The next start reuses it.
//!
//! The FIFO is the jitter buffer. It's shrunk to one page (~42 ms) and written without
//! blocking, so audio can never pile up: a burst after a Wi-Fi stall is dropped instead
//! of adding delay, and when nothing arrives the module plays silence.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};

use super::{MicStats, DESCRIPTION, SAMPLE_RATE};
use log::{info, warn};

pub const SOURCE_NAME: &str = "pocketwebcam_mic";
/// Smallest pipe Linux allows: 2048 mono samples, ~42 ms.
const PIPE_BYTES: libc::c_int = 4096;
/// After running dry, hold back this much before writing again so small hiccups
/// don't each cause a click.
const PRIME_BYTES: usize = 3 * 960; // 30 ms
/// Chunks closer together than this are one stream; a longer gap is a new start.
const FLOWING: Duration = Duration::from_millis(100);

pub struct VirtualMic {
    fifo: File,
    /// Gain as a linear factor, stored as f32 bits.
    gain: AtomicU32,
    peak: AtomicU32,
    last_push: Mutex<Option<Instant>>,
    /// Samples held back while priming after an underrun.
    pending: Mutex<Vec<u8>>,
    dropped: AtomicU64,
    underruns: AtomicU64,
}

impl VirtualMic {
    /// Opens the microphone, creating it if this login session doesn't have one yet.
    pub fn create() -> Result<VirtualMic> {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let path = dir.join("pocketwebcam-mic.fifo");
        let fifo = match reuse_existing(&path) {
            Some(fifo) => {
                info!("reusing the virtual microphone \"{DESCRIPTION}\"");
                fifo
            }
            None => {
                let module = load(&path)?;
                match open_fifo(&path) {
                    Ok(fifo) => fifo,
                    Err(e) => {
                        unload(&module);
                        return Err(e);
                    }
                }
            }
        };
        // SAFETY: plain fcntl on a descriptor we own.
        if unsafe { libc::fcntl(fifo.as_raw_fd(), libc::F_SETPIPE_SZ, PIPE_BYTES) } < 0 {
            warn!("could not shrink the microphone pipe; audio may lag");
        }
        info!("virtual microphone \"{DESCRIPTION}\" ready");
        Ok(VirtualMic {
            fifo,
            gain: AtomicU32::new(1.0f32.to_bits()),
            peak: AtomicU32::new(0),
            last_push: Mutex::new(None),
            pending: Mutex::new(Vec::new()),
            dropped: AtomicU64::new(0),
            underruns: AtomicU64::new(0),
        })
    }

    pub fn set_gain_db(&self, db: f32) {
        let factor = 10f32.powf(db.clamp(0.0, 24.0) / 20.0);
        self.gain.store(factor.to_bits(), Ordering::Relaxed);
    }

    /// Plays one chunk of mono s16le samples from the phone. Never blocks.
    pub fn push(&self, samples: &[u8]) {
        let gain = f32::from_bits(self.gain.load(Ordering::Relaxed));
        let mut chunk = Vec::with_capacity(samples.len());
        let mut peak = 0i32;
        for s in samples.as_chunks::<2>().0 {
            let v = i16::from_le_bytes(*s);
            let v = if gain == 1.0 { v } else { (v as f32 * gain).clamp(-32768.0, 32767.0) as i16 };
            peak = peak.max((v as i32).abs());
            chunk.extend_from_slice(&v.to_le_bytes());
        }
        self.peak.store((peak as f32 / 32768.0).to_bits(), Ordering::Relaxed);
        let flowing = self.last_push.lock().unwrap().replace(Instant::now()).is_some_and(|t| t.elapsed() < FLOWING);

        let mut pending = self.pending.lock().unwrap();
        if pending.is_empty() && self.buffered_bytes() > 0 {
            self.write(&chunk);
            return;
        }
        // The pipe ran dry: wait until a few chunks are in hand, then send them together.
        if pending.is_empty() && flowing {
            self.underruns.fetch_add(1, Ordering::Relaxed);
        }
        pending.extend_from_slice(&chunk);
        if pending.len() >= PRIME_BYTES {
            let data = std::mem::take(&mut *pending);
            self.write(&data);
        }
    }

    pub fn stats(&self) -> MicStats {
        let idle_s = self.last_push.lock().unwrap().map_or(f32::INFINITY, |t| t.elapsed().as_secs_f32());
        MicStats {
            peak: if idle_s < 0.25 { f32::from_bits(self.peak.load(Ordering::Relaxed)) } else { 0.0 },
            idle_s,
            buffered_ms: self.buffered_bytes() as f32 / 2.0 / SAMPLE_RATE as f32 * 1000.0,
            dropped: self.dropped.load(Ordering::Relaxed),
            underruns: self.underruns.load(Ordering::Relaxed),
        }
    }

    /// Whole chunks only: a partial write would shift every later sample by a byte.
    fn write(&self, data: &[u8]) {
        let free = (PIPE_BYTES as usize).saturating_sub(self.buffered_bytes());
        let fits = free.min(data.len()) / 960 * 960;
        if fits < data.len() {
            self.dropped.fetch_add(((data.len() - fits) / 960) as u64, Ordering::Relaxed);
        }
        if fits > 0 {
            // Keep the newest audio.
            if let Err(e) = (&self.fifo).write_all(&data[data.len() - fits..]) {
                if e.kind() != std::io::ErrorKind::WouldBlock {
                    warn!("microphone write failed: {e}");
                }
            }
        }
    }

    /// Bytes waiting in the pipe for the sound server to read.
    fn buffered_bytes(&self) -> usize {
        let mut n: libc::c_int = 0;
        // SAFETY: FIONREAD writes one int; works on either end of a Linux pipe.
        if unsafe { libc::ioctl(self.fifo.as_raw_fd(), libc::FIONREAD, &mut n) } < 0 {
            return 0;
        }
        n.max(0) as usize
    }
}

/// Creates the module-pipe-source microphone reading from `path`; returns its module id.
fn load(path: &Path) -> Result<String> {
    let out = Command::new("pactl")
        .args([
            "load-module",
            "module-pipe-source",
            &format!("source_name={SOURCE_NAME}"),
            &format!("file={}", path.display()),
            "format=s16le",
            &format!("rate={SAMPLE_RATE}"),
            "channels=1",
            &format!("source_properties='device.description=\"{DESCRIPTION}\" device.icon_name=audio-input-microphone'"),
        ])
        .output()
        .map_err(|e| anyhow!("pactl is not installed ({e}); it comes with PipeWire or PulseAudio"))?;
    if !out.status.success() {
        bail!("could not create the microphone: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn open_fifo(path: &Path) -> Result<File> {
    OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))
}

/// Opens the microphone an earlier run created. Anything unusable (another FIFO path, a
/// duplicate, a FIFO that won't open) is unloaded so a fresh one can take its place.
fn reuse_existing(path: &Path) -> Option<File> {
    let out = Command::new("pactl").args(["list", "modules", "short"]).output().ok()?;
    let ours = format!("source_name={SOURCE_NAME} ");
    let same_file = format!("file={} ", path.display());
    let mut fifo = None;
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if !(line.contains("module-pipe-source") && line.contains(&ours)) {
            continue;
        }
        let Some(id) = line.split_whitespace().next() else { continue };
        if fifo.is_none() && line.contains(&same_file) {
            if let Ok(f) = open_fifo(path) {
                fifo = Some(f);
                continue;
            }
        }
        info!("removing an unusable Pocket Webcam microphone (module {id})");
        unload(id);
    }
    fifo
}

fn unload(module: &str) {
    let _ = Command::new("pactl").args(["unload-module", module]).output();
}
