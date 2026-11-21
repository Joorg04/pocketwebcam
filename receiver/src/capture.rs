//! Recordings and snapshots: where they go, what they're called, and the recorder that
//! writes the phone's own H.264/HEVC stream into a file without re-encoding it.
//!
//! Writing the stream as it arrives costs no CPU and keeps the phone's full resolution.
//! The trade-off: looks and other effects (added on this computer) aren't in recordings;
//! snapshots are taken from the virtual camera picture and do include them.
//!
//! Phones that can send audio get an AAC track too (the only thing re-encoded). It's
//! silent wherever the mic is off, so turning the mic on mid-recording still gets recorded.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{anyhow, Context, Result};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use log::{info, warn};
use serde::{Deserialize, Serialize};

use crate::paths::{self, UserDir};
use crate::protocol::{AudioFrame, StreamFormat, VideoFrame};
use crate::settings::Settings;

/// Every capture file starts with this, so the app can find its own files in a shared folder.
pub const FILE_PREFIX: &str = "Pocket Webcam ";
/// How long finishing a file may take before we give up waiting for the muxer.
const FINISH_TIMEOUT: Duration = Duration::from_secs(5);
/// Phone audio, as AUDIO_FRAME carries it.
const AUDIO_CAPS: &str = "audio/x-raw,format=S16LE,rate=48000,channels=1,layout=interleaved";
const AUDIO_RATE: u64 = 48_000;
/// Silence is filled in up to this far behind the video, so the muxer never waits on a
/// mic that's off, while real audio arriving a little late still fits in.
const AUDIO_SLACK_US: u64 = 300_000;

/// File type for recordings. The video inside is always the phone's codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Container {
    /// Plays everywhere; keeps portrait recordings upright.
    #[default]
    Mp4,
    /// Survives a crash mid-recording; portrait recordings may play sideways in some players.
    Mkv,
}

impl Container {
    pub fn extension(self) -> &'static str {
        match self {
            Container::Mp4 => "mp4",
            Container::Mkv => "mkv",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Container::Mp4 => "MP4",
            Container::Mkv => "MKV",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureKind {
    Video,
    Photo,
}

/// One file in the captures folder(s).
#[derive(Debug, Clone, PartialEq)]
pub struct CaptureFile {
    pub path: PathBuf,
    pub kind: CaptureKind,
    pub modified: SystemTime,
}

/// The folder chosen in Settings, if any.
fn custom_dir(settings: &Settings) -> Option<PathBuf> {
    settings.captures_dir.as_deref().filter(|d| !d.trim().is_empty()).map(PathBuf::from)
}

/// Where recordings go: the chosen folder, else Videos › Pocket Webcam.
pub fn videos_dir(settings: &Settings) -> PathBuf {
    custom_dir(settings).unwrap_or_else(|| paths::user_dir(UserDir::Videos).join("Pocket Webcam"))
}

/// Where snapshots go: the chosen folder, else Pictures › Pocket Webcam.
pub fn pictures_dir(settings: &Settings) -> PathBuf {
    custom_dir(settings).unwrap_or_else(|| paths::user_dir(UserDir::Pictures).join("Pocket Webcam"))
}

/// "Pocket Webcam 2026-10-20 17.03.12.mp4", made unique if a file with that name exists.
pub fn new_capture_path(dir: &Path, extension: &str) -> PathBuf {
    let stamp = local_timestamp();
    let mut path = dir.join(format!("{FILE_PREFIX}{stamp}.{extension}"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{FILE_PREFIX}{stamp} ({n}).{extension}"));
        n += 1;
    }
    path
}

/// The local time as "YYYY-MM-DD HH.MM.SS" (dots, because colons upset some file systems).
fn local_timestamp() -> String {
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs() as libc::time_t;
    // SAFETY: localtime_r / localtime_s only write into the struct we pass.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        #[cfg(unix)]
        libc::localtime_r(&now, &mut tm);
        #[cfg(windows)]
        libc::localtime_s(&mut tm, &now);
        tm
    };
    format!(
        "{:04}-{:02}-{:02} {:02}.{:02}.{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

/// The newest `limit` captures in the videos and pictures folders, newest first.
pub fn recent_captures(settings: &Settings, limit: usize) -> Vec<CaptureFile> {
    let mut dirs = vec![videos_dir(settings), pictures_dir(settings)];
    dirs.dedup();
    let mut files: Vec<CaptureFile> = dirs
        .iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            if !name.starts_with(FILE_PREFIX) {
                return None;
            }
            let kind = match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
                "mp4" | "mkv" => CaptureKind::Video,
                "jpg" | "jpeg" | "png" => CaptureKind::Photo,
                _ => return None,
            };
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some(CaptureFile { path, kind, modified })
        })
        .collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.modified));
    files.truncate(limit);
    files
}

