//! GStreamer side: a persistent output (the virtual webcam) and a per-stream decoder.
//!
//! The two are separate pipelines joined by appsink -> appsrc. That way the virtual
//! camera stays open at a fixed size while phones connect, disconnect and change
//! codec or resolution; apps like Zoom never see the device disappear.

use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use log::{debug, info, warn};

use crate::effects::{self, EffectsControl};
use crate::protocol::{StreamFormat, VideoFrame};

/// After this long without a decoded frame, the output shows the idle screen.
const IDLE_AFTER: Duration = Duration::from_millis(500);

#[derive(Debug, Clone)]
pub enum Sink {
    /// Write to a v4l2loopback device, e.g. /dev/video10.
    V4l2(String),
    /// Show a window (for testing without v4l2loopback).
    Window,
    /// The Windows 11 virtual camera (see `wincam`).
    #[cfg(windows)]
    WinCamera,
    /// Discard frames (for headless testing).
    Null,
}

/// Width of the small RGBA copy of the output that the desktop window shows.
const PREVIEW_WIDTH: u32 = 640;
/// The window never needs more than this many preview frames per second.
const PREVIEW_FPS: u32 = 20;
/// Width of the raw (before effects) copy kept for the effects gallery thumbnails.
const THUMB_WIDTH: usize = 160;
/// How often the raw copy is refreshed (at most 5 per second).
const THUMB_EVERY: Duration = Duration::from_millis(200);

/// One preview frame: RGBA, `PREVIEW_WIDTH` wide.
pub struct PreviewFrame {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
    /// Increases with every frame, so a viewer can tell whether anything changed.
    pub seq: u64,
}

/// A small YUY2 copy of a phone frame before effects, for the effects gallery.
pub struct ThumbSource {
    pub width: usize,
    pub height: usize,
    pub yuy2: Vec<u8>,
    /// Increases with every copy, so a viewer can tell whether anything changed.
    pub seq: u64,
}

/// One full-size YUY2 frame of the virtual camera picture, effects included.
pub struct Snapshot {
    pub width: usize,
    pub height: usize,
    pub yuy2: Vec<u8>,
}

/// Latest frame of the virtual camera, for the desktop window.
#[derive(Default)]
pub struct Preview {
    latest: Mutex<Option<Arc<PreviewFrame>>>,
    raw: Mutex<Option<Arc<ThumbSource>>>,
    /// Set by the window to ask for a snapshot; the next phone frame fills `snapshot`.
    snapshot_wanted: AtomicBool,
    snapshot: Mutex<Option<Snapshot>>,
}

impl Preview {
    pub fn latest(&self) -> Option<Arc<PreviewFrame>> {
        self.latest.lock().unwrap().clone()
    }

    fn store(&self, width: usize, height: usize, rgba: Vec<u8>) {
        let mut slot = self.latest.lock().unwrap();
        let seq = slot.as_ref().map_or(0, |f| f.seq + 1);
        *slot = Some(Arc::new(PreviewFrame { width, height, rgba, seq }));
    }

    /// Latest phone frame before effects (none until a phone has streamed).
    pub fn raw_thumb(&self) -> Option<Arc<ThumbSource>> {
        self.raw.lock().unwrap().clone()
    }

    fn store_raw(&self, width: usize, height: usize, yuy2: Vec<u8>) {
        let mut slot = self.raw.lock().unwrap();
        let seq = slot.as_ref().map_or(0, |f| f.seq + 1);
        *slot = Some(Arc::new(ThumbSource { width, height, yuy2, seq }));
    }

    /// Asks for a full-size copy of the next phone frame (see `take_snapshot`).
    pub fn request_snapshot(&self) {
        self.snapshot.lock().unwrap().take();
        self.snapshot_wanted.store(true, Ordering::Relaxed);
    }

    /// The requested snapshot, once a phone frame has arrived.
    pub fn take_snapshot(&self) -> Option<Snapshot> {
        self.snapshot.lock().unwrap().take()
    }
}

/// The pipeline that feeds the virtual camera (or a test window). Separate from the preview
/// pipeline so the camera can be stopped and started without touching the phone connection.
struct CameraOut {
    pipeline: gst::Pipeline,
    src: gst_app::AppSrc,
    /// The v4l2loopback device, until it has been told to stay a camera (see `hold_camera`).
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    unheld: Mutex<Option<String>>,
}

