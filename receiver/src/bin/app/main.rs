//! Pocket Webcam desktop app: pick a phone, see the live picture, change lens, resolution,
//! frame rate and more. The video goes to the virtual webcam for Zoom, Meet, OBS and friends.

// A window app on Windows: no console window next to it.
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(feature = "background")]
mod background;
mod controls;
mod effects;
mod guide;
mod icons;
mod licenses;
mod pairing;
mod stats;
mod strip;
mod theme;
#[cfg(not(windows))]
mod tray;
#[cfg(windows)]
#[path = "tray_windows.rs"]
mod tray;
mod window;

use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align2, Color32, ColorImage, CornerRadius, FontId, Margin, Rect, RichText, Sense, Stroke, StrokeKind,
    TextureHandle, TextureOptions, Vec2,
};
use pocketwebcam_receiver::engine::{Engine, OutputStatus, Snapshot};
use pocketwebcam_receiver::settings::Settings;

use icons::Icon;
use theme::{ACCENT, BG, BORDER, BORDER_STRONG, ELEVATED, GREEN, MUTED, ORANGE, RAISED, RED, TEXT};

const LOGO_PNG: &[u8] = include_bytes!("../../../../brand/profile-400.png");

/// Only one copy may run (two would fight over the tray and the virtual camera).
/// Returns the lock to hold for the life of the process, or `None` if another copy has it.
fn single_instance() -> Option<std::fs::File> {
    let dir = pocketwebcam_receiver::paths::runtime_dir();
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(dir.join("pocketwebcam-app.lock"))
        .ok()?;
    match file.try_lock() {
        Ok(()) => Some(file),
        Err(std::fs::TryLockError::WouldBlock) => None,
        // Can't tell (odd filesystem): better to run than to refuse.
        Err(_) => Some(file),
    }
}

fn main() -> eframe::Result {
    pocketwebcam_receiver::paths::use_bundled_gstreamer();
    let Some(_instance_lock) = single_instance() else {
        window::raise_existing();
        eprintln!("Pocket Webcam is already running.");
        return Ok(());
    };
    // After the lock: only the copy that runs writes the log file.
    pocketwebcam_receiver::logging::init(true);
    if let Err(e) = gstreamer::init() {
        log::error!("could not start GStreamer: {e}");
        std::process::exit(1);
    }

    let icon = image::load_from_memory(LOGO_PNG).ok().map(|img| {
        let rgba = img.to_rgba8();
        egui::IconData { width: rgba.width(), height: rgba.height(), rgba: rgba.into_raw() }
    });
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Pocket Webcam")
        .with_app_id("com.pocketwebcam.app")
        .with_inner_size([1280.0, 800.0])
        .with_min_inner_size([960.0, 600.0]);
    if let Some(icon) = icon {
        viewport = viewport.with_icon(Arc::new(icon));
    }

    eframe::run_native(
        "Pocket Webcam",
        eframe::NativeOptions { viewport, ..Default::default() },
        Box::new(|cc| {
            theme::apply(&cc.egui_ctx);
            Ok(Box::new(App::new(cc)))
        }),
    )
}