/// A recording in progress.
struct Active {
    pipeline: gst::Pipeline,
    src: gst_app::AppSrc,
    audio: Option<AudioTrack>,
    path: PathBuf,
    format: StreamFormat,
    started: Instant,
    /// Phone timestamp of the first frame written; `None` until the first keyframe arrives.
    first_pts: Option<u64>,
}

struct AudioTrack {
    src: gst_app::AppSrc,
    /// End of the audio written so far, in µs from the start of the file.
    written_us: u64,
    /// End of the video written so far, so the sound can be padded to match at the end.
    video_end_us: u64,
}

impl AudioTrack {
    /// Writes `samples` (s16le mono) starting at `at_us`, which must be ≥ `written_us`.
    fn write(&mut self, at_us: u64, samples: Vec<u8>) {
        let n = (samples.len() / 2) as u64;
        let mut buffer = gst::Buffer::from_slice(samples);
        {
            let b = buffer.get_mut().expect("new buffer is writable");
            b.set_pts(gst::ClockTime::from_useconds(at_us));
            b.set_duration(gst::ClockTime::from_useconds(n * 1_000_000 / AUDIO_RATE));
        }
        if self.src.push_buffer(buffer).is_err() {
            warn!("recording pipeline refused audio");
        }
        self.written_us = at_us + n * 1_000_000 / AUDIO_RATE;
    }

    /// Fills silence up to `until_us`.
    fn fill_silence(&mut self, until_us: u64) {
        while self.written_us < until_us {
            // At most a second per buffer; rounded to whole samples.
            let us = (until_us - self.written_us).min(1_000_000);
            let n = (us * AUDIO_RATE / 1_000_000).max(1) as usize;
            self.write(self.written_us, vec![0; n * 2]);
        }
    }
}

/// The first AAC encoder this system has, or `None` (then recordings have no sound).
fn aac_encoder() -> Option<&'static str> {
    ["fdkaacenc", "avenc_aac", "faac"].into_iter().find(|name| gst::ElementFactory::find(name).is_some())
}

/// What the window shows while recording.
#[derive(Debug, Clone)]
pub struct RecordingStatus {
    pub path: PathBuf,
    pub started: Instant,
}

/// Writes the phone's encoded frames to a file. Shared between the window (start / stop) and
/// the connection thread (frames, format changes, disconnects).
#[derive(Default)]
pub struct Recorder {
    active: Mutex<Option<Active>>,
    /// Why the last recording ended on its own, for the window to show once.
    notice: Mutex<Option<String>>,
    /// Files still being finished in the background; each yields its path once written.
    finishing: Mutex<Vec<JoinHandle<Option<PathBuf>>>>,
}

