//! Runs background blur / virtual background on the video thread without ever waiting for
//! the model: a worker thread segments small copies of recent frames (latest wins) and
//! publishes masks, and each frame is composited with the newest mask available. On a slow
//! CPU masks arrive less often, but the video keeps its full frame rate.

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use log::{debug, warn};

use super::builtin;
use super::composite::{self, Blur, Mask, TEMPORAL_KEEP};
use super::model::{Segmenter, INPUT_H, INPUT_W};
use crate::effects::{self, Background, BUILTIN_PREFIX};

/// The worker thread exits after this long without a frame to segment.
#[cfg(not(test))]
const IDLE_EXIT: Duration = Duration::from_secs(5);
#[cfg(test)]
const IDLE_EXIT: Duration = Duration::from_millis(500);
/// A mask older than this (no frames, or the effect was off) is dropped instead of reused.
const STALE_AFTER: Duration = Duration::from_secs(1);
/// Timing is logged (debug) every this many processed frames.
const LOG_EVERY: u32 = 300;
/// A background image that failed to load is tried again after this long.
#[cfg(not(test))]
const RETRY_AFTER: Duration = Duration::from_secs(5);
#[cfg(test)]
const RETRY_AFTER: Duration = Duration::from_millis(300);
/// Largest background image accepted, per side: room for 48 MP phone photos (8064x6048),
/// DSLR shots and panoramas.
pub const MAX_IMAGE_SIDE: u32 = 12000;
/// Most memory the image decoder may allocate: 400 MiB, enough for an 8064x6048 photo even
/// as 16-bit RGBA (~390 MB). Larger images within `MAX_IMAGE_SIDE` fail to load (blur instead).
const MAX_IMAGE_ALLOC: u64 = 400 * 1024 * 1024;

/// The worker's input slot and lifecycle, behind one lock.
#[derive(Default)]
struct Slot {
    /// Newest model input (INPUT_W x INPUT_H RGB) not yet taken by the worker.
    job: Option<Vec<u8>>,
    /// A worker thread is running (or starting).
    alive: bool,
    /// The processor was dropped; the worker should exit.
    stop: bool,
}

#[derive(Default)]
struct Shared {
    slot: Mutex<Slot>,
    wake: Condvar,
    /// Newest mask with its sequence number.
    mask: Mutex<Option<(u64, Arc<Mask>)>>,
    next_seq: AtomicU64,
    /// The model could not be loaded; never try again.
    failed: AtomicBool,
}

/// Background image loads, shared with the loader thread. At most one loader thread runs;
/// a newer request replaces a pending one, and a load that is no longer wanted is abandoned.
#[derive(Default)]
struct Loads {
    /// Newest requested (id, path) not yet picked up by the loader thread.
    pending: Option<(u64, String)>,
    /// The load the processor is waiting for (0: none); results for other ids are dropped.
    wanted: u64,
    /// Finished load for `wanted`.
    done: Option<(u64, Result<Vec<u8>, String>)>,
    /// A loader thread is running (or starting).
    running: bool,
    /// Loader threads spawned and loads started, for tests.
    #[cfg(test)]
    spawned: u32,
    #[cfg(test)]
    started: u32,
}

enum ImageState {
    /// Waiting for load `id`.
    Loading(u64),
    Ready(Vec<u8>),
    /// Failed at this time; retried after `RETRY_AFTER`.
    Failed(Instant),
}

/// The current background image, keyed by (path, width, height).
struct CachedImage {
    path: String,
    width: usize,
    height: usize,
    state: ImageState,
}

#[derive(Default)]
struct Timing {
    frames: u32,
    total: Duration,
    max: Duration,
    masks: u32,
    since: Option<Instant>,
}

/// Blurs or replaces the background of `width` x `height` YUY2 frames.
pub struct Processor {
    width: usize,
    height: usize,
    shared: Arc<Shared>,
    blur: Blur,
    /// Full-size alpha for `mask_seq`.
    alpha: Vec<u8>,
    mask_seq: Option<u64>,
    image: Option<CachedImage>,
    loads: Arc<Mutex<Loads>>,
    /// Id of the newest image load requested.
    load_id: u64,
    /// Image paths whose load failure has been reported.
    warned: HashSet<String>,
    last_call: Option<Instant>,
    timing: Timing,
}