impl CameraOut {
    /// `None` for `Sink::Null` (nothing to feed).
    fn new(sink: &Sink, width: u32, height: u32, fps: u32) -> Result<Option<Self>> {
        let sink_desc = match sink {
            Sink::V4l2(device) => {
                #[cfg(target_os = "linux")]
                crate::loopback::prepare(device);
                format!("v4l2sink device={device} sync=false")
            }
            Sink::Window => "videoconvert ! autovideosink sync=false".to_string(),
            #[cfg(windows)]
            Sink::WinCamera => {
                // The camera takes NV12 up to 1080p; bigger settings are scaled down to fit.
                let (w, h) = if width > crate::wincam::MAX_WIDTH || height > crate::wincam::MAX_HEIGHT {
                    (crate::wincam::MAX_WIDTH, crate::wincam::MAX_HEIGHT)
                } else {
                    (width, height)
                };
                format!(
                    "videoscale ! videoconvert ! video/x-raw,format=NV12,width={w},height={h} \
                     ! appsink name=wincam sync=false max-buffers=1 drop=true"
                )
            }
            Sink::Null => return Ok(None),
        };
        let pipeline = launch(&format!(
            "appsrc name=src is-live=true format=time do-timestamp=true \
             ! queue max-size-buffers=2 leaky=downstream ! {sink_desc}"
        ))?;
        let src = app_src(&pipeline)?;
        src.set_caps(Some(&raw_caps(width, height, Some(fps))?));
        #[cfg(windows)]
        if let Some(sink) = pipeline.by_name("wincam").and_then(|e| e.downcast::<gst_app::AppSink>().ok()) {
            let writer = Mutex::new(crate::wincam::Writer::default());
            sink.set_callbacks(
                gst_app::AppSinkCallbacks::builder()
                    .new_sample(move |sink| {
                        let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                        let (Some(buffer), Some(caps)) = (sample.buffer(), sample.caps()) else {
                            return Ok(gst::FlowSuccess::Ok);
                        };
                        let Some(s) = caps.structure(0) else { return Ok(gst::FlowSuccess::Ok) };
                        let (Ok(w), Ok(h)) = (s.get::<i32>("width"), s.get::<i32>("height")) else {
                            return Ok(gst::FlowSuccess::Ok);
                        };
                        if let Ok(map) = buffer.map_readable() {
                            writer.lock().unwrap().write(w as u32, h as u32, map.as_slice());
                        }
                        Ok(gst::FlowSuccess::Ok)
                    })
                    .build(),
            );
        }
        pipeline.set_state(gst::State::Playing).context("starting the virtual camera")?;
        let unheld = match sink {
            Sink::V4l2(device) if cfg!(target_os = "linux") => Some(device.clone()),
            _ => None,
        };
        Ok(Some(CameraOut { pipeline, src, unheld: Mutex::new(unheld) }))
    }
}