impl Recorder {
    /// Starts writing to `path`. Frames are dropped until the next keyframe, so ask the phone
    /// for one right after. `audio`: the phone can send its microphone, so add a sound track.
    pub fn start(&self, path: PathBuf, container: Container, format: &StreamFormat, audio: bool) -> Result<()> {
        let mut active = self.active.lock().unwrap();
        if active.is_some() {
            return Err(anyhow!("already recording"));
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let (caps, parser) = match format.codec.as_str() {
            "hevc" => ("video/x-h265", "h265parse"),
            _ => ("video/x-h264", "h264parse"),
        };
        let mux = match container {
            Container::Mp4 => "mp4mux",
            Container::Mkv => "matroskamux",
        };
        let flip = if format.mirror { "flip-" } else { "" };
        let orientation = format!("{flip}rotate-{}", format.rotation % 360);
        let encoder = if audio { aac_encoder() } else { None };
        if audio && encoder.is_none() {
            warn!("no AAC encoder found (install gst-libav); recording without sound");
        }
        let audio_branch = encoder.map_or(String::new(), |enc| {
            format!(" appsrc name=asrc is-live=true format=time caps={AUDIO_CAPS} ! audioconvert ! {enc} ! aacparse ! mux.")
        });
        let pipeline = gst::parse::launch(&format!(
            "appsrc name=src is-live=true format=time caps={caps},stream-format=byte-stream,alignment=au \
             ! {parser} ! taginject tags=\"image-orientation={orientation}\" ! {mux} name=mux ! filesink name=file{audio_branch}"
        ))?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow!("recording pipeline is not a pipeline"))?;
        let file = pipeline.by_name("file").ok_or_else(|| anyhow!("recording pipeline has no file sink"))?;
        file.set_property("location", path.to_string_lossy().to_string());
        let src = pipeline
            .by_name("src")
            .and_then(|e| e.downcast::<gst_app::AppSrc>().ok())
            .ok_or_else(|| anyhow!("recording pipeline has no appsrc"))?;
        let audio = pipeline
            .by_name("asrc")
            .and_then(|e| e.downcast::<gst_app::AppSrc>().ok())
            .map(|src| AudioTrack { src, written_us: 0, video_end_us: 0 });
        pipeline.set_state(gst::State::Playing).context("starting the recording")?;
        info!("recording to {}", path.display());
        *active = Some(Active {
            pipeline,
            src,
            audio,
            path,
            format: format.clone(),
            started: Instant::now(),
            first_pts: None,
        });
        Ok(())
    }

    pub fn status(&self) -> Option<RecordingStatus> {
        self.active.lock().unwrap().as_ref().map(|a| RecordingStatus { path: a.path.clone(), started: a.started })
    }

    /// Writes one frame from the phone (called for every frame; cheap when not recording).
    pub fn push(&self, frame: &VideoFrame) {
        let mut guard = self.active.lock().unwrap();
        let Some(active) = guard.as_mut() else { return };
        let first = match active.first_pts {
            Some(first) => first,
            None if frame.keyframe => *active.first_pts.insert(frame.pts_us),
            None => return,
        };
        let mut buffer = gst::Buffer::from_slice(frame.data.to_vec());
        {
            let b = buffer.get_mut().expect("new buffer is writable");
            // The phones never reorder frames (no B-frames), so decode order = display order.
            let pts = gst::ClockTime::from_useconds(frame.pts_us.saturating_sub(first));
            b.set_pts(pts);
            b.set_dts(pts);
            // Without a duration, MP4 can't tell how long the last frame lasts and drops it.
            b.set_duration(gst::ClockTime::from_useconds(1_000_000 / u64::from(active.format.fps.max(1))));
            if !frame.keyframe {
                b.set_flags(gst::BufferFlags::DELTA_UNIT);
            }
        }
        if active.src.push_buffer(buffer).is_err() {
            warn!("recording pipeline refused a frame");
        }
        if let Some(track) = active.audio.as_mut() {
            let at = frame.pts_us.saturating_sub(first);
            track.video_end_us = at + 1_000_000 / u64::from(active.format.fps.max(1));
            track.fill_silence(at.saturating_sub(AUDIO_SLACK_US));
        }
    }

    /// Writes one chunk of the phone's microphone (cheap when not recording).
    pub fn push_audio(&self, frame: &AudioFrame) {
        let mut guard = self.active.lock().unwrap();
        let Some(active) = guard.as_mut() else { return };
        let (Some(first), Some(track)) = (active.first_pts, active.audio.as_mut()) else { return };
        // Before the first keyframe there's no file time yet.
        let Some(at) = frame.pts_us.checked_sub(first) else { return };
        // Audio that's later than the silence already written for that moment is too late.
        // (5 ms tolerance: chunk timestamps wobble a little.)
        if at + 5_000 < track.written_us {
            return;
        }
        track.fill_silence(at);
        track.write(track.written_us.max(at), frame.samples.to_vec());
    }