impl Processor {
    pub fn new(width: usize, height: usize) -> Self {
        Processor {
            width,
            height,
            shared: Arc::new(Shared::default()),
            blur: Blur::new(),
            alpha: Vec::new(),
            mask_seq: None,
            image: None,
            loads: Arc::default(),
            load_id: 0,
            warned: HashSet::new(),
            last_call: None,
            timing: Timing::default(),
        }
    }

    /// Applies `bg` to `frame` (YUY2 at this processor's size) in place. Never waits for the
    /// model: until a mask exists (or if the model failed to load) the frame is left as is.
    pub fn process(&mut self, frame: &mut [u8], bg: &Background) {
        let (w, h) = (self.width, self.height);
        if !matches!(bg, Background::Image { .. })
            && matches!(&self.image, Some(CachedImage { state: ImageState::Failed(_), .. }))
        {
            // Switched away from a broken image: try it afresh if it's chosen again.
            self.image = None;
        }
        if bg.is_off() || w == 0 || h == 0 || frame.len() < w * h * 2 {
            return;
        }
        let start = Instant::now();
        if self.last_call.is_some_and(|t| t.elapsed() >= STALE_AFTER) {
            // The person has probably moved since; don't flash an old mask.
            *self.shared.mask.lock().unwrap() = None;
            self.mask_seq = None;
        }
        self.last_call = Some(start);

        self.submit(frame);
        if let Background::Image { path } = bg {
            self.poll_image(path);
        }
        let latest = self.shared.mask.lock().unwrap().clone();
        if let Some((seq, mask)) = latest {
            if self.mask_seq != Some(seq) {
                composite::upscale_mask(&mask, w, h, &mut self.alpha);
                self.mask_seq = Some(seq);
                self.timing.masks += 1;
            }
            let image = match (&self.image, bg) {
                (Some(CachedImage { state: ImageState::Ready(yuy2), .. }), Background::Image { .. }) => {
                    Some(yuy2)
                }
                _ => None,
            };
            match (bg, image) {
                (_, Some(yuy2)) => composite::composite_yuy2(frame, yuy2, &self.alpha, w, h),
                (bg, None) => {
                    // Image still loading or failed: blur, so the room isn't revealed.
                    let strength = match bg {
                        Background::Blur { strength } => *strength,
                        _ => Background::DEFAULT_BLUR,
                    };
                    let blurred = self.blur.run(frame, w, h, strength);
                    composite::composite_yuy2(frame, blurred, &self.alpha, w, h);
                }
            }
        }
        self.record(start.elapsed());
    }

    /// Hands the worker a small copy of `frame` if it has nothing queued, starting the worker
    /// if needed.
    fn submit(&mut self, frame: &[u8]) {
        if self.shared.failed.load(Ordering::Relaxed) {
            return;
        }
        if self.shared.slot.lock().unwrap().job.is_some() {
            return;
        }
        let small = effects::downscale_yuy2(frame, self.width, self.height, INPUT_W, INPUT_H);
        let rgb: Vec<u8> = effects::yuy2_to_rgba(&small, INPUT_W, INPUT_H)
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect();
        let mut slot = self.shared.slot.lock().unwrap();
        slot.job = Some(rgb);
        if !slot.alive {
            slot.alive = true;
            let shared = self.shared.clone();
            if let Err(e) = thread::Builder::new()
                .name("background-model".into())
                .spawn(move || worker(shared))
            {
                warn!("could not start the background model thread: {e}");
                slot.alive = false;
                slot.job = None;
            }
        }
        drop(slot);
        self.shared.wake.notify_one();
    }

