//! The strip under the preview: Capture (record, snapshot, recent captures) and Scenes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align2, Color32, ColorImage, CornerRadius, FontId, Rect, RichText, Sense, Stroke, StrokeKind,
    TextureHandle, TextureOptions, Vec2, WidgetInfo, WidgetType,
};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use pocketwebcam_receiver::capture::{self, CaptureFile, CaptureKind};
use pocketwebcam_receiver::effects::{yuy2_to_rgba, Effects};
use pocketwebcam_receiver::engine::Snapshot;
use pocketwebcam_receiver::protocol::{Camera, Capabilities, ConfigUpdate, State};
use pocketwebcam_receiver::scenes::Scene;

use crate::icons::{self, Icon};
use crate::theme::{self, ACCENT, ACCENT_TINT, BORDER, BORDER_STRONG, CONTROL, CONTROL_HOVER, MUTED, RED, SUBTLE, TEXT};
use crate::App;

/// How many recent captures the strip shows.
const RECENT: usize = 6;
/// Thumbnails are cropped to 16:10.
const THUMB_ASPECT: f32 = 16.0 / 10.0;
const SCENE_H: f32 = 56.0;
/// Below this inner width the two halves stack.
const SIDE_BY_SIDE_MIN: f32 = 1000.0;
const PAD: f32 = 14.0;
/// After applying a scene, wait this long for the phone to settle before tracking changes.
const SCENE_SETTLE: Duration = Duration::from_millis(1500);
/// How often the captures folder is re-read (files can come and go outside the app).
const RESCAN_EVERY: Duration = Duration::from_secs(5);
/// Give up on a snapshot if no phone frame arrives in this time.
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(2);

enum Thumb {
    Loading,
    Ready(TextureHandle),
    Failed,
}

/// The settings a scene compares against, to notice changes after it was applied.
#[derive(Clone, PartialEq)]
struct Fingerprint {
    camera: String,
    size: (u32, u32, u32),
    codec: String,
    bitrate_100k: u64,
    zoom_10: i64,
    toggles: (bool, bool, bool),
    effects: Effects,
}

impl Fingerprint {
    fn of(state: &State, effects: &Effects) -> Self {
        Fingerprint {
            camera: state.camera.clone(),
            size: (state.width, state.height, state.fps),
            codec: state.codec.clone(),
            bitrate_100k: state.bitrate / 100_000,
            zoom_10: (state.zoom * 10.0).round() as i64,
            toggles: (state.mirror, state.torch, state.stabilization),
            effects: effects.clone(),
        }
    }
}

pub struct StripState {
    captures: Vec<CaptureFile>,
    scanned: Option<Instant>,
    thumbs: HashMap<PathBuf, Thumb>,
    thumb_tx: Sender<(PathBuf, Option<ColorImage>)>,
    thumb_rx: Receiver<(PathBuf, Option<ColorImage>)>,
    /// When a snapshot was asked for and no frame has arrived yet.
    snapshot_wanted: Option<Instant>,
    snapshot_saving: Option<JoinHandle<Result<PathBuf, String>>>,
    /// A message under the capture buttons, and whether it's an error.
    notice: Option<(String, bool, Instant)>,
    /// The scene applied last, and what the settings looked like once the phone settled.
    active_scene: Option<usize>,
    applied_at: Option<Instant>,
    baseline: Option<Fingerprint>,
    renaming: Option<(usize, String)>,
}

impl StripState {
    pub fn new() -> Self {
        let (thumb_tx, thumb_rx) = channel();
        StripState {
            captures: Vec::new(),
            scanned: None,
            thumbs: HashMap::new(),
            thumb_tx,
            thumb_rx,
            snapshot_wanted: None,
            snapshot_saving: None,
            notice: None,
            active_scene: None,
            applied_at: None,
            baseline: None,
            renaming: None,
        }
    }