pub struct App {
    pub engine: Engine,
    logo: Option<TextureHandle>,
    video: Option<TextureHandle>,
    video_seq: Option<u64>,
    pub ui: controls::UiState,
    pub fx: effects::FxState,
    #[cfg(feature = "background")]
    pub bg: background::BgState,
    pub strip: strip::StripState,
    pub stats: stats::StatsState,
    pub window: window::WindowControl,
    pub tray: Option<tray::TrayGuard>,
    /// Started with `--minimized` (at login): hide to the tray once the window exists.
    hide_at_start: bool,
    frames: u32,
    started: Instant,
    /// Show the focus ring: on after Tab, off after a mouse click.
    focus_visible: bool,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let ctx = &cc.egui_ctx;
        let settings = Settings::load();
        let logo = image::load_from_memory(LOGO_PNG).ok().map(|img| {
            let rgba = img.to_rgba8();
            let size = [rgba.width() as usize, rgba.height() as usize];
            ctx.load_texture("logo", ColorImage::from_rgba_unmultiplied(size, &rgba), TextureOptions::LINEAR)
        });
        let engine = Engine::start(settings.clone());
        let window = window::WindowControl::new(ctx.clone());
        #[cfg(windows)]
        window.set_native(cc);
        let tray = tray::spawn(engine.clone(), window.clone());
        let minimized = std::env::args().any(|a| a == "--minimized");
        App {
            hide_at_start: minimized && tray.is_some(),
            window,
            tray,
            frames: 0,
            engine,
            logo,
            video: None,
            video_seq: None,
            ui: controls::UiState::new(settings),
            fx: effects::FxState::new(),
            #[cfg(feature = "background")]
            bg: background::BgState::new(),
            strip: strip::StripState::new(),
            stats: stats::StatsState::default(),
            started: Instant::now(),
            focus_visible: false,
        }
    }

    /// Copies the newest preview frame into a texture.
    fn update_video(&mut self, ctx: &egui::Context) {
        let Some(frame) = self.engine.preview().latest() else { return };
        if self.video_seq == Some(frame.seq) {
            return;
        }
        self.video_seq = Some(frame.seq);
        let image = ColorImage::from_rgba_unmultiplied([frame.width, frame.height], &frame.rgba);
        match &mut self.video {
            Some(tex) => tex.set(image, TextureOptions::LINEAR),
            None => self.video = Some(ctx.load_texture("video", image, TextureOptions::LINEAR)),
        }
    }

    fn header(&mut self, ui: &mut egui::Ui, snap: &Snapshot) {
        let full = ui.max_rect();
        ui.horizontal_centered(|ui| {
            if let Some(logo) = &self.logo {
                ui.add(egui::Image::new(logo).fit_to_exact_size(Vec2::splat(28.0)).corner_radius(CornerRadius::same(7)));
            }
            ui.label(theme::strong("Pocket Webcam").size(16.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::icon_button(ui, Icon::Gear, "Settings").clicked() {
                    self.ui.draft = snap.settings.clone();
                    self.ui.autostart = pocketwebcam_receiver::autostart::is_enabled();
                    self.ui.settings_open = !self.ui.settings_open;
                }
                if theme::secondary_button(ui, "Guide").clicked() {
                    guide::open(self);
                }
            });
        });
        self.phone_pill(ui, snap, full);
    }

    /// The phone picker in the middle of the header: status at a glance, menu on click.
    fn phone_pill(&mut self, ui: &mut egui::Ui, snap: &Snapshot, header: Rect) {
        let live = snap.phone.as_ref().is_some_and(|p| p.frames > 0);
        let name = snap.phone.as_ref().and_then(|p| p.info.hello.as_ref()).map(|h| h.name.clone());
        let (dot, title, detail) = match (&snap.phone, &snap.connecting) {
            (Some(p), _) if live => {
                let mut parts = vec!["Connected".to_string()];
                if let Some(rtt) = p.info.rtt_ms {
                    parts.push(format!("{rtt:.0} ms"));
                }
                if let Some(b) = p.info.state.as_ref().and_then(|s| s.battery) {
                    parts.push(format!("{:.0}%", b * 100.0));
                }
                (GREEN, name.unwrap_or_else(|| "Phone".into()), parts.join(" · "))
            }
            (Some(_), _) => (ORANGE, name.unwrap_or_else(|| "Phone".into()), "Starting video…".into()),
            (None, Some(n)) => (ORANGE, n.clone(), "Connecting…".into()),
            (None, None) if snap.phones.is_empty() => (MUTED, "No phone".into(), "Looking on your Wi-Fi…".into()),
            (None, None) if !snap.phones.iter().any(|p| snap.is_paired(p)) => {
                (MUTED, "Pair a phone".into(), format!("{} found", snap.phones.len()))
            }
            (None, None) => (MUTED, "Choose a phone".into(), format!("{} found", snap.phones.len())),
        };

        let title_font = theme::semibold(14.0);
        let detail_font = FontId::proportional(14.0);
        let title_w = ui.fonts_mut(|f| f.layout_no_wrap(title.clone(), title_font.clone(), TEXT).size().x);
        let detail_w = ui.fonts_mut(|f| f.layout_no_wrap(detail.clone(), detail_font.clone(), MUTED).size().x);
        let width = 16.0 + 8.0 + 10.0 + title_w + 10.0 + detail_w + 10.0 + 16.0 + 14.0;
        let rect = Rect::from_center_size(header.center(), Vec2::new(width, theme::CONTROL_H));
        let response = ui.interact(rect, ui.id().with("phone_pill"), Sense::click());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Phone: {title}, {detail}")));
        if response.hovered() {
            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
        }
        let painter = ui.painter();
        let fill = if response.hovered() { theme::CONTROL_HOVER } else { ELEVATED };
        painter.rect(rect, CornerRadius::same(255), fill, Stroke::new(1.0, BORDER_STRONG), StrokeKind::Inside);
        let mut x = rect.left() + 16.0;
        painter.circle_filled(egui::pos2(x + 4.0, rect.center().y), 4.0, dot);
        x += 18.0;
        painter.text(egui::pos2(x, rect.center().y), Align2::LEFT_CENTER, &title, title_font, TEXT);
        x += title_w + 10.0;
        painter.text(egui::pos2(x, rect.center().y), Align2::LEFT_CENTER, &detail, detail_font, MUTED);
        let chevron = Rect::from_center_size(egui::pos2(rect.right() - 22.0, rect.center().y), Vec2::splat(14.0));
        icons::paint(painter, chevron, Icon::ChevronDown, MUTED);

        egui::Popup::from_toggle_button_response(&response)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .align(egui::RectAlign::BOTTOM)
            .gap(6.0)
            .width(340.0)
            .frame(theme::card_frame())
            .show(|ui| controls::phone_menu(self, ui, snap));
    }

    fn status_bar(&self, ui: &mut egui::Ui, snap: &Snapshot) {
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 18.0;
            if let Some(p) = &snap.phone {
                let mut parts = Vec::new();
                if let Some(rtt) = p.info.rtt_ms {
                    parts.push(format!("Latency {rtt:.0} ms"));
                }
                if let Some(b) = p.info.state.as_ref().and_then(|s| s.battery) {
                    let charging = p.info.state.as_ref().is_some_and(|s| s.charging);
                    parts.push(format!("Battery {:.0}%{}", b * 100.0, if charging { " (charging)" } else { "" }));
                }
                if p.dropped > 0 {
                    parts.push(format!("{} frames skipped", p.dropped));
                }
                if !parts.is_empty() {
                    ui.label(RichText::new(parts.join(" · ")).color(MUTED));
                }
                // Only worth the space when the phone is actually getting hot.
                if let Some(state) = &p.info.state {
                    let color = if state.thermal == "critical" { RED } else { ORANGE };
                    if matches!(state.thermal.as_str(), "serious" | "critical") {
                        ui.label(RichText::new(format!("Phone is hot ({})", state.thermal)).color(color));
                    }
                    if let Some(why) = state.camera_paused_hint() {
                        ui.label(RichText::new(why).color(ORANGE));
                    }
                }
                if p.decode_overloaded {
                    ui.label(RichText::new("This computer can't keep up with the video").color(ORANGE)).on_hover_text(
                        "Frames are being skipped so the picture doesn't fall behind. Pick a lower resolution or \
                         frame rate, or turn on \"Use the GPU to decode\" in Settings.",
                    );
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let size = format!("{} × {}", snap.settings.width, snap.settings.height);
                match &snap.output {
                    OutputStatus::VirtualCamera { .. } => {
                        theme::status_dot_rtl(ui, GREEN, &format!("Virtual camera ready · {size}"));
                    }
                    OutputStatus::PreviewOnly { .. } => {
                        theme::status_dot_rtl(ui, ORANGE, "Virtual camera not set up");
                    }
                    OutputStatus::Busy { .. } => {
                        theme::status_dot_rtl(ui, ORANGE, "Virtual camera in use by another program");
                    }
                    OutputStatus::Starting => {
                        theme::status_dot_rtl(ui, MUTED, "Starting virtual camera…");
                    }
                    OutputStatus::Stopped => {
                        theme::status_dot_rtl(ui, MUTED, "Virtual camera stopped");
                    }
                }
            });
        });
    }

    /// The live picture, with the lens / Mirror / Light row right under it.
    fn preview(&mut self, ui: &mut egui::Ui, snap: &Snapshot) {
        let avail = ui.available_rect_before_wrap();
        let has_row = snap.phone.as_ref().is_some_and(|p| p.info.state.is_some());
        let row_h = if has_row { theme::CONTROL_H + 8.0 + 12.0 } else { 0.0 };
        // The capture & scenes strip gets at least what it needs, plus whatever the preview leaves.
        let strip_h = strip::min_height(avail.width()) + 12.0;
        let space = Vec2::new(avail.width(), (avail.height() - row_h - strip_h).max(120.0));
        let out_aspect = snap.settings.width as f32 / snap.settings.height.max(1) as f32;
        let live = snap.phone.as_ref().is_some_and(|p| p.frames > 0);
        let video_aspect = snap.phone.as_ref().and_then(|p| p.info.format.as_ref()).map(|f| {
            let (w, h) = if f.rotation % 180 == 90 { (f.height, f.width) } else { (f.width, f.height) };
            w as f32 / h.max(1) as f32
        });
        let fit = effective_fit(self.ui.fit, video_aspect, snap);

        // Fit to width: the frame hugs the (letterboxed) output picture. Fit to height: the
        // frame takes all the space and the video inside it is as tall as it can be.
        let frame_size = match fit {
            controls::Fit::Height if live => space,
            _ => {
                let mut size = Vec2::new(space.x, space.x / out_aspect);
                if size.y > space.y {
                    size = Vec2::new(space.y * out_aspect, space.y);
                }
                size
            }
        };
        let rect = Rect::from_min_size(egui::pos2(avail.center().x - frame_size.x / 2.0, avail.top()), frame_size);
        let (_, painter) = ui.allocate_painter(avail.size(), Sense::hover());
        let radius = CornerRadius::same(theme::RADIUS);
        painter.rect_filled(rect, radius, Color32::BLACK);

        match (&self.video, live) {
            (Some(tex), true) => {
                if fit == controls::Fit::Height {
                    // Cut the video back out of the output's black borders and fit it by height.
                    let a = video_aspect.unwrap_or(out_aspect);
                    let uv = if a < out_aspect {
                        let w = a / out_aspect;
                        Rect::from_min_max(egui::pos2(0.5 - w / 2.0, 0.0), egui::pos2(0.5 + w / 2.0, 1.0))
                    } else {
                        let h = out_aspect / a;
                        Rect::from_min_max(egui::pos2(0.0, 0.5 - h / 2.0), egui::pos2(1.0, 0.5 + h / 2.0))
                    };
                    let mut size = Vec2::new(rect.height() * a, rect.height());
                    if size.x > rect.width() {
                        size = Vec2::new(rect.width(), rect.width() / a);
                    }
                    let content = Rect::from_center_size(rect.center(), size);
                    let corner = if (size.x - rect.width()).abs() < 1.0 { radius } else { CornerRadius::ZERO };
                    egui::Image::new(tex).uv(uv).corner_radius(corner).paint_at(ui, content);
                } else {
                    egui::Image::new(tex).corner_radius(radius).paint_at(ui, rect);
                }
                if let Some(state) = snap.phone.as_ref().and_then(|p| p.info.state.as_ref()) {
                    let codec = if state.codec == "h264" { "H.264".to_string() } else { state.codec.to_uppercase() };
                    let label = format!(
                        "{}×{} · {} fps · {codec} · {:.0} Mbps",
                        state.width,
                        state.height,
                        state.fps,
                        state.bitrate as f64 / 1e6
                    );
                    theme::overlay_chip(
                        &painter,
                        rect.left_bottom() + Vec2::new(12.0, -12.0),
                        Align2::LEFT_BOTTOM,
                        None,
                        &label,
                        FontId::proportional(13.0),
                    );
                }
                // LIVE only while other apps actually get the video.
                let camera_on = matches!(snap.output, OutputStatus::VirtualCamera { .. });
                let (mut chip, dot) = if camera_on { ("LIVE".to_string(), RED) } else { ("PREVIEW ONLY".to_string(), MUTED) };
                if let Some(rec) = self.engine.recording() {
                    let t = rec.started.elapsed().as_secs();
                    chip = format!("{chip} · REC {:02}:{:02}", t / 60, t % 60);
                }
                let dot = if self.engine.recording().is_some() { RED } else { dot };
                theme::overlay_chip(
                    &painter,
                    rect.left_top() + Vec2::new(12.0, 12.0),
                    Align2::LEFT_TOP,
                    Some(dot),
                    &chip,
                    theme::semibold(12.0),
                );
                self.fit_chip(ui, rect.right_bottom() + Vec2::new(-12.0, -12.0), fit);
                stats::chip(self, ui, snap, rect.right_top() + Vec2::new(-12.0, 12.0));
            }
            _ => self.idle(ui, &painter, rect, snap),
        }
        painter.rect_stroke(rect, radius, Stroke::new(1.0, BORDER), StrokeKind::Inside);

        let mut bottom = rect.bottom();
        if has_row {
            let row = Rect::from_min_size(rect.left_bottom() + Vec2::new(0.0, 12.0), Vec2::new(rect.width(), row_h - 12.0));
            let mut row_ui = ui.new_child(egui::UiBuilder::new().max_rect(row).layout(egui::Layout::left_to_right(egui::Align::Center)));
            controls::under_preview(self, &mut row_ui, snap);
            bottom = row.bottom();
        }
        let strip_rect = Rect::from_min_max(egui::pos2(avail.left(), bottom + 12.0), avail.right_bottom());
        strip::show(self, ui, snap, strip_rect);
    }

    /// The "Fit" chip in the preview's bottom-right corner; opens a menu of fit modes.
    fn fit_chip(&mut self, ui: &mut egui::Ui, anchor: egui::Pos2, fit: controls::Fit) {
        use controls::Fit;
        let what = if fit == Fit::Height { "Fit height" } else { "Fit width" };
        let text = if self.ui.fit == Fit::Auto { format!("Auto · {what}") } else { what.to_string() };
        let font = FontId::proportional(13.0);
        let text_w = ui.fonts_mut(|f| f.layout_no_wrap(text.clone(), font.clone(), TEXT).size().x);
        let rect = Align2::RIGHT_BOTTOM.anchor_size(anchor, Vec2::new(text_w + 20.0 + 18.0, 30.0));
        let response = ui.interact(rect, ui.id().with("fit_chip"), Sense::click());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Preview fit: {text}")));
        if response.hovered() {
            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
        }
        let painter = ui.painter();
        let fill = if response.hovered() { Color32::from_black_alpha(200) } else { theme::SCRIM };
        painter.rect_filled(rect, CornerRadius::same(255), fill);
        painter.text(egui::pos2(rect.left() + 10.0, rect.center().y), Align2::LEFT_CENTER, &text, font, TEXT);
        let chevron = Rect::from_center_size(egui::pos2(rect.right() - 16.0, rect.center().y), Vec2::splat(12.0));
        icons::paint(painter, chevron, Icon::ChevronDown, TEXT);

        egui::Popup::menu(&response).align(egui::RectAlign::TOP_END).gap(6.0).show(|ui| {
            ui.set_min_width(250.0);
            for (mode, label) in [
                (Fit::Auto, "Auto: height for portrait and front camera"),
                (Fit::Width, "Fit to width"),
                (Fit::Height, "Fit to height"),
            ] {
                if ui.selectable_label(self.ui.fit == mode, label).clicked() {
                    self.ui.fit = mode;
                }
            }
        });
    }

    /// What the preview shows when no picture is coming in.
    fn idle(&self, ui: &egui::Ui, painter: &egui::Painter, rect: egui::Rect, snap: &Snapshot) {
        let c = rect.center();
        // Everything scales down with the preview, so a narrow window never clips the text.
        let k = (rect.width() / 640.0).clamp(0.55, 1.0);
        if let Some(logo) = &self.logo {
            let logo_rect = egui::Rect::from_center_size(c - Vec2::new(0.0, 46.0 * k), Vec2::splat(84.0 * k));
            egui::Image::new(logo).corner_radius(CornerRadius::same(20)).paint_at(ui, logo_rect);
        }
        let (title, hint) = match (&snap.phone, &snap.connecting) {
            (Some(_), _) => ("Starting video…".to_string(), "The first picture is on its way.".to_string()),
            (None, Some(name)) => (format!("Connecting to {name}…"), "This takes a second or two.".to_string()),
            (None, None) if snap.phones.is_empty() => (
                "Waiting for your phone".to_string(),
                "Open Pocket Webcam on your phone. Both devices need to be on the same Wi-Fi.".to_string(),
            ),
            (None, None) => (
                "Choose a phone".to_string(),
                "Pick your phone from the menu at the top to start.".to_string(),
            ),
        };
        let wrap = (rect.width() - 40.0).max(120.0);
        let title_galley = painter.layout(title, FontId::proportional(20.0 * k.max(0.8)), TEXT, wrap);
        let hint_galley = painter.layout(hint, FontId::proportional(14.0 * k.max(0.85)), MUTED, wrap);
        let top = c.y + 24.0 * k;
        let title_pos = egui::pos2(c.x - title_galley.size().x / 2.0, top);
        let hint_pos = egui::pos2(c.x - hint_galley.size().x / 2.0, top + title_galley.size().y + 8.0);
        let below = hint_pos.y + hint_galley.size().y;
        painter.galley(title_pos, title_galley, TEXT);
        painter.galley(hint_pos, hint_galley, MUTED);

        // A slow pulse so it's clear the app is looking, not frozen.
        if snap.phone.is_none() && snap.connecting.is_none() {
            let t = self.started.elapsed().as_secs_f32();
            let alpha = (0.5 + 0.5 * (t * 2.4).sin()) * 0.9;
            painter.circle_filled(
                egui::pos2(c.x, below + 18.0),
                4.0,
                ACCENT.gamma_multiply(alpha.clamp(0.15, 0.9)),
            );
        }
    }
}