    /// Starts loading `path` if it isn't the cached image (or its load failed a while ago),
    /// and picks up a finished load.
    fn poll_image(&mut self, path: &str) {
        let (w, h) = (self.width, self.height);
        let load = match &self.image {
            Some(c) if c.path == path && c.width == w && c.height == h => {
                matches!(c.state, ImageState::Failed(at) if at.elapsed() >= RETRY_AFTER)
            }
            _ => true,
        };
        if load {
            let state = self.request_load(path);
            self.image = Some(CachedImage { path: path.to_string(), width: w, height: h, state });
        }
        let Some(image) = self.image.as_mut() else { return };
        let ImageState::Loading(id) = image.state else { return };
        let done = {
            let mut loads = self.loads.lock().unwrap();
            match loads.done.take() {
                Some((done_id, result)) if done_id == id => result,
                other => {
                    loads.done = other;
                    return;
                }
            }
        };
        image.state = match done {
            Ok(yuy2) => {
                self.warned.remove(&image.path);
                ImageState::Ready(yuy2)
            }
            Err(e) => {
                if self.warned.insert(image.path.clone()) {
                    warn!("could not load background image {:?}: {e}; blurring instead", image.path);
                }
                ImageState::Failed(Instant::now())
            }
        };
    }

    /// Queues a load of `path` (replacing any queued one), starting the loader thread if it
    /// isn't running.
    fn request_load(&mut self, path: &str) -> ImageState {
        self.load_id += 1;
        let id = self.load_id;
        let (w, h) = (self.width, self.height);
        let mut loads = self.loads.lock().unwrap();
        loads.wanted = id;
        loads.pending = Some((id, path.to_string()));
        loads.done = None;
        if loads.running {
            return ImageState::Loading(id);
        }
        loads.running = true;
        #[cfg(test)]
        {
            loads.spawned += 1;
        }
        let shared = self.loads.clone();
        match thread::Builder::new()
            .name("background-image".into())
            .spawn(move || loader(shared, w, h))
        {
            Ok(_) => ImageState::Loading(id),
            Err(e) => {
                loads.running = false;
                loads.pending = None;
                if self.warned.insert(path.to_string()) {
                    warn!("could not start loading background image {path:?}: {e}");
                }
                ImageState::Failed(Instant::now())
            }
        }
    }

    fn record(&mut self, took: Duration) {
        let t = &mut self.timing;
        let since = *t.since.get_or_insert_with(Instant::now);
        t.frames += 1;
        t.total += took;
        t.max = t.max.max(took);
        if t.frames >= LOG_EVERY {
            let secs = since.elapsed().as_secs_f64().max(1e-3);
            debug!(
                "background: {} frames, process mean {:.2} ms, max {:.2} ms, {:.1} masks/s",
                t.frames,
                t.total.as_secs_f64() * 1000.0 / t.frames as f64,
                t.max.as_secs_f64() * 1000.0,
                t.masks as f64 / secs
            );
            *t = Timing { since: Some(Instant::now()), ..Timing::default() };
        }
    }

    #[cfg(test)]
    fn has_mask(&self) -> bool {
        self.mask_seq.is_some()
    }

    #[cfg(test)]
    fn image_ready(&self) -> bool {
        matches!(&self.image, Some(CachedImage { state: ImageState::Ready(_), .. }))
    }

    #[cfg(test)]
    fn worker_alive(&self) -> bool {
        self.shared.slot.lock().unwrap().alive
    }
}

impl Drop for Processor {
    fn drop(&mut self) {
        self.shared.slot.lock().unwrap().stop = true;
        self.shared.wake.notify_all();
        let mut loads = self.loads.lock().unwrap_or_else(|e| e.into_inner());
        loads.wanted = 0;
        loads.pending = None;
    }
}