    fn say(&mut self, text: impl Into<String>, error: bool) {
        self.notice = Some((text.into(), error, Instant::now()));
    }
}

/// Height the strip needs at `width`; the preview gives it at least this much.
pub fn min_height(width: f32) -> f32 {
    let inner = width - 2.0 * PAD;
    let label = 18.0 + 8.0;
    let notice = 22.0;
    if inner >= SIDE_BY_SIDE_MIN {
        let cap_w = (inner - 24.0) / 2.0;
        let cap = label + thumb_size(cap_w).y.max(theme::CONTROL_H) + notice;
        2.0 * PAD + cap.max(label + SCENE_H)
    } else {
        let cap = label + thumb_size(inner).y.max(theme::CONTROL_H) + notice;
        2.0 * PAD + cap + 14.0 + label + SCENE_H
    }
}

/// Size of one capture thumbnail when the capture half is `width` wide.
fn thumb_size(width: f32) -> Vec2 {
    // Record (~120) + snapshot (40) + gaps leave the rest for the thumbnails.
    let area = width - 120.0 - 40.0 - 3.0 * 8.0;
    let w = ((area - (RECENT as f32 - 1.0) * 8.0) / RECENT as f32).clamp(44.0, 112.0).floor();
    Vec2::new(w, (w / THUMB_ASPECT).round())
}

/// Background work, run every frame: finished recordings and snapshots, folder rescans,
/// loaded thumbnails, and the scene "modified" baseline.
pub fn update(app: &mut App, ctx: &egui::Context, snap: &Snapshot) {
    let recorder = app.engine.recorder().clone();
    let saved = recorder.take_saved();
    if let Some(notice) = recorder.take_notice() {
        app.strip.say(notice, false);
    }
    if !saved.is_empty() {
        app.strip.scanned = None;
    }

    // Snapshot: wait for the frame, then encode and save it off the UI thread.
    if let Some(since) = app.strip.snapshot_wanted {
        if let Some(frame) = app.engine.preview().take_snapshot() {
            app.strip.snapshot_wanted = None;
            let dir = capture::pictures_dir(&snap.settings);
            app.strip.snapshot_saving = Some(thread::spawn(move || save_snapshot(&dir, frame)));
        } else if since.elapsed() > SNAPSHOT_TIMEOUT {
            app.strip.snapshot_wanted = None;
            app.strip.say("No picture to capture yet.", true);
        }
    }
    if app.strip.snapshot_saving.as_ref().is_some_and(|h| h.is_finished()) {
        let result = app.strip.snapshot_saving.take().and_then(|h| h.join().ok());
        match result {
            Some(Ok(path)) => {
                let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                app.strip.say(format!("Snapshot saved: {name}"), false);
                app.strip.scanned = None;
            }
            Some(Err(e)) => app.strip.say(format!("Couldn't save the snapshot: {e}"), true),
            None => app.strip.say("Couldn't save the snapshot.", true),
        }
    }

    if app.strip.scanned.is_none_or(|t| t.elapsed() > RESCAN_EVERY) {
        app.strip.scanned = Some(Instant::now());
        app.strip.captures = capture::recent_captures(&snap.settings, RECENT);
        let keep: Vec<PathBuf> = app.strip.captures.iter().map(|c| c.path.clone()).collect();
        app.strip.thumbs.retain(|p, _| keep.contains(p));
        // A recording still being written has no index yet; try its thumbnail again later.
        app.strip.thumbs.retain(|_, t| !matches!(t, Thumb::Failed));
        for c in &app.strip.captures {
            if !app.strip.thumbs.contains_key(&c.path) {
                app.strip.thumbs.insert(c.path.clone(), Thumb::Loading);
                let (tx, path, kind) = (app.strip.thumb_tx.clone(), c.path.clone(), c.kind);
                thread::spawn(move || {
                    let image = load_thumbnail(&path, kind);
                    let _ = tx.send((path, image));
                });
            }
        }
    }
    while let Ok((path, image)) = app.strip.thumb_rx.try_recv() {
        let thumb = match image {
            Some(image) => Thumb::Ready(ctx.load_texture(path.to_string_lossy(), image, TextureOptions::LINEAR)),
            None => Thumb::Failed,
        };
        app.strip.thumbs.insert(path, thumb);
    }

    if app.strip.notice.as_ref().is_some_and(|n| n.2.elapsed() > Duration::from_secs(8)) {
        app.strip.notice = None;
    }

    if let (Some(at), None) = (app.strip.applied_at, &app.strip.baseline) {
        if at.elapsed() > SCENE_SETTLE {
            if let Some(state) = snap.phone.as_ref().and_then(|p| p.info.state.as_ref()) {
                app.strip.baseline = Some(Fingerprint::of(state, &snap.settings.effects));
            }
        }
    }
}