/// Resolves `Fit::Auto`: fit by height for portrait video or the front camera.
fn effective_fit(fit: controls::Fit, video_aspect: Option<f32>, snap: &Snapshot) -> controls::Fit {
    use controls::Fit;
    if fit != Fit::Auto {
        return fit;
    }
    let portrait = video_aspect.is_some_and(|a| a < 1.0);
    let front = snap.phone.as_ref().is_some_and(|p| {
        let state = p.info.state.as_ref();
        p.info.caps.cameras.iter().any(|c| Some(&c.id) == state.map(|s| &s.camera) && c.position == "front")
    });
    if portrait || front { Fit::Height } else { Fit::Width }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        ctx.request_repaint_after(Duration::from_millis(33));
        self.update_video(&ctx);
        let snap = self.engine.snapshot();
        effects::update_thumbs(self, &ctx, &snap.settings.effects);
        strip::update(self, &ctx, &snap);
        stats::update(self, &snap);

        // Closing the window keeps the app (and the stream) running in the tray.
        if self.tray.is_some()
            && snap.settings.close_to_tray
            && ctx.input(|i| i.viewport().close_requested())
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.window.hide();
        }
        self.frames = self.frames.saturating_add(1);
        if self.hide_at_start && self.frames >= 10 {
            self.hide_at_start = false;
            self.window.hide();
        }

        // The focus ring only shows while navigating with the keyboard.
        ctx.input(|i| {
            if i.key_pressed(egui::Key::Tab) {
                self.focus_visible = true;
            }
            if i.pointer.any_pressed() {
                self.focus_visible = false;
            }
        });

        egui::Panel::top("header")
            .exact_size(56.0)
            .frame(
                egui::Frame::new()
                    .fill(RAISED)
                    .stroke(Stroke::new(1.0, BORDER))
                    .inner_margin(Margin::symmetric(16, 0)),
            )
            .show(ui, |ui| self.header(ui, &snap));

        egui::Panel::bottom("status")
            .exact_size(34.0)
            .frame(
                egui::Frame::new()
                    .fill(RAISED)
                    .stroke(Stroke::new(1.0, BORDER))
                    .inner_margin(Margin::symmetric(16, 0)),
            )
            .show(ui, |ui| self.status_bar(ui, &snap));

        egui::Panel::right("controls")
            .exact_size(360.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(BG).inner_margin(Margin { left: 0, right: 16, top: 16, bottom: 16 }))
            .show(ui, |ui| {
                theme::card_frame().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.set_min_height(ui.available_height());
                    controls::panel(self, ui, &snap);
                });
            });
        effects::finish_drag(self, &ctx);

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(Margin::same(16)))
            .show(ui, |ui| {
                controls::banners(self, ui, &snap);
                self.preview(ui, &snap);
            });

        controls::settings_window(self, &ctx, &snap);
        licenses::window(self, &ctx);
        guide::show(self, &ctx, &snap);
        pairing::show(self, &ctx, &snap);
        if self.focus_visible {
            theme::focus_ring(&ctx);
        }
    }
}