/// The model thread: loads the segmenter once, then turns queued frames into masks until it
/// has been idle for `IDLE_EXIT` or the processor is dropped.
fn worker(shared: Arc<Shared>) {
    let segmenter = match Segmenter::load() {
        Ok(s) => s,
        Err(e) => {
            warn!("could not load the background model ({e:#}); background effects are disabled");
            shared.failed.store(true, Ordering::Relaxed);
            let mut slot = shared.slot.lock().unwrap();
            slot.job = None;
            slot.alive = false;
            return;
        }
    };
    let mut run_warned = false;
    loop {
        let rgb = {
            let slot = shared.slot.lock().unwrap();
            let (mut slot, _) = shared
                .wake
                .wait_timeout_while(slot, IDLE_EXIT, |s| s.job.is_none() && !s.stop)
                .unwrap();
            match slot.job.take() {
                Some(rgb) if !slot.stop => rgb,
                _ => {
                    slot.alive = false;
                    return;
                }
            }
        };
        match segmenter.run(&rgb) {
            Ok(probs) => {
                let prev = shared.mask.lock().unwrap().clone();
                let mask = composite::mask_from_probs(
                    &probs,
                    INPUT_W,
                    INPUT_H,
                    prev.as_ref().map(|(_, m)| &**m),
                    TEMPORAL_KEEP,
                );
                let seq = shared.next_seq.fetch_add(1, Ordering::Relaxed);
                *shared.mask.lock().unwrap() = Some((seq, Arc::new(mask)));
            }
            Err(e) => {
                if !run_warned {
                    run_warned = true;
                    warn!("background model failed on a frame: {e:#}");
                }
            }
        }
        let mut slot = shared.slot.lock().unwrap();
        if slot.stop {
            slot.alive = false;
            return;
        }
    }
}

/// The image loader thread: loads queued requests, newest first, until none is left.
fn loader(loads: Arc<Mutex<Loads>>, w: usize, h: usize) {
    loop {
        let (id, path) = {
            let mut l = loads.lock().unwrap();
            match l.pending.take() {
                Some(job) => {
                    #[cfg(test)]
                    {
                        l.started += 1;
                    }
                    job
                }
                None => {
                    l.running = false;
                    return;
                }
            }
        };
        let wanted = || loads.lock().unwrap().wanted == id;
        let result = load_image(&path, w, h, &wanted);
        let mut l = loads.lock().unwrap();
        if l.wanted == id {
            l.done = Some((id, result));
        }
    }
}

/// Loads background `path` (`builtin:<id>` or an image file) as a `w*h` YUY2 frame. Gives up
/// between the expensive steps once `wanted` returns false.
fn load_image(path: &str, w: usize, h: usize, wanted: &dyn Fn() -> bool) -> Result<Vec<u8>, String> {
    const UNWANTED: &str = "no longer wanted";
    if let Some(id) = path.strip_prefix(BUILTIN_PREFIX) {
        let rgba = builtin::render_rgba(id, w, h).ok_or_else(|| format!("no built-in background {id:?}"))?;
        return Ok(composite::rgba_to_yuy2_cover(&rgba, w, h, w, h));
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_SIDE);
    limits.max_image_height = Some(MAX_IMAGE_SIDE);
    limits.max_alloc = Some(MAX_IMAGE_ALLOC);
    let mut reader = image::ImageReader::open(path)
        .and_then(|r| r.with_guessed_format())
        .map_err(|e| e.to_string())?;
    reader.limits(limits);
    let mut decoded = reader.decode().map_err(|e| e.to_string())?;
    if !wanted() {
        return Err(UNWANTED.into());
    }
    // Shrink big photos before converting, so the work and the cached copy stay small.
    if let Some((nw, nh)) = shrunk_size(decoded.width(), decoded.height(), w, h) {
        decoded = decoded.thumbnail_exact(nw, nh);
        if !wanted() {
            return Err(UNWANTED.into());
        }
    }
    let rgba = decoded.into_rgba8();
    if !wanted() {
        return Err(UNWANTED.into());
    }
    let (iw, ih) = (rgba.width() as usize, rgba.height() as usize);
    Ok(composite::rgba_to_yuy2_cover(rgba.as_raw(), iw, ih, w, h))
}

/// The size to shrink a `iw` x `ih` image to before cover-fitting it to `w` x `h`: the same
/// aspect, still covering twice the output size. `None` if it isn't larger than that.
fn shrunk_size(iw: u32, ih: u32, w: usize, h: usize) -> Option<(u32, u32)> {
    let (iw, ih) = (iw as f64, ih as f64);
    let scale = (2.0 * w as f64 / iw).max(2.0 * h as f64 / ih);
    (scale < 1.0).then(|| (((iw * scale).round() as u32).max(1), ((ih * scale).round() as u32).max(1)))
}