/// Turns a full-size YUY2 frame into a JPEG in `dir`.
fn save_snapshot(dir: &Path, frame: pocketwebcam_receiver::video::Snapshot) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let rgba = yuy2_to_rgba(&frame.yuy2, frame.width, frame.height);
    let rgb: Vec<u8> = rgba.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
    let path = capture::new_capture_path(dir, "jpg");
    let file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(std::io::BufWriter::new(file), 92);
    encoder
        .encode(&rgb, frame.width as u32, frame.height as u32, image::ExtendedColorType::Rgb8)
        .map_err(|e| e.to_string())?;
    Ok(path)
}

/// A small RGBA picture of a capture: the photo itself, or a video's first frame.
fn load_thumbnail(path: &Path, kind: CaptureKind) -> Option<ColorImage> {
    match kind {
        CaptureKind::Photo => {
            let img = image::open(path).ok()?.thumbnail(320, 320).to_rgba8();
            let size = [img.width() as usize, img.height() as usize];
            Some(ColorImage::from_rgba_unmultiplied(size, &img))
        }
        CaptureKind::Video => {
            // `videoflip video-direction=auto` turns portrait recordings upright.
            let pipeline = gst::parse::launch(
                "filesrc name=src ! decodebin ! videoconvert ! videoflip video-direction=auto ! videoscale \
                 ! video/x-raw,format=RGBA,width=320,pixel-aspect-ratio=1/1 ! appsink name=out sync=false max-buffers=1",
            )
            .ok()?
            .downcast::<gst::Pipeline>()
            .ok()?;
            pipeline.by_name("src")?.set_property("location", path.to_string_lossy().to_string());
            let sink = pipeline.by_name("out")?.downcast::<gst_app::AppSink>().ok()?;
            pipeline.set_state(gst::State::Paused).ok()?;
            let sample = sink.try_pull_preroll(gst::ClockTime::from_seconds(5));
            let _ = pipeline.set_state(gst::State::Null);
            let sample = sample?;
            let s = sample.caps()?.structure(0)?;
            let (w, h) = (s.get::<i32>("width").ok()? as usize, s.get::<i32>("height").ok()? as usize);
            let map = sample.buffer()?.map_readable().ok()?;
            (map.len() >= w * h * 4).then(|| ColorImage::from_rgba_unmultiplied([w, h], &map[..w * h * 4]))
        }
    }
}

/// Runs a helper program without handing it this app's open files. GStreamer opens the
/// virtual camera without close-on-exec, so a viewer started by `xdg-open` would otherwise
/// keep /dev/video10 open for as long as it runs.
#[cfg(unix)]
fn command(program: &str) -> std::process::Command {
    use std::os::unix::process::CommandExt;
    let mut cmd = std::process::Command::new(program);
    // SAFETY: close_range is async-signal-safe and only touches the child's descriptors.
    unsafe {
        cmd.pre_exec(|| {
            libc::syscall(libc::SYS_close_range, 3u32, u32::MAX, 0u32);
            Ok(())
        });
    }
    cmd
}