impl Drop for CameraOut {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

pub struct Output {
    /// Effects, idle screen and the window preview; always running.
    pipeline: gst::Pipeline,
    src: gst_app::AppSrc,
    /// The virtual camera, while it's on.
    camera: Mutex<Option<CameraOut>>,
    pub width: u32,
    pub height: u32,
    fps: u32,
    last_frame: Mutex<Instant>,
    preview: Option<Arc<Preview>>,
    effects: Arc<EffectsControl>,
    last_thumb: Mutex<Instant>,
    /// A frame too short for its size has been reported (logged once).
    short_logged: AtomicBool,
    /// Background blur / virtual background, applied before the colour effects.
    #[cfg(feature = "background")]
    background: Mutex<crate::background::Processor>,
}

impl Output {
    /// `preview`, when given, receives a small copy of every output frame (idle screen included)
    /// and a raw copy of phone frames before effects. `effects` is applied to phone frames only.
    pub fn new(
        sink: &Sink,
        width: u32,
        height: u32,
        fps: u32,
        preview: Option<Arc<Preview>>,
        effects: Arc<EffectsControl>,
    ) -> Result<Arc<Self>> {
        let preview_height = (PREVIEW_WIDTH * height / width.max(1)).max(2) & !1;
        let preview_branch = if preview.is_some() {
            format!(
                "t. ! queue max-size-buffers=1 leaky=downstream ! videorate drop-only=true max-rate={PREVIEW_FPS} \
                 ! videoscale ! videoconvert \
                 ! video/x-raw,format=RGBA,width={PREVIEW_WIDTH},height={preview_height} \
                 ! appsink name=preview sync=false max-buffers=1 drop=true"
            )
        } else {
            String::new()
        };
        let pipeline = launch(&format!(
            "appsrc name=src is-live=true format=time do-timestamp=true ! tee name=t \
             t. ! queue max-size-buffers=2 leaky=downstream ! fakesink sync=false \
             {preview_branch}"
        ))?;
        if let Some(preview) = preview.clone() {
            let appsink = pipeline
                .by_name("preview")
                .and_then(|e| e.downcast::<gst_app::AppSink>().ok())
                .ok_or_else(|| anyhow!("output pipeline has no preview appsink"))?;
            appsink.set_callbacks(
                gst_app::AppSinkCallbacks::builder()
                    .new_sample(move |sink| {
                        let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                        let (Some(buffer), Some(caps)) = (sample.buffer(), sample.caps()) else {
                            return Ok(gst::FlowSuccess::Ok);
                        };
                        let Some(s) = caps.structure(0) else { return Ok(gst::FlowSuccess::Ok) };
                        let (Ok(w), Ok(h)) = (s.get::<i32>("width"), s.get::<i32>("height")) else {
                            return Ok(gst::FlowSuccess::Ok);
                        };
                        if let Ok(map) = buffer.map_readable() {
                            preview.store(w as usize, h as usize, map.as_slice().to_vec());
                        }
                        Ok(gst::FlowSuccess::Ok)
                    })
                    .build(),
            );
        }
        let src = app_src(&pipeline)?;
        src.set_caps(Some(&raw_caps(width, height, Some(fps))?));
        pipeline.set_state(gst::State::Playing).context("starting output pipeline")?;

        let camera = CameraOut::new(sink, width, height, fps)?;
        let output = Arc::new(Output {
            pipeline,
            src,
            camera: Mutex::new(camera),
            width,
            height,
            fps,
            last_frame: Mutex::new(Instant::now() - IDLE_AFTER),
            preview,
            effects,
            last_thumb: Mutex::new(Instant::now() - THUMB_EVERY),
            short_logged: AtomicBool::new(false),
            #[cfg(feature = "background")]
            background: Mutex::new(crate::background::Processor::new(width as usize, height as usize)),
        });
        output.clone().spawn_idle_loop();
        Ok(output)
    }

    /// Pushes one decoded frame (already YUY2 at the output size).
    pub fn push(&self, mut buffer: gst::Buffer) {
        {
            // The output pipeline stamps its own timestamps (do-timestamp).
            let b = buffer.make_mut();
            b.set_pts(gst::ClockTime::NONE);
            b.set_dts(gst::ClockTime::NONE);
            b.set_duration(gst::ClockTime::NONE);
        }
        let (w, h) = (self.width as usize, self.height as usize);
        if let Some(preview) = &self.preview {
            let mut last = self.last_thumb.lock().unwrap();
            if last.elapsed() >= THUMB_EVERY {
                *last = Instant::now();
                let th = (THUMB_WIDTH * h / w.max(1)).clamp(2, THUMB_WIDTH * 4) & !1;
                if let Ok(map) = buffer.map_readable() {
                    let thumb = effects::downscale_yuy2(map.as_slice(), w, h, THUMB_WIDTH, th);
                    preview.store_raw(THUMB_WIDTH, th, thumb);
                }
            }
        }
        let c = self.effects.compiled();
        #[cfg(feature = "background")]
        let background = !c.background().is_off();
        #[cfg(not(feature = "background"))]
        let background = false;
        if !c.identity() || background {
            if let Ok(mut map) = buffer.make_mut().map_writable() {
                if map.len() >= w * h * 2 {
                    #[cfg(feature = "background")]
                    if background {
                        // A panic in an earlier frame poisons the lock; keep going rather than
                        // panicking on every frame after it.
                        let mut bg = self.background.lock().unwrap_or_else(|e| e.into_inner());
                        bg.process(map.as_mut_slice(), c.background());
                    }
                    if !c.identity() {
                        effects::apply_yuy2(&c, map.as_mut_slice());
                    }
                } else if !self.short_logged.swap(true, Ordering::Relaxed) {
                    debug!("frame is {} bytes, expected {}; skipping effects", map.len(), w * h * 2);
                }
            }
        }
        if let Some(preview) = &self.preview {
            if preview.snapshot_wanted.swap(false, Ordering::Relaxed) {
                if let Ok(map) = buffer.map_readable() {
                    *preview.snapshot.lock().unwrap() =
                        Some(Snapshot { width: w, height: h, yuy2: map.as_slice().to_vec() });
                }
            }
        }
        *self.last_frame.lock().unwrap() = Instant::now();
        self.feed(buffer);
    }