/// Checks that `path` is a picture [`Processor`] can use as a background, reading only its
/// header. The error is a short message for the user.
pub fn check_image(path: &Path) -> Result<(), String> {
    let dims = image::ImageReader::open(path)
        .and_then(|r| r.with_guessed_format())
        .ok()
        .and_then(|r| r.into_dimensions().ok());
    match dims {
        None => Err("Couldn't open that image. Try a PNG or JPEG.".into()),
        Some((w, h)) if w > MAX_IMAGE_SIDE || h > MAX_IMAGE_SIDE => {
            Err(format!("That image is too large (over {MAX_IMAGE_SIDE} pixels wide or tall)."))
        }
        Some(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic YUY2 frame: a bright blob in the middle of a textured background.
    fn frame(w: usize, h: usize) -> Vec<u8> {
        let mut f = vec![0u8; w * h * 2];
        for y in 0..h {
            for x in 0..w {
                let (dx, dy) = (x as i64 - w as i64 / 2, y as i64 - h as i64 / 2);
                let inside = dx * dx * 4 + dy * dy * 9 < (h * h) as i64;
                let o = (y * w + x) * 2;
                f[o] = if inside { 200 } else { (40 + (x * 7 + y * 13) % 120) as u8 };
                f[o + 1] = if x % 2 == 0 { (100 + y % 50) as u8 } else { (140 - x % 40) as u8 };
            }
        }
        f
    }

    /// Calls `process` every 20 ms (on fresh copies of `input`) until `done` or 10 s pass.
    fn process_until(p: &mut Processor, input: &[u8], bg: &Background, done: impl Fn(&Processor) -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let mut f = input.to_vec();
            p.process(&mut f, bg);
            if done(p) {
                return true;
            }
            thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn off_leaves_frame_identical() {
        let input = frame(640, 360);
        let mut p = Processor::new(640, 360);
        let mut f = input.clone();
        for _ in 0..3 {
            p.process(&mut f, &Background::Off);
        }
        assert_eq!(f, input);
        assert!(!p.worker_alive());
    }

    #[test]
    fn blur_produces_mask_and_keeps_length() {
        let input = frame(640, 360);
        let mut p = Processor::new(640, 360);
        let bg = Background::Blur { strength: 0.8 };
        assert!(process_until(&mut p, &input, &bg, Processor::has_mask), "no mask within 10 s");
        let mut f = input.clone();
        p.process(&mut f, &bg);
        assert_eq!(f.len(), input.len());
    }

    #[test]
    fn short_frame_is_ignored() {
        let mut p = Processor::new(640, 360);
        let mut f = vec![7u8; 100];
        p.process(&mut f, &Background::Blur { strength: 0.5 });
        assert_eq!(f, vec![7u8; 100]);
    }

    #[test]
    fn missing_image_does_not_panic() {
        let input = frame(640, 360);
        let mut p = Processor::new(640, 360);
        let bg = Background::Image { path: "/nonexistent/pocketwebcam/bg.png".into() };
        let failed = |p: &Processor| matches!(&p.image, Some(CachedImage { state: ImageState::Failed(_), .. }));
        assert!(process_until(&mut p, &input, &bg, failed));
        let mut f = input.clone();
        p.process(&mut f, &bg);
        assert_eq!(f.len(), input.len());
    }

    fn loads_started(p: &Processor) -> u32 {
        p.loads.lock().unwrap().started
    }

    /// Waits up to 10 s for the current image load to finish (polling without frames).
    fn wait_image(p: &mut Processor, path: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while matches!(&p.image, Some(CachedImage { state: ImageState::Loading(_), .. })) {
            assert!(Instant::now() < deadline, "image load did not finish");
            thread::sleep(Duration::from_millis(10));
            p.poll_image(path);
        }
    }

    #[test]
    fn failed_image_is_retried_after_switching_away_or_waiting() {
        let mut p = Processor::new(64, 36);
        let path = "/nonexistent/pocketwebcam/retry.png";
        p.poll_image(path);
        wait_image(&mut p, path);
        assert!(matches!(&p.image, Some(CachedImage { state: ImageState::Failed(_), .. })));
        assert_eq!(loads_started(&p), 1);
        // Within the back-off the failure is kept.
        p.poll_image(path);
        assert!(matches!(&p.image, Some(CachedImage { state: ImageState::Failed(_), .. })));
        // Switching to blur forgets it; choosing the image again loads it again.
        let mut f = vec![0u8; 64 * 36 * 2];
        p.process(&mut f, &Background::Blur { strength: 0.5 });
        assert!(p.image.is_none());
        p.poll_image(path);
        wait_image(&mut p, path);
        // After the back-off it is tried again without switching away.
        thread::sleep(RETRY_AFTER);
        p.poll_image(path);
        wait_image(&mut p, path);
        // Each failure was a fresh load; the warning is only logged once.
        let deadline = Instant::now() + Duration::from_secs(10);
        while loads_started(&p) < 3 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(loads_started(&p), 3);
        assert_eq!(p.warned.len(), 1);
    }

    #[test]
    fn image_loads_are_single_flight_and_newest_wins() {
        let mut p = Processor::new(1920, 1080);
        let ids = ["builtin:dusk", "builtin:nope", "/nonexistent/a.png"];
        for i in 0..30 {
            p.poll_image(ids[i % ids.len()]);
        }
        let last = "builtin:dusk";
        p.poll_image(last);
        wait_image(&mut p, last);
        assert!(p.image_ready(), "newest request did not win");
        let Some(CachedImage { state: ImageState::Ready(yuy2), .. }) = &p.image else { unreachable!() };
        assert_eq!(*yuy2, load_image(last, 1920, 1080, &|| true).unwrap());
        let loads = p.loads.lock().unwrap();
        assert_eq!(loads.spawned, 1, "more than one loader thread");
        assert!(loads.started <= 2, "{} loads for one burst of requests", loads.started);
    }

    #[test]
    fn builtin_image_changes_frame() {
        let input = frame(640, 360);
        let mut p = Processor::new(640, 360);
        let bg = Background::Image { path: "builtin:dusk".into() };
        let ready = |p: &Processor| p.has_mask() && p.image_ready();
        assert!(process_until(&mut p, &input, &bg, ready), "no mask or image within 10 s");
        let mut f = input.clone();
        p.process(&mut f, &bg);
        assert_eq!(f.len(), input.len());
        assert_ne!(f, input);
    }

    #[test]
    fn worker_exits_when_idle_and_restarts() {
        let input = frame(640, 360);
        let mut p = Processor::new(640, 360);
        let bg = Background::Blur { strength: 0.5 };
        assert!(process_until(&mut p, &input, &bg, Processor::has_mask));
        assert!(p.worker_alive());
        let deadline = Instant::now() + IDLE_EXIT + Duration::from_secs(5);
        while p.worker_alive() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(50));
        }
        assert!(!p.worker_alive(), "worker still running after idling");
        let mut f = input.clone();
        p.process(&mut f, &bg);
        assert!(p.worker_alive(), "worker not restarted");
    }

    #[test]
    fn dropping_processor_stops_worker() {
        let input = frame(640, 360);
        let mut p = Processor::new(640, 360);
        let bg = Background::Blur { strength: 0.5 };
        assert!(process_until(&mut p, &input, &bg, Processor::has_mask));
        let shared = p.shared.clone();
        drop(p);
        let deadline = Instant::now() + Duration::from_secs(2);
        while shared.slot.lock().unwrap().alive && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        // Well before the idle timeout would have stopped it.
        assert!(!shared.slot.lock().unwrap().alive);
    }

    /// A unique path in the temp dir for a test file.
    fn temp_png(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("pocketwebcam-test-{}-{name}.png", std::process::id()))
    }

    #[test]
    fn shrunk_size_keeps_aspect_and_covers_twice_the_output() {
        assert_eq!(shrunk_size(8064, 6048, 1280, 720), Some((2560, 1920)));
        // A panorama is shrunk by its height, so it still covers the output.
        assert_eq!(shrunk_size(12000, 3000, 1920, 1080), Some((8640, 2160)));
        assert_eq!(shrunk_size(2560, 1440, 1280, 720), None);
        assert_eq!(shrunk_size(640, 480, 1280, 720), None);
    }

    #[test]
    fn large_image_loads_downscaled() {
        let path = temp_png("large");
        let img = image::RgbImage::from_fn(5000, 3000, |x, y| image::Rgb([(x % 256) as u8, (y % 256) as u8, 90]));
        img.save(&path).unwrap();
        let (w, h) = (640, 360);
        let mut p = Processor::new(w, h);
        let path_str = path.to_str().unwrap().to_string();
        p.poll_image(&path_str);
        wait_image(&mut p, &path_str);
        let loaded = match &p.image {
            Some(CachedImage { state: ImageState::Ready(yuy2), .. }) => Ok(yuy2.len()),
            _ => Err(()),
        };
        let checked = check_image(&path);
        std::fs::remove_file(&path).ok();
        assert_eq!(loaded, Ok(w * h * 2));
        assert_eq!(checked, Ok(()));
    }

    #[test]
    fn check_image_rejects_unreadable_and_too_large() {
        assert!(check_image(Path::new("/nonexistent/pocketwebcam/bg.png")).is_err());
        let not_image = temp_png("not-image");
        std::fs::write(&not_image, b"not an image").unwrap();
        let wide = temp_png("wide");
        image::GrayImage::new(MAX_IMAGE_SIDE + 1, 1).save(&wide).unwrap();
        let ok = temp_png("ok");
        image::GrayImage::new(MAX_IMAGE_SIDE, 1).save(&ok).unwrap();
        let results = [check_image(&not_image), check_image(&wide), check_image(&ok)];
        for path in [&not_image, &wide, &ok] {
            std::fs::remove_file(path).ok();
        }
        assert_eq!(results[0], Err("Couldn't open that image. Try a PNG or JPEG.".into()));
        assert!(results[1].as_ref().is_err_and(|e| e.contains("too large")), "{:?}", results[1]);
        assert_eq!(results[2], Ok(()));
    }

    /// Per-frame cost on the video thread at 1080p; run with
    /// `cargo test --release --lib background::processor -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn bench_process_1080p() {
        let (w, h) = (1920, 1080);
        let input = frame(w, h);
        for (name, bg) in [
            ("blur", Background::Blur { strength: 0.6 }),
            ("builtin image", Background::Image { path: "builtin:dusk".into() }),
        ] {
            let mut p = Processor::new(w, h);
            let ready = |p: &Processor| p.has_mask() && (matches!(bg, Background::Blur { .. }) || p.image_ready());
            assert!(process_until(&mut p, &input, &bg, ready));
            let mut times = Vec::new();
            let mut last_seq = p.mask_seq;
            let mut masks = 0;
            let started = Instant::now();
            for _ in 0..150 {
                let mut f = input.clone();
                let t = Instant::now();
                p.process(&mut f, &bg);
                times.push(t.elapsed());
                if p.mask_seq != last_seq {
                    last_seq = p.mask_seq;
                    masks += 1;
                }
                // Roughly 30 fps, so the worker runs alongside as it would live.
                thread::sleep(Duration::from_millis(33).saturating_sub(t.elapsed()));
            }
            let mean = times.iter().sum::<Duration>() / times.len() as u32;
            let max = times.iter().max().unwrap();
            println!(
                "process 1080p {name} over {} frames: mean {:.2} ms, max {:.2} ms ({} new masks in {:.1} s)",
                times.len(),
                mean.as_secs_f64() * 1000.0,
                max.as_secs_f64() * 1000.0,
                masks,
                started.elapsed().as_secs_f64()
            );
        }
    }
}