#[cfg(windows)]
pub(crate) fn open_file(path: &Path) {
    // Explorer opens a file with its default app, and a folder in a window.
    let _ = std::process::Command::new("explorer").arg(path).spawn();
}

#[cfg(windows)]
fn show_in_folder(path: &Path) {
    let mut select = std::ffi::OsString::from("/select,");
    select.push(path);
    let _ = std::process::Command::new("explorer").arg(select).spawn();
}

/// Opens a file with its usual app, or a folder in the file manager.
#[cfg(unix)]
pub(crate) fn open_file(path: &Path) {
    let _ = command("xdg-open").arg(path).spawn();
}

/// Opens the file manager with `path` selected, or at least its folder.
#[cfg(unix)]
fn show_in_folder(path: &Path) {
    let uri = format!("file://{}", percent_encode(&path.to_string_lossy()));
    let shown = command("dbus-send")
        .args([
            "--session",
            "--dest=org.freedesktop.FileManager1",
            "--type=method_call",
            "/org/freedesktop/FileManager1",
            "org.freedesktop.FileManager1.ShowItems",
            &format!("array:string:{uri}"),
            "string:",
        ])
        .status()
        .is_ok_and(|s| s.success());
    if !shown {
        if let Some(dir) = path.parent() {
            open_file(dir);
        }
    }
}

#[cfg(unix)]
fn percent_encode(path: &str) -> String {
    path.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Moves a file to the Recycle Bin. False if Windows couldn't.
#[cfg(windows)]
fn recycle(path: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::{
        SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FO_DELETE, SHFILEOPSTRUCTW,
    };
    // The list of files ends with an empty name, so the path needs two NULs.
    let from: Vec<u16> = path.as_os_str().encode_wide().chain([0, 0]).collect();
    // SAFETY: every pointer is either null or `from`, which outlives the call.
    let mut op: SHFILEOPSTRUCTW = unsafe { std::mem::zeroed() };
    op.wFunc = FO_DELETE;
    op.pFrom = from.as_ptr();
    op.fFlags = (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT) as u16;
    // SAFETY: as above.
    unsafe { SHFileOperationW(&mut op) == 0 && op.fAnyOperationsAborted == 0 }
}

/// Moves a capture to the Trash / Recycle Bin (recoverable), falling back to deleting it.
fn delete_capture(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    let trashed = command("gio").arg("trash").arg(path).status().is_ok_and(|s| s.success());
    #[cfg(windows)]
    let trashed = recycle(path);
    if trashed {
        return Ok(());
    }
    std::fs::remove_file(path).map_err(|e| e.to_string())
}

/// The strip card. Fills `rect` (it is given whatever height the preview left over).
pub fn show(app: &mut App, ui: &mut egui::Ui, snap: &Snapshot, rect: Rect) {
    let mut card_ui = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    theme::card_frame().inner_margin(egui::Margin::same(PAD as i8)).show(&mut card_ui, |ui| {
        ui.set_min_size(rect.size() - Vec2::splat(2.0 * PAD));
        let inner = ui.available_width();
        if inner >= SIDE_BY_SIDE_MIN {
            ui.horizontal_top(|ui| {
                let half = (inner - 24.0) / 2.0;
                let column = egui::Layout::top_down(egui::Align::Min);
                let height = ui.available_height();
                ui.allocate_ui_with_layout(Vec2::new(half, height), column, |ui| capture_section(app, ui, snap, half));
                ui.add_space(11.0);
                let (sep, _) = ui.allocate_exact_size(Vec2::new(1.0, height), Sense::hover());
                ui.painter().rect_filled(sep, 0.0, BORDER);
                ui.add_space(11.0);
                ui.allocate_ui_with_layout(Vec2::new(half, height), column, |ui| scenes_section(app, ui, snap, half));
            });
        } else {
            capture_section(app, ui, snap, inner);
            ui.add_space(6.0);
            theme::divider(ui);
            ui.add_space(6.0);
            scenes_section(app, ui, snap, inner);
        }
    });
}

fn capture_section(app: &mut App, ui: &mut egui::Ui, snap: &Snapshot, width: f32) {
    ui.set_width(width);
    let live = snap.phone.as_ref().is_some_and(|p| p.frames > 0);
    // A text-height row (a plain `horizontal` would be padded to the 40 px control height).
    ui.allocate_ui_with_layout(Vec2::new(width, 18.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        theme::section_label(ui, "Capture");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let r = ui.link(RichText::new("See all").size(12.0).color(ACCENT));
            if r.clicked() {
                let dir = capture::videos_dir(&snap.settings);
                let _ = std::fs::create_dir_all(&dir);
                open_file(&dir);
            }
        });
    });

    ui.horizontal(|ui| {
        let recording = app.engine.recording().map(|r| r.started.elapsed());
        let r = theme::record_button(ui, recording, live || recording.is_some());
        let r = if live || recording.is_some() { r } else { r.on_hover_text("Connect a phone to record.") };
        if r.clicked() {
            if recording.is_some() {
                app.engine.stop_recording();
            } else if let Err(e) = app.engine.start_recording() {
                app.strip.say(format!("Couldn't start recording: {e:#}"), true);
            }
        }

        let busy = app.strip.snapshot_wanted.is_some() || app.strip.snapshot_saving.is_some();
        let r = ui.add_enabled_ui(live && !busy, |ui| theme::icon_button(ui, Icon::Shutter, "Take snapshot")).inner;
        if r.clicked() {
            app.engine.preview().request_snapshot();
            app.strip.snapshot_wanted = Some(Instant::now());
        }
        ui.add_space(4.0);

        let size = thumb_size(width);
        if app.strip.captures.is_empty() {
            ui.label(RichText::new("Recordings and snapshots appear here.").size(12.0).color(SUBTLE));
        }
        let captures = app.strip.captures.clone();
        for c in &captures {
            capture_tile(app, ui, c, size);
        }
    });

    match &app.strip.notice {
        Some((text, error, _)) => {
            ui.label(RichText::new(text).size(12.0).color(if *error { RED } else { MUTED }));
        }
        None => {
            ui.add_space(18.0);
        }
    }
}