    /// The phone started a new stream. A different codec, size or orientation can't go into
    /// the same file, so the recording is finished and kept.
    pub fn format_changed(&self, format: &StreamFormat) {
        let same = self.active.lock().unwrap().as_ref().is_none_or(|a| {
            let o = &a.format;
            (&o.codec, o.width, o.height, o.rotation, o.mirror)
                == (&format.codec, format.width, format.height, format.rotation, format.mirror)
        });
        if !same {
            self.finish(Some("the video format changed"));
        }
    }

    /// Stops recording and finishes the file in the background.
    pub fn stop(&self) {
        self.finish(None);
    }

    /// Stops because the phone went away, keeping what was recorded.
    pub fn connection_ended(&self) {
        self.finish(Some("the phone disconnected"));
    }

    fn finish(&self, reason: Option<&str>) {
        let Some(active) = self.active.lock().unwrap().take() else { return };
        if let Some(reason) = reason {
            let name = active.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            *self.notice.lock().unwrap() = Some(format!("Recording stopped because {reason}. Saved {name}."));
        }
        let wrote_frames = active.first_pts.is_some();
        let (pipeline, path) = (active.pipeline, active.path);
        let src = active.src;
        let audio = active.audio.map(|mut a| {
            a.fill_silence(a.video_end_us);
            a.src
        });
        let done = thread::spawn(move || {
            // End of stream makes the muxer write its index; without it an MP4 won't play.
            let _ = src.end_of_stream();
            if let Some(audio) = audio {
                let _ = audio.end_of_stream();
            }
            if let Some(bus) = pipeline.bus() {
                let _ = bus.timed_pop_filtered(
                    gst::ClockTime::from_mseconds(FINISH_TIMEOUT.as_millis() as u64),
                    &[gst::MessageType::Eos, gst::MessageType::Error],
                );
            }
            let _ = pipeline.set_state(gst::State::Null);
            if !wrote_frames {
                // Stopped before the first keyframe: there's nothing worth keeping.
                let _ = std::fs::remove_file(&path);
                return None;
            }
            info!("recording saved: {}", path.display());
            Some(path)
        });
        self.finishing.lock().unwrap().push(done);
    }

    /// Why the last recording stopped on its own, once.
    pub fn take_notice(&self) -> Option<String> {
        self.notice.lock().unwrap().take()
    }

    /// Files that finished writing since the last call.
    pub fn take_saved(&self) -> Vec<PathBuf> {
        let mut finishing = self.finishing.lock().unwrap();
        let (done, still): (Vec<_>, Vec<_>) = finishing.drain(..).partition(|h| h.is_finished());
        *finishing = still;
        done.into_iter().filter_map(|h| h.join().ok().flatten()).collect()
    }