    /// Sends a finished frame to the virtual camera (if on) and the preview.
    fn feed(&self, buffer: gst::Buffer) {
        if let Some(camera) = self.camera.lock().unwrap().as_ref() {
            let _ = camera.src.push_buffer(buffer.clone());
        }
        let _ = self.src.push_buffer(buffer);
    }

    /// Turns the virtual camera on (feeding `sink`) or off (`Sink::Null`), without
    /// interrupting the preview or the phone.
    pub fn set_sink(&self, sink: &Sink) -> Result<()> {
        let mut camera = self.camera.lock().unwrap();
        camera.take(); // release the device before opening it again
        *camera = CameraOut::new(sink, self.width, self.height, self.fps)?;
        Ok(())
    }

    /// Keeps the camera alive with a "waiting" screen whenever no phone is streaming.
    /// Holds only a weak reference, so the thread ends (and the camera is released) once
    /// the output is dropped.
    fn spawn_idle_loop(self: Arc<Self>) {
        let idle = text_frame(self.width, self.height, "Pocket Webcam\nOpen the app on your phone")
            .unwrap_or_else(|e| {
                warn!("could not render idle screen ({e}); using black");
                black_frame(self.width, self.height)
            });
        #[cfg(target_os = "linux")]
        let off = text_frame(self.width, self.height, "Pocket Webcam\nCamera is off")
            .unwrap_or_else(|_| black_frame(self.width, self.height));
        let bus = self.pipeline.bus().expect("pipeline has a bus");
        let output = Arc::downgrade(&self);
        drop(self);
        thread::spawn(move || {
            while let Some(out) = output.upgrade() {
                if out.last_frame.lock().unwrap().elapsed() >= IDLE_AFTER {
                    out.feed(idle.copy());
                }
                let camera = out.camera.lock().unwrap();
                let camera_bus = camera.as_ref().and_then(|c| c.pipeline.bus());
                #[cfg(target_os = "linux")]
                let unheld = camera.as_ref().and_then(|c| c.unheld.lock().unwrap().clone());
                drop(camera);
                #[cfg(target_os = "linux")]
                if let Some(device) = unheld {
                    // Once the writer has set the format, so the kept format is ours.
                    if crate::loopback::current_size(&device) == Some((out.width, out.height)) {
                        match hold_camera(&device, out.width, out.height, out.fps, &off) {
                            Ok(()) => debug!("{device} stays a camera while the app is off"),
                            Err(e) => warn!("could not keep {device} listed as a camera: {e:#}"),
                        }
                        if let Some(c) = out.camera.lock().unwrap().as_ref() {
                            c.unheld.lock().unwrap().take();
                        }
                    }
                }
                drop(out);
                for bus in std::iter::once(&bus).chain(camera_bus.as_ref()) {
                    if let Some(err) = bus_error(bus) {
                        warn!("output error: {err}");
                    }
                }
                thread::sleep(Duration::from_millis(200));
            }
        });
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

pub struct Decoder {
    pipeline: gst::Pipeline,
    src: gst_app::AppSrc,
    bus: gst::Bus,
    first_pts: Option<u64>,
    /// This GStreamer can say how many frames are waiting (1.20 and later).
    has_level: bool,
    queue_limit: u64,
}

impl Decoder {
    pub fn new(format: &StreamFormat, output: Arc<Output>, hw: bool) -> Result<Self> {
        let (parser, sw_decoder, hw_decoder, caps) = match format.codec.as_str() {
            // GPU decoders: VA-API on Linux, Direct3D 11 on Windows.
            "h264" => ("h264parse", "avdec_h264", if cfg!(windows) { "d3d11h264dec" } else { "vah264dec" }, "video/x-h264"),
            "hevc" => ("h265parse", "avdec_h265", if cfg!(windows) { "d3d11h265dec" } else { "vah265dec" }, "video/x-h265"),
            other => bail!("unsupported codec: {other}"),
        };
        let decoder = if hw && gst::ElementFactory::find(hw_decoder).is_some() {
            hw_decoder
        } else {
            if hw {
                warn!("{hw_decoder} not available, falling back to {sw_decoder}");
            }
            sw_decoder
        };
        // Rotate clockwise first, then (optionally) mirror the upright result.
        let flip = match (format.rotation, format.mirror) {
            (90, false) => "clockwise",
            (180, false) => "rotate-180",
            (270, false) => "counterclockwise",
            (90, true) => "upper-left-diagonal",
            (180, true) => "vertical-flip",
            (270, true) => "upper-right-diagonal",
            (_, true) => "horizontal-flip",
            _ => "none",
        };

        let pipeline = launch(&format!(
            "appsrc name=src is-live=true format=time \
             ! {parser} ! {decoder} ! videoconvert ! videoflip method={flip} \
             ! videoscale add-borders=true ! videoconvert \
             ! appsink name=sink sync=false max-buffers=2 drop=true"
        ))?;

        let src = app_src(&pipeline)?;
        src.set_caps(Some(
            &gst::Caps::from_str(&format!("{caps},stream-format=byte-stream,alignment=au"))
                .map_err(|_| anyhow!("bad input caps"))?,
        ));

        let sink = pipeline
            .by_name("sink")
            .and_then(|e| e.downcast::<gst_app::AppSink>().ok())
            .ok_or_else(|| anyhow!("decoder pipeline has no appsink"))?;
        sink.set_caps(Some(&raw_caps(output.width, output.height, None)?));
        sink.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                    if let Some(buffer) = sample.buffer_owned() {
                        output.push(buffer);
                    }
                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        );

        let bus = pipeline.bus().ok_or_else(|| anyhow!("pipeline has no bus"))?;
        pipeline.set_state(gst::State::Playing).context("starting decoder")?;
        info!(
            "decoding {} {}x{} @ {} fps with {decoder}",
            format.codec, format.width, format.height, format.fps
        );
        let has_level = src.find_property("current-level-buffers").is_some();
        // About a quarter of a second of video.
        let queue_limit = u64::from(format.fps / 4).max(8);
        Ok(Decoder { pipeline, src, bus, first_pts: None, has_level, queue_limit })
    }

    /// Frames from the phone that are still waiting to be decoded. Normally 0 or 1; it
    /// only grows when this computer decodes slower than the phone sends.
    pub fn queued(&self) -> u64 {
        if self.has_level {
            self.src.property::<u64>("current-level-buffers")
        } else {
            0
        }
    }

    /// More waiting frames than this means the picture is falling behind.
    pub fn queue_limit(&self) -> u64 {
        self.queue_limit
    }

    pub fn push(&mut self, frame: &VideoFrame) -> Result<()> {
        let first = *self.first_pts.get_or_insert(frame.pts_us);
        let mut buffer = gst::Buffer::from_slice(frame.data.to_vec());
        {
            let b = buffer.get_mut().unwrap();
            b.set_pts(gst::ClockTime::from_useconds(frame.pts_us.saturating_sub(first)));
            if !frame.keyframe {
                b.set_flags(gst::BufferFlags::DELTA_UNIT);
            }
        }
        self.src.push_buffer(buffer).map_err(|e| anyhow!("decoder rejected frame: {e:?}"))?;
        match bus_error(&self.bus) {
            Some(err) => Err(anyhow!(err)),
            None => Ok(()),
        }
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

fn launch(description: &str) -> Result<gst::Pipeline> {
    gst::parse::launch(description)
        .with_context(|| format!("building pipeline: {description}"))?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow!("not a pipeline"))
}

fn app_src(pipeline: &gst::Pipeline) -> Result<gst_app::AppSrc> {
    pipeline
        .by_name("src")
        .and_then(|e| e.downcast::<gst_app::AppSrc>().ok())
        .ok_or_else(|| anyhow!("pipeline has no appsrc"))
}

fn raw_caps(width: u32, height: u32, fps: Option<u32>) -> Result<gst::Caps> {
    let mut s = format!(
        "video/x-raw,format=YUY2,width={width},height={height},pixel-aspect-ratio=1/1"
    );
    if let Some(fps) = fps {
        s.push_str(&format!(",framerate={fps}/1"));
    }
    gst::Caps::from_str(&s).map_err(|_| anyhow!("bad caps: {s}"))
}

fn bus_error(bus: &gst::Bus) -> Option<String> {
    let msg = bus.pop_filtered(&[gst::MessageType::Error])?;
    match msg.view() {
        gst::MessageView::Error(err) => Some(match err.debug() {
            Some(debug) => format!("{} ({debug})", err.error()),
            None => err.error().to_string(),
        }),
        _ => None,
    }
}

/// Keeps a v4l2loopback device listed as a camera after our writer closes, showing `off`
/// to readers while nothing writes (see `loopback`).
#[cfg(target_os = "linux")]
fn hold_camera(device: &str, width: u32, height: u32, fps: u32, off: &gst::Buffer) -> Result<()> {
    use crate::loopback::{set_control, KEEP_FORMAT, TIMEOUT, TIMEOUT_IMAGE_IO, TIMEOUT_MS};

    set_control(device, KEEP_FORMAT, 1).context("keep_format")?;
    // The next opener of the device writes the timeout image instead of frames.
    set_control(device, TIMEOUT_IMAGE_IO, 1).context("timeout_image_io")?;
    let pipeline = launch(&format!("appsrc name=src format=time ! v4l2sink device={device} sync=false"))?;
    let src = app_src(&pipeline)?;
    src.set_caps(Some(&raw_caps(width, height, Some(fps))?));
    pipeline.set_state(gst::State::Playing).context("starting the timeout image writer")?;
    let _ = src.push_buffer(off.copy());
    let _ = src.end_of_stream();
    let bus = pipeline.bus().expect("pipeline has a bus");
    let done = bus.timed_pop_filtered(
        gst::ClockTime::from_seconds(3),
        &[gst::MessageType::Eos, gst::MessageType::Error],
    );
    let _ = pipeline.set_state(gst::State::Null);
    match done.as_ref().map(|m| m.view()) {
        Some(gst::MessageView::Eos(_)) => {}
        Some(gst::MessageView::Error(err)) => bail!("writing the timeout image: {}", err.error()),
        _ => bail!("writing the timeout image timed out"),
    }
    set_control(device, TIMEOUT, TIMEOUT_MS).context("timeout")?;
    Ok(())
}

/// Renders a two-line text screen once, at the output size.
fn text_frame(width: u32, height: u32, message: &str) -> Result<gst::Buffer> {
    // Render the text at the output size; overlaying at videotestsrc's default 320x240
    // and scaling up afterwards makes it huge and blurry.
    let pipeline = launch(&format!(
        "videotestsrc num-buffers=1 pattern=black ! video/x-raw,width={width},height={height} \
         ! textoverlay name=text auto-resize=false valignment=center halignment=center line-alignment=center \
         ! videoconvert ! appsink name=sink"
    ))?;
    let font_px = (height / 16).max(12);
    let text = pipeline.by_name("text").ok_or_else(|| anyhow!("no textoverlay"))?;
    text.set_property("text", message);
    text.set_property("font-desc", format!("Sans {font_px}px"));
    let sink = pipeline
        .by_name("sink")
        .and_then(|e| e.downcast::<gst_app::AppSink>().ok())
        .ok_or_else(|| anyhow!("no appsink"))?;
    sink.set_caps(Some(&raw_caps(width, height, None)?));

    pipeline.set_state(gst::State::Playing)?;
    let sample = sink.try_pull_sample(gst::ClockTime::from_seconds(3));
    let _ = pipeline.set_state(gst::State::Null);
    sample
        .and_then(|s| s.buffer_owned())
        .ok_or_else(|| anyhow!("idle screen render timed out"))
}

fn black_frame(width: u32, height: u32) -> gst::Buffer {
    // YUY2 black: Y=16, U/V=128.
    let pixels = (width * height / 2) as usize;
    gst::Buffer::from_slice([16u8, 128, 16, 128].repeat(pixels))
}