fn capture_tile(app: &mut App, ui: &mut egui::Ui, c: &CaptureFile, size: Vec2) {
    let name = c.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, format!("Open {name}")));
    let response = response.on_hover_text(format!("{name}\nClick to open. Right-click (or Shift+F10) for more."));
    if response.hovered() {
        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
    }
    let radius = CornerRadius::same(theme::RADIUS_SEGMENT);
    let painter = ui.painter();
    match app.strip.thumbs.get(&c.path) {
        Some(Thumb::Ready(tex)) => {
            // Crop the middle to 16:10.
            let [w, h] = tex.size();
            let aspect = w as f32 / h.max(1) as f32;
            let uv = if aspect > THUMB_ASPECT {
                let u = THUMB_ASPECT / aspect;
                Rect::from_min_max(egui::pos2(0.5 - u / 2.0, 0.0), egui::pos2(0.5 + u / 2.0, 1.0))
            } else {
                let v = aspect / THUMB_ASPECT;
                Rect::from_min_max(egui::pos2(0.0, 0.5 - v / 2.0), egui::pos2(1.0, 0.5 + v / 2.0))
            };
            egui::Image::new(tex).uv(uv).corner_radius(radius).paint_at(ui, rect);
        }
        _ => {
            painter.rect_filled(rect, radius, CONTROL);
        }
    }
    let stroke = if response.hovered() { Stroke::new(1.0, BORDER_STRONG) } else { Stroke::new(1.0, BORDER) };
    painter.rect_stroke(rect, radius, stroke, StrokeKind::Inside);
    if c.kind == CaptureKind::Video {
        let badge = Rect::from_min_size(rect.left_bottom() + Vec2::new(5.0, -19.0), Vec2::splat(14.0));
        painter.circle_filled(badge.center(), 8.0, theme::SCRIM);
        icons::paint(painter, badge.shrink(3.0), Icon::Play, TEXT);
    }

    if response.clicked() {
        open_file(&c.path);
    }
    let path = c.path.clone();
    theme::context_menu(&response, |ui| {
        if ui.button("Open").clicked() {
            open_file(&path);
        }
        if ui.button("Show in folder").clicked() {
            show_in_folder(&path);
        }
        if ui.button(RichText::new("Delete").color(RED)).on_hover_text("Moves the file to the Trash.").clicked() {
            match delete_capture(&path) {
                Ok(()) => app.strip.say(format!("Moved {name} to the Trash."), false),
                Err(e) => app.strip.say(format!("Couldn't delete {name}: {e}"), true),
            }
            app.strip.scanned = None;
        }
    });
}