    /// True while a stopped recording is still being written out.
    pub fn is_finishing(&self) -> bool {
        !self.finishing.lock().unwrap().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encodes `n` frames of test video into H.264 access units: (keyframe, data).
    fn encoded_frames(n: u32) -> Vec<(bool, Vec<u8>)> {
        gst::init().unwrap();
        // Ubuntu 22.04 (the Linux release build) has no openh264enc, so fall back to x264.
        let encoder = if gst::ElementFactory::find("openh264enc").is_some() {
            "openh264enc gop-size=10"
        } else {
            "x264enc key-int-max=10 bframes=0 tune=zerolatency"
        };
        let pipeline = gst::parse::launch(&format!(
            "videotestsrc num-buffers={n} ! video/x-raw,width=320,height=240,framerate=30/1 \
             ! {encoder} ! h264parse config-interval=-1 \
             ! video/x-h264,stream-format=byte-stream,alignment=au ! appsink name=out sync=false"
        ))
        .unwrap()
        .downcast::<gst::Pipeline>()
        .unwrap();
        let sink = pipeline.by_name("out").unwrap().downcast::<gst_app::AppSink>().unwrap();
        pipeline.set_state(gst::State::Playing).unwrap();
        let mut frames = Vec::new();
        while let Ok(sample) = sink.pull_sample() {
            let buffer = sample.buffer().unwrap();
            let key = !buffer.flags().contains(gst::BufferFlags::DELTA_UNIT);
            frames.push((key, buffer.map_readable().unwrap().to_vec()));
        }
        pipeline.set_state(gst::State::Null).unwrap();
        frames
    }

    /// Counts the video frames in a recorded file.
    fn frames_in(path: &Path, demux: &str) -> usize {
        let pipeline = gst::parse::launch(&format!("filesrc name=src ! {demux} ! h264parse ! appsink name=out sync=false"))
            .unwrap()
            .downcast::<gst::Pipeline>()
            .unwrap();
        pipeline.by_name("src").unwrap().set_property("location", path.to_string_lossy().to_string());
        let sink = pipeline.by_name("out").unwrap().downcast::<gst_app::AppSink>().unwrap();
        pipeline.set_state(gst::State::Playing).unwrap();
        let mut n = 0;
        while sink.pull_sample().is_ok() {
            n += 1;
        }
        pipeline.set_state(gst::State::Null).unwrap();
        n
    }

    fn format() -> StreamFormat {
        StreamFormat { codec: "h264".into(), width: 320, height: 240, fps: 30, rotation: 90, mirror: false }
    }

    fn wait_saved(recorder: &Recorder) -> Vec<PathBuf> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let saved = recorder.take_saved();
            if !saved.is_empty() || !recorder.is_finishing() {
                return saved;
            }
            assert!(Instant::now() < deadline, "recording never finished");
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn record(tag: &str, container: Container, frames: &[(bool, Vec<u8>)], skip: usize) -> (PathBuf, Vec<PathBuf>) {
        let dir = std::env::temp_dir().join(format!("pw-rec-{tag}-{}-{}", std::process::id(), container.extension()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = new_capture_path(&dir, container.extension());
        let recorder = Recorder::default();
        recorder.start(path.clone(), container, &format(), false).unwrap();
        for (i, (key, data)) in frames.iter().enumerate().skip(skip) {
            recorder.push(&VideoFrame { keyframe: *key, pts_us: 1_000_000 + i as u64 * 33_333, data });
        }
        recorder.stop();
        (path, wait_saved(&recorder))
    }

    #[test]
    fn records_playable_mp4_and_mkv_from_the_first_keyframe() {
        let frames = encoded_frames(40);
        assert!(frames[0].0, "test stream starts with a keyframe");
        // Start mid-GOP: the frames before the next keyframe (index 10) must be skipped.
        for (container, demux) in [(Container::Mp4, "qtdemux"), (Container::Mkv, "matroskademux")] {
            let (path, saved) = record("play", container, &frames, 3);
            assert_eq!(saved, vec![path.clone()]);
            assert_eq!(frames_in(&path, demux), 30, "{container:?}");
            if let Ok(keep) = std::env::var("PW_KEEP_RECORDING") {
                let _ = std::fs::copy(&path, format!("{keep}.{}", container.extension()));
            }
            let _ = std::fs::remove_dir_all(path.parent().unwrap());
        }
    }

    /// Decodes the sound track: (seconds of audio, seconds of it that aren't silent).
    fn audio_in(path: &Path) -> (f64, f64) {
        let pipeline = gst::parse::launch(
            "filesrc name=src ! qtdemux ! aacparse ! avdec_aac ! audioconvert \
             ! audio/x-raw,format=S16LE,channels=1,rate=48000 ! appsink name=out sync=false",
        )
        .unwrap()
        .downcast::<gst::Pipeline>()
        .unwrap();
        pipeline.by_name("src").unwrap().set_property("location", path.to_string_lossy().to_string());
        let sink = pipeline.by_name("out").unwrap().downcast::<gst_app::AppSink>().unwrap();
        pipeline.set_state(gst::State::Playing).unwrap();
        let (mut total, mut loud) = (0usize, 0usize);
        while let Ok(sample) = sink.pull_sample() {
            let map = sample.buffer().unwrap().map_readable().unwrap().to_vec();
            for chunk in map.chunks(960) {
                let n = chunk.len() / 2;
                total += n;
                let peak = chunk.as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes(*b).unsigned_abs()).max().unwrap_or(0);
                if peak > 2000 {
                    loud += n;
                }
            }
        }
        pipeline.set_state(gst::State::Null).unwrap();
        (total as f64 / 48_000.0, loud as f64 / 48_000.0)
    }

    #[test]
    fn records_the_mic_and_fills_silence_where_it_was_off() {
        let frames = encoded_frames(60); // 2 s at 30 fps (also initialises GStreamer)
        if aac_encoder().is_none() {
            eprintln!("skipped: no AAC encoder");
            return;
        }
        let dir = std::env::temp_dir().join(format!("pw-rec-audio-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = new_capture_path(&dir, "mp4");
        let recorder = Recorder::default();
        recorder.start(path.clone(), Container::Mp4, &format(), true).unwrap();
        // A tone from 0.5 s to 1.0 s (mic switched on, then off); interleaved like the network does.
        let tone: Vec<u8> = (0..480)
            .flat_map(|i| ((8000.0 * (i as f64 / 480.0 * 4.4 * std::f64::consts::TAU).sin()) as i16).to_le_bytes())
            .collect();
        let start = 1_000_000;
        let mut next_audio = start + 500_000;
        for (i, (key, data)) in frames.iter().enumerate() {
            let pts = start + i as u64 * 33_333;
            recorder.push(&VideoFrame { keyframe: *key, pts_us: pts, data });
            while next_audio <= pts && next_audio < start + 1_000_000 {
                recorder.push_audio(&AudioFrame { pts_us: next_audio, samples: &tone });
                next_audio += 10_000;
            }
        }
        recorder.stop();
        assert_eq!(wait_saved(&recorder), vec![path.clone()]);
        assert_eq!(frames_in(&path, "qtdemux"), 60);
        let (seconds, loud) = audio_in(&path);
        assert!((seconds - 2.0).abs() < 0.1, "sound track is {seconds:.2} s long");
        assert!((loud - 0.5).abs() < 0.1, "{loud:.2} s of tone");
        if let Ok(keep) = std::env::var("PW_KEEP_RECORDING") {
            let _ = std::fs::copy(&path, format!("{keep}-audio.mp4"));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_recording_without_a_keyframe_leaves_no_file() {
        let frames = encoded_frames(10);
        let (path, saved) = record("nokey", Container::Mp4, &frames, 1);
        assert!(saved.is_empty());
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_format_change_finishes_the_file_with_a_notice() {
        let frames = encoded_frames(12);
        let dir = std::env::temp_dir().join(format!("pw-rec-fmt-{}", std::process::id()));
        let path = new_capture_path(&dir, "mp4");
        let recorder = Recorder::default();
        recorder.start(path.clone(), Container::Mp4, &format(), false).unwrap();
        for (i, (key, data)) in frames.iter().enumerate() {
            recorder.push(&VideoFrame { keyframe: *key, pts_us: i as u64 * 33_333, data });
        }
        recorder.format_changed(&format()); // same format: keeps recording
        assert!(recorder.status().is_some());
        recorder.format_changed(&StreamFormat { rotation: 0, ..format() });
        assert!(recorder.status().is_none());
        assert!(recorder.take_notice().unwrap().contains("format changed"));
        assert_eq!(wait_saved(&recorder), vec![path]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn capture_names_are_unique_and_found_newest_first() {
        let dir = std::env::temp_dir().join(format!("pw-cap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = new_capture_path(&dir, "jpg");
        std::fs::write(&a, b"x").unwrap();
        let b = new_capture_path(&dir, "jpg");
        assert_ne!(a, b);
        std::fs::write(&b, b"x").unwrap();
        std::fs::write(dir.join("holiday.jpg"), b"x").unwrap(); // not ours
        let settings = Settings { captures_dir: Some(dir.to_string_lossy().into()), ..Settings::default() };
        let found = recent_captures(&settings, 6);
        assert_eq!(found.len(), 2);
        assert!(found.iter().all(|f| f.kind == CaptureKind::Photo));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