fn phone_state(snap: &Snapshot) -> Option<(&State, &Capabilities, &Camera)> {
    let phone = snap.phone.as_ref()?;
    let state = phone.info.state.as_ref()?;
    let caps = &phone.info.caps;
    let camera = caps.cameras.iter().find(|c| c.id == state.camera)?;
    Some((state, caps, camera))
}

/// A scene with the current lens, camera settings and effects.
fn scene_from_current(name: String, state: &State, camera: &Camera, effects: &Effects) -> Scene {
    Scene {
        name,
        position: camera.position.clone(),
        lens: camera.lens.clone(),
        width: state.width,
        height: state.height,
        fps: state.fps,
        codec: state.codec.clone(),
        bitrate: state.bitrate,
        zoom: state.zoom,
        mirror: state.mirror,
        torch: state.torch,
        stabilization: state.stabilization,
        effects: effects.clone(),
    }
}

/// Sends a scene to the phone and applies its effects.
fn apply_scene(app: &mut App, scene: &Scene, caps: &Capabilities, state: &State) {
    let camera = caps
        .cameras
        .iter()
        .find(|c| c.position == scene.position && c.lens == scene.lens)
        .or_else(|| caps.cameras.iter().find(|c| c.id == state.camera));
    let Some(camera) = camera else { return };
    let has = |f: &str| camera.features.iter().any(|x| x == f);
    // Use the scene's size and rate if this lens has them, else the closest it has.
    let format = camera
        .formats
        .iter()
        .min_by_key(|f| (f.width.abs_diff(scene.width) + f.height.abs_diff(scene.height), !f.fps.contains(&scene.fps)));
    let (width, height, fps) = match format {
        Some(f) => (f.width, f.height, f.fps.iter().copied().min_by_key(|n| n.abs_diff(scene.fps)).unwrap_or(scene.fps)),
        None => (scene.width, scene.height, scene.fps),
    };
    let codec = if caps.codecs.is_empty() || caps.codecs.contains(&scene.codec) { scene.codec.clone() } else { state.codec.clone() };
    let bitrate = caps.bitrate.as_ref().map_or(scene.bitrate, |r| scene.bitrate.clamp(r.min, r.max));
    let zoom = camera.zoom.as_ref().map(|r| scene.zoom.clamp(r.min, r.max));
    app.engine.send_config(ConfigUpdate {
        camera: Some(camera.id.clone()),
        width: Some(width),
        height: Some(height),
        fps: Some(fps),
        codec: Some(codec),
        bitrate: Some(bitrate),
        zoom,
        torch: has("torch").then_some(scene.torch),
        stabilization: has("stabilization").then_some(scene.stabilization),
        mirror: Some(scene.mirror),
        audio: None, // scenes leave the mic as it is
    });
    app.fx.reset_drag();
    app.engine.set_effects(scene.effects.clone(), true);
}

fn scene_subtitle(scene: &Scene) -> String {
    let size = match (scene.width, scene.height) {
        (3840, 2160) => "4K".to_string(),
        (_, h) => format!("{h}p"),
    };
    let look = scene.effects.look.label();
    format!("{size} · {} · {look}", scene.fps)
}

fn scenes_section(app: &mut App, ui: &mut egui::Ui, snap: &Snapshot, width: f32) {
    ui.set_width(width);
    theme::section_label(ui, "Scenes");
    let phone = phone_state(snap);
    let current = phone.map(|(state, _, _)| Fingerprint::of(state, &snap.settings.effects));
    let gap = 8.0;
    let card_w = ((width - 3.0 * gap) / 4.0).floor();
    let scenes = snap.settings.scenes.clone();

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        for (i, slot) in scenes.iter().enumerate() {
            if app.strip.renaming.as_ref().is_some_and(|r| r.0 == i) {
                rename_field(app, ui, i, card_w);
                continue;
            }
            match slot {
                Some(scene) => {
                    let selected = app.strip.active_scene == Some(i);
                    let modified = selected
                        && app.strip.baseline.as_ref().is_some_and(|b| current.as_ref().is_some_and(|c| c != b));
                    let r = scene_card(ui, scene, selected, modified, phone.is_some(), card_w);
                    if r.clicked() {
                        if let Some((state, caps, _)) = phone {
                            apply_scene(app, scene, caps, state);
                            app.strip.active_scene = Some(i);
                            app.strip.applied_at = Some(Instant::now());
                            app.strip.baseline = None;
                        }
                    }
                    theme::context_menu(&r, |ui| {
                        let can_update = phone.is_some();
                        if ui.add_enabled(can_update, egui::Button::new("Update with current settings")).clicked() {
                            if let Some((state, _, camera)) = phone {
                                let updated = scene_from_current(scene.name.clone(), state, camera, &snap.settings.effects);
                                app.engine.update_settings(|s| s.scenes[i] = Some(updated));
                                app.strip.active_scene = Some(i);
                                app.strip.applied_at = None;
                                app.strip.baseline = current.clone();
                            }
                        }
                        if ui.button("Rename").clicked() {
                            app.strip.renaming = Some((i, scene.name.clone()));
                        }
                        if ui.button(RichText::new("Delete").color(RED)).clicked() {
                            app.engine.update_settings(|s| s.scenes[i] = None);
                            if app.strip.active_scene == Some(i) {
                                app.strip.active_scene = None;
                            }
                        }
                    });
                }
                None => {
                    let r = empty_slot(ui, phone.is_some(), card_w);
                    if r.clicked() {
                        if let Some((state, _, camera)) = phone {
                            let name = format!("Scene {}", i + 1);
                            let scene = scene_from_current(name.clone(), state, camera, &snap.settings.effects);
                            app.engine.update_settings(|s| s.scenes[i] = Some(scene));
                            app.strip.active_scene = Some(i);
                            app.strip.applied_at = None;
                            app.strip.baseline = current.clone();
                            app.strip.renaming = Some((i, name));
                        }
                    }
                }
            }
        }
    });
}

fn scene_card(ui: &mut egui::Ui, scene: &Scene, selected: bool, modified: bool, enabled: bool, width: f32) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width, SCENE_H), if enabled { Sense::click() } else { Sense::hover() });
    let state = if modified { "modified" } else if selected { "applied" } else { "" };
    response.widget_info(|| WidgetInfo::selected(WidgetType::RadioButton, enabled, selected, format!("Scene {} {state}", scene.name)));
    let response = if enabled {
        response.on_hover_text("Click to apply. Right-click (or Shift+F10) to update, rename or delete.")
    } else {
        response.on_hover_text("Connect a phone to use scenes.")
    };
    if response.hovered() && enabled {
        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
    }
    let (fill, stroke) = if selected {
        (ACCENT_TINT, ACCENT)
    } else if response.hovered() && enabled {
        (CONTROL_HOVER, BORDER_STRONG)
    } else {
        (CONTROL, BORDER)
    };
    let fade = if enabled { 1.0 } else { 0.5 };
    let painter = ui.painter();
    painter.rect(rect, CornerRadius::same(theme::RADIUS_CONTROL), fill, Stroke::new(1.0, stroke), StrokeKind::Inside);
    let text_w = rect.width() - 24.0 - if modified { 12.0 } else { 0.0 };
    let title = painter.layout_no_wrap(scene.name.clone(), theme::semibold(14.0), TEXT.gamma_multiply(fade));
    let title = if title.size().x > text_w {
        painter.layout(scene.name.clone(), theme::semibold(14.0), TEXT.gamma_multiply(fade), text_w)
    } else {
        title
    };
    painter.galley(rect.left_top() + Vec2::new(12.0, 10.0), title, TEXT);
    let subtitle = if modified { "Modified".to_string() } else { scene_subtitle(scene) };
    let color = if selected { ACCENT } else { MUTED };
    painter.text(
        rect.left_top() + Vec2::new(12.0, 31.0),
        Align2::LEFT_TOP,
        subtitle,
        FontId::proportional(12.0),
        color.gamma_multiply(fade),
    );
    if modified {
        painter.circle_filled(egui::pos2(rect.right() - 12.0, rect.top() + 12.0), 3.5, ACCENT);
    }
    response
}

fn empty_slot(ui: &mut egui::Ui, enabled: bool, width: f32) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width, SCENE_H), if enabled { Sense::click() } else { Sense::hover() });
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, "Save current settings as a scene"));
    let response = if enabled {
        response.on_hover_text("Saves the lens, video, picture and effects settings as a scene.")
    } else {
        response.on_hover_text("Connect a phone to save a scene.")
    };
    if response.hovered() && enabled {
        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
    }
    let fill = if response.hovered() && enabled { CONTROL_HOVER } else { Color32::TRANSPARENT };
    let painter = ui.painter();
    painter.rect(rect, CornerRadius::same(theme::RADIUS_CONTROL), fill, Stroke::new(1.0, BORDER_STRONG), StrokeKind::Inside);
    let color = if enabled { MUTED } else { SUBTLE.gamma_multiply(0.6) };
    let galley = painter.layout_no_wrap("Save current".into(), theme::semibold(13.0), color);
    let total = 14.0 + 6.0 + galley.size().x;
    let left = rect.center().x - total / 2.0;
    icons::paint(painter, Rect::from_center_size(egui::pos2(left + 7.0, rect.center().y), Vec2::splat(14.0)), Icon::Plus, color);
    painter.galley(egui::pos2(left + 20.0, rect.center().y - galley.size().y / 2.0), galley, color);
    response
}

/// The name field shown in place of a scene card while renaming. Enter or clicking away
/// saves; Escape cancels.
fn rename_field(app: &mut App, ui: &mut egui::Ui, i: usize, width: f32) {
    let Some((_, text)) = app.strip.renaming.as_mut() else { return };
    let r = ui.add_sized(
        Vec2::new(width, SCENE_H),
        egui::TextEdit::singleline(text).hint_text("Scene name").vertical_align(egui::Align::Center),
    );
    if !r.has_focus() && !r.lost_focus() {
        r.request_focus();
    }
    let (enter, escape) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
    if escape {
        app.strip.renaming = None;
    } else if r.lost_focus() || enter {
        let name = text.trim().to_string();
        app.strip.renaming = None;
        if !name.is_empty() {
            app.engine.update_settings(|s| {
                if let Some(scene) = s.scenes[i].as_mut() {
                    scene.name = name;
                }
            });
        }
    }
}
