//! The phone menu, the lens / Mirror / Light row under the preview, the Video | Picture |
//! Effects panel, banners and the settings window.

use eframe::egui::{self, Margin, RichText, ScrollArea, Stroke, TextEdit};
use pocketwebcam_receiver::audio::MicStatus;
use pocketwebcam_receiver::capture::Container;
use pocketwebcam_receiver::engine::{OutputStatus, Snapshot, Wanted};
use pocketwebcam_receiver::protocol::{Camera, Capabilities, ConfigUpdate, State};
use pocketwebcam_receiver::settings::Settings;

use crate::icons::Icon;
use crate::theme::{self, Segments, BG, BORDER, ELEVATED, GREEN, MUTED, ORANGE, RED, TEXT};
use crate::App;

/// How the preview fits the video into the space it has.
#[derive(Clone, Copy, PartialEq)]
pub enum Fit {
    /// Height for portrait or front-camera video, width otherwise.
    Auto,
    /// The preview is as wide as the space and hugs the video.
    Width,
    /// The preview fills the space; the video is as tall as it fits, with black bars beside it.
    Height,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Tab {
    Video,
    Picture,
    Effects,
}

/// Window state that isn't part of the engine.
pub struct UiState {
    pub settings_open: bool,
    pub licenses_open: bool,
    pub guide_open: bool,
    pub guide_step: usize,
    /// The guide put a pairing QR code up (and takes it down when the guide closes).
    pub guide_pairing: bool,
    pub draft: Settings,
    pub tab: Tab,
    pub fit: Fit,
    host_input: String,
    host_error: Option<String>,
    /// Slider values while the user is still dragging (sent on release).
    zoom: Option<f64>,
    bitrate: Option<f64>,
    pub copied: bool,
    /// Whether "start on login" is on; read from the autostart file when Settings opens.
    pub autostart: bool,
    /// The "Save captures to" folder dialog, while it's open.
    folder_picker: Option<std::thread::JoinHandle<Option<std::path::PathBuf>>>,
    autostart_error: Option<String>,
}

impl UiState {
    pub fn new(settings: Settings) -> Self {
        UiState {
            settings_open: false,
            licenses_open: false,
            guide_open: !settings.onboarded,
            guide_step: 0,
            guide_pairing: false,
            draft: settings,
            tab: Tab::Video,
            fit: Fit::Auto,
            host_input: String::new(),
            host_error: None,
            zoom: None,
            bitrate: None,
            copied: false,
            autostart: pocketwebcam_receiver::autostart::is_enabled(),
            autostart_error: None,
            folder_picker: None,
        }
    }
}

#[cfg(windows)]
const DONATE_URL: &str = "https://pocketwebcam.com/donate";

/// The tarball installer and the distro packages put the setup script in different places.
#[cfg(not(windows))]
fn setup_command() -> &'static str {
    static COMMAND: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    COMMAND.get_or_init(pocketwebcam_receiver::paths::loopback_setup_command)
}

/// What the virtual camera is called in other apps' camera lists.
pub const CAMERA_LABEL: &str =
    if cfg!(windows) { "Pocket Webcam (Windows Virtual Camera)" } else { "Pocket Webcam Camera" };

/// How to set up the virtual camera, for when it isn't available.
#[cfg_attr(windows, allow(unused_variables))] // `app` only holds the Linux "Copied" state
pub fn camera_setup_help(app: &mut App, ui: &mut egui::Ui) {
    #[cfg(not(windows))]
    {
        ui.horizontal(|ui| {
            ui.label(RichText::new(setup_command()).monospace().color(TEXT));
            let label = if app.ui.copied { "Copied" } else { "Copy" };
            if theme::secondary_button(ui, label).clicked() {
                ui.ctx().copy_text(setup_command().to_string());
                app.ui.copied = true;
            }
        });
        ui.label(RichText::new("Run it once in a terminal, then press Start virtual camera.").size(13.0).color(MUTED));
    }
    #[cfg(windows)]
    {
        ui.label(
            RichText::new(
                "In the Pocket Webcam folder, right-click install-camera.cmd and choose Run as administrator. Then press Start virtual camera.",
            )
            .size(13.0)
            .color(MUTED),
        );
        if theme::secondary_button(ui, "Show install-camera.cmd").clicked() {
            if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.to_path_buf())) {
                let mut select = std::ffi::OsString::from("/select,");
                select.push(dir.join("install-camera.cmd"));
                let _ = std::process::Command::new("explorer").arg(select).spawn();
            }
        }
    }
}

/// Quality presets: name, size, frame rate.
const PRESETS: [(&str, u32, u32, u32); 3] =
    [("Fast", 1920, 1080, 30), ("Balanced", 1920, 1080, 60), ("Best", 3840, 2160, 60)];

/// Warnings above the preview: missing virtual camera, connection errors.
pub fn banners(app: &mut App, ui: &mut egui::Ui, snap: &Snapshot) {
    if let OutputStatus::PreviewOnly { reason } = &snap.output {
        theme::card_frame().stroke(Stroke::new(1.0, ORANGE)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(theme::strong("Other apps can't see your phone yet").color(ORANGE));
            ui.label(RichText::new(reason).color(MUTED));
            camera_setup_help(app, ui);
        });
        ui.add_space(10.0);
    }
    if let OutputStatus::Busy { reason } = &snap.output {
        theme::card_frame().stroke(Stroke::new(1.0, ORANGE)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(theme::strong("Another program is using your virtual camera").color(ORANGE));
            ui.label(RichText::new(reason).color(MUTED));
        });
        ui.add_space(10.0);
    }
    if mic_blocked(snap) {
        theme::card_frame().stroke(Stroke::new(1.0, ORANGE)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(theme::strong("The phone's microphone is blocked").color(ORANGE));
            ui.label(
                RichText::new("On the iPhone, open Settings › Apps › Pocket Webcam and turn on Microphone. Video works either way.")
                    .color(MUTED),
            );
        });
        ui.add_space(10.0);
    }
    if let Some(err) = &snap.last_error {
        ui.label(RichText::new(err).color(RED));
        ui.add_space(6.0);
    }
}

/// The current phone's settings and capabilities, once the phone has sent them.
fn phone_state(snap: &Snapshot) -> Option<(&State, &Capabilities, &Camera)> {
    let phone = snap.phone.as_ref()?;
    let state = phone.info.state.as_ref()?;
    let caps = &phone.info.caps;
    let camera = caps.cameras.iter().find(|c| c.id == state.camera)?;
    Some((state, caps, camera))
}

/// Contents of the phone menu that opens from the header pill.
pub fn phone_menu(app: &mut App, ui: &mut egui::Ui, snap: &Snapshot) {
    let connected_name = snap.phone.as_ref().and_then(|p| p.info.hello.as_ref()).map(|h| h.name.clone());

    theme::section_label(ui, "Phones");
    if snap.phones.is_empty() && connected_name.is_none() {
        ui.horizontal(|ui| {
            ui.add(egui::Spinner::new().size(14.0));
            ui.label(RichText::new("Looking for phones on your Wi-Fi…").color(MUTED));
        });
    }
    /// What can be done with a phone in the list.
    enum Kind {
        Paired,
        NotPaired,
        TooOld,
    }
    let mut rows: Vec<(String, Kind)> = snap
        .phones
        .iter()
        .map(|p| {
            let kind = if !p.compatible() {
                Kind::TooOld
            } else if snap.is_paired(p) {
                Kind::Paired
            } else {
                Kind::NotPaired
            };
            (p.name.clone(), kind)
        })
        .collect();
    if let Some(name) = &connected_name {
        if !rows.iter().any(|(n, _)| n == name) {
            rows.push((name.clone(), Kind::Paired));
        }
    }
    for (name, kind) in rows {
        let is_connected = connected_name.as_deref() == Some(name.as_str());
        let is_connecting = snap.connecting.as_deref() == Some(name.as_str()) && !is_connected;
        ui.horizontal(|ui| {
            ui.set_min_height(theme::CONTROL_H);
            let dot = if is_connected { GREEN } else if is_connecting { ORANGE } else { BORDER };
            theme::status_dot(ui, dot, &name);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if is_connected {
                    if theme::secondary_button(ui, "Disconnect").clicked() {
                        app.engine.disconnect();
                    }
                } else if is_connecting {
                    ui.add(egui::Spinner::new().size(14.0));
                } else {
                    match kind {
                        Kind::Paired => {
                            if theme::primary_button(ui, "Connect").clicked() {
                                app.engine.connect_phone(&name);
                                app.engine.remember_phone(&name);
                            }
                        }
                        Kind::NotPaired => {
                            if theme::secondary_button(ui, "Pair")
                                .on_hover_text("Pair by comparing a 6-digit code on both screens")
                                .clicked()
                            {
                                app.engine.connect_phone(&name);
                            }
                        }
                        Kind::TooOld => {
                            ui.label(RichText::new("Update the app").color(MUTED)).on_hover_text(
                                "This phone's Pocket Webcam app is too old to connect to this computer. Update it from the App Store or Play Store.",
                            );
                        }
                    }
                }
            });
        });
    }
    ui.add_space(4.0);
    if theme::primary_button(ui, "Pair a phone").clicked() {
        app.engine.start_qr_pairing();
    }
    if snap.wanted == Wanted::Nothing && snap.phone.is_none() && !snap.phones.is_empty() {
        ui.label(RichText::new("Disconnected").color(MUTED));
    }

    ui.add_space(4.0);
    theme::divider(ui);
    ui.add_space(2.0);
    egui::CollapsingHeader::new(RichText::new("Connect by IP address").color(MUTED))
        .id_salt("manual_ip")
        .show(ui, |ui| {
            ui.label(
                RichText::new("Use this if your network blocks automatic discovery. The phone shows its address in the app.")
                    .size(12.0)
                    .color(MUTED),
            );
            ui.horizontal(|ui| {
                ui.add(
                    TextEdit::singleline(&mut app.ui.host_input)
                        .hint_text("192.168.1.42")
                        .desired_width(170.0)
                        .min_size(egui::vec2(0.0, theme::CONTROL_H))
                        .vertical_align(egui::Align::Center),
                );
                if theme::primary_button(ui, "Connect").clicked() {
                    app.ui.host_error = app.engine.connect_host(&app.ui.host_input).err().map(|e| e.to_string());
                }
            });
            if let Some(e) = &app.ui.host_error {
                ui.label(RichText::new(e).size(12.0).color(RED));
            }
        });
}

/// The phone could send audio but the user said no to the permission prompt.
fn mic_blocked(snap: &Snapshot) -> bool {
    snap.phone.as_ref().is_some_and(|p| {
        p.info.caps.audio.is_some() && p.info.state.as_ref().is_some_and(|s| s.mic_permission == "denied")
    })
}

/// Lens picker on the left, Mic, Mirror and Light on the right, as wide as the preview.
pub fn under_preview(app: &mut App, ui: &mut egui::Ui, snap: &Snapshot) {
    let Some((state, caps, camera)) = phone_state(snap) else { return };
    let names: Vec<String> = caps.cameras.iter().map(|c| c.name.clone()).collect();
    let selected = caps.cameras.iter().position(|c| c.id == state.camera);
    let has_torch = camera.features.iter().any(|f| f == "torch");
    // Older phone apps can't send audio; without a virtual mic there's nowhere for it to go.
    let has_mic = caps.audio.is_some() && snap.mic == MicStatus::Ready;

    // Drop the button labels (icons + tooltips only) when the window is too narrow for both.
    let tools = 1.0 + f32::from(u8::from(has_torch)) + f32::from(u8::from(has_mic));
    let full_tools = tools * 112.0 + 8.0;
    let labels = theme::segmented_width(ui, &names) + 16.0 + full_tools <= ui.available_width();

    ui.horizontal(|ui| {
        if let Some(i) = theme::segmented(ui, "lens", &names, selected, ELEVATED, Segments::Fit) {
            let id = &caps.cameras[i].id;
            if *id != state.camera {
                app.engine.send_config(ConfigUpdate { camera: Some(id.clone()), ..Default::default() });
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if has_torch && theme::tool_button(ui, Icon::Bolt, "Light", state.torch, labels).clicked() {
                app.engine.send_config(ConfigUpdate { torch: Some(!state.torch), ..Default::default() });
            }
            if theme::tool_button(ui, Icon::Mirror, "Mirror", state.mirror, labels).clicked() {
                app.engine.send_config(ConfigUpdate { mirror: Some(!state.mirror), ..Default::default() });
            }
            if has_mic {
                mic_button(app, ui, state, labels);
            }
        });
    });
}

/// Turns the phone's microphone on or off, with a level bar along its bottom edge while on.
fn mic_button(app: &App, ui: &mut egui::Ui, state: &State, labels: bool) {
    let blocked = state.mic_permission == "denied";
    let icon = if state.audio { Icon::Mic } else { Icon::MicOff };
    let response = theme::tool_button(ui, icon, "Mic", state.audio, labels);
    let response = if blocked {
        response.on_hover_text("Blocked on the phone. Allow the microphone for Pocket Webcam in the phone's Settings.")
    } else if labels {
        response.on_hover_text("Use the phone's microphone as \"Pocket Webcam Microphone\" in other apps")
    } else {
        response
    };
    if response.clicked() && !blocked {
        app.engine.set_phone_mic(!state.audio);
    }
    if state.audio {
        let level = app.engine.mic_stats().map_or(0.0, |m| m.peak);
        // Speech peaks mostly sit between 0.05 and 0.5; a square root spreads them out.
        let fill = level.sqrt().min(1.0);
        let r = response.rect.shrink2(egui::vec2(10.0, 0.0));
        let bar = egui::Rect::from_min_max(egui::pos2(r.left(), r.bottom() - 6.0), egui::pos2(r.right(), r.bottom() - 4.0));
        let painter = ui.painter();
        painter.rect_filled(bar, egui::CornerRadius::same(1), theme::BORDER);
        let lit = egui::Rect::from_min_max(bar.min, egui::pos2(egui::lerp(bar.left()..=bar.right(), fill), bar.max.y));
        painter.rect_filled(lit, egui::CornerRadius::same(1), theme::ACCENT);
        // Keep the bar moving even when no video frame triggers a repaint.
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
    }
}

/// The right-hand panel: Video | Picture | Effects tabs.
pub fn panel(app: &mut App, ui: &mut egui::Ui, snap: &Snapshot) {
    let tabs = [Tab::Video, Tab::Picture, Tab::Effects];
    let names: Vec<String> = ["Video", "Picture", "Effects"].iter().map(|s| s.to_string()).collect();
    let current = tabs.iter().position(|t| *t == app.ui.tab);
    if let Some(i) = theme::segmented(ui, "tabs", &names, current, BG, Segments::Fill) {
        app.ui.tab = tabs[i];
    }
    ui.add_space(6.0);

    // The tab scrolls; the footer stays pinned to the bottom of the panel.
    let footer_h = theme::FOOTER_BUTTON_H + 12.0;
    ScrollArea::vertical().auto_shrink([false, false]).max_height(ui.available_height() - footer_h).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 10.0;
        match app.ui.tab {
            Tab::Effects => crate::effects::section(app, ui, snap),
            tab => match phone_state(snap) {
                Some((state, caps, camera)) if tab == Tab::Video => video_tab(app, ui, camera, state, caps),
                Some((state, _, camera)) => picture_tab(app, ui, camera, state),
                None => {
                    let text = if snap.phone.is_some() {
                        "Waiting for the phone's settings…"
                    } else {
                        "Connect a phone to change its camera settings. Choose one from the menu at the top."
                    };
                    ui.label(RichText::new(text).color(MUTED));
                }
            },
        }
    });
    ui.add_space(12.0);
    camera_button(app, ui, snap);
}

/// "Start / Stop virtual camera", following the engine's virtual camera status.
fn camera_button(app: &mut App, ui: &mut egui::Ui, snap: &Snapshot) {
    let (text, start, enabled) = match &snap.output {
        OutputStatus::VirtualCamera { .. } => ("Stop virtual camera", false, true),
        OutputStatus::Stopped | OutputStatus::PreviewOnly { .. } | OutputStatus::Busy { .. } => {
            ("Start virtual camera", true, true)
        }
        OutputStatus::Starting => ("Starting virtual camera…", true, false),
    };
    let r = ui.add_enabled_ui(enabled, |ui| theme::wide_primary_button(ui, text)).inner;
    let r = match &snap.output {
        OutputStatus::PreviewOnly { .. } => r.on_hover_text("The virtual camera isn't set up yet; see the message above the preview."),
        OutputStatus::Busy { .. } => r.on_hover_text("Another program is using the virtual camera; see the message above the preview."),
        OutputStatus::VirtualCamera { .. } => r.on_hover_text("Other apps stop seeing the phone. The preview keeps working."),
        _ => r,
    };
    if r.clicked() {
        app.engine.set_virtual_camera(start);
    }
}

fn resolution_name(w: u32, h: u32) -> String {
    let tag = match (w, h) {
        (3840, 2160) => " (4K)",
        (1920, 1080) => " (1080p)",
        (1280, 720) => " (720p)",
        (640, 480) => " (VGA)",
        _ => "",
    };
    format!("{w} × {h}{tag}")
}

fn short_resolution(w: u32, h: u32) -> String {
    match (w, h) {
        (3840, 2160) => "4K".into(),
        _ => format!("{h}p"),
    }
}

fn codec_name(codec: &str) -> String {
    match codec {
        "h264" => "H.264".into(),
        other => other.to_uppercase(),
    }
}

fn video_tab(app: &mut App, ui: &mut egui::Ui, camera: &Camera, state: &State, caps: &Capabilities) {
    theme::section_label(ui, "Quality preset");
    ui.horizontal(|ui| {
        let gap = 8.0;
        ui.spacing_mut().item_spacing.x = gap;
        let width = ((ui.available_width() - 2.0 * gap) / 3.0).floor();
        for (name, w, h, fps) in PRESETS {
            let supported = camera.formats.iter().any(|f| f.width == w && f.height == h && f.fps.contains(&fps));
            let selected = state.width == w && state.height == h && state.fps == fps;
            let subtitle = format!("{} · {fps}", short_resolution(w, h));
            let mut r = theme::preset_card(ui, name, &subtitle, selected, supported, width);
            if !supported {
                r = r.on_hover_text(format!("This lens can't film {subtitle}."));
            }
            if r.clicked() && !selected {
                app.engine.send_config(ConfigUpdate {
                    width: Some(w),
                    height: Some(h),
                    fps: Some(fps),
                    ..Default::default()
                });
            }
        }
    });
    ui.add_space(4.0);
    theme::divider(ui);
    ui.add_space(4.0);

    theme::row(ui, "Resolution", |ui| {
        egui::ComboBox::from_id_salt("resolution")
            .selected_text(resolution_name(state.width, state.height))
            .show_ui(ui, |ui| {
                for f in &camera.formats {
                    let selected = f.width == state.width && f.height == state.height;
                    if ui.selectable_label(selected, resolution_name(f.width, f.height)).clicked() && !selected {
                        // Keep the frame rate if the new size supports it, else the closest.
                        let fps = f.fps.iter().copied().min_by_key(|n| n.abs_diff(state.fps));
                        app.engine.send_config(ConfigUpdate {
                            width: Some(f.width),
                            height: Some(f.height),
                            fps,
                            ..Default::default()
                        });
                    }
                }
            });
    });

    let fps_options: Vec<u32> = camera
        .formats
        .iter()
        .find(|f| f.width == state.width && f.height == state.height)
        .map(|f| f.fps.clone())
        .unwrap_or_default();
    if !fps_options.is_empty() {
        ui.label(RichText::new("Frame rate").color(TEXT));
        let labels: Vec<String> = fps_options.iter().map(|n| n.to_string()).collect();
        let selected = fps_options.iter().position(|n| *n == state.fps);
        if let Some(i) = theme::segmented(ui, "fps", &labels, selected, BG, Segments::Fill) {
            if fps_options[i] != state.fps {
                app.engine.send_config(ConfigUpdate { fps: Some(fps_options[i]), ..Default::default() });
            }
        }
    }

    let codecs: Vec<String> =
        if caps.codecs.is_empty() { vec!["h264".into(), "hevc".into()] } else { caps.codecs.clone() };
    ui.label(RichText::new("Codec").color(TEXT));
    let labels: Vec<String> = codecs.iter().map(|c| codec_name(c)).collect();
    let selected = codecs.iter().position(|c| *c == state.codec);
    if let Some(i) = theme::segmented(ui, "codec", &labels, selected, BG, Segments::Fill) {
        if codecs[i] != state.codec {
            app.engine.send_config(ConfigUpdate { codec: Some(codecs[i].clone()), ..Default::default() });
        }
    }

    let (min, max) = caps.bitrate.as_ref().map_or((2.0, 60.0), |r| (r.min as f64 / 1e6, r.max as f64 / 1e6));
    let mut value = app.ui.bitrate.unwrap_or(state.bitrate as f64 / 1e6);
    theme::value_row(ui, "Bitrate", &format!("{value:.0} Mbps"));
    let r = theme::slider(ui, &mut value, min..=max, 1.0, "Bitrate");
    if r.dragged() {
        app.ui.bitrate = Some(value);
    }
    if r.drag_stopped() || (r.changed() && !r.dragged()) {
        app.ui.bitrate = None;
        app.engine.send_config(ConfigUpdate { bitrate: Some((value * 1e6) as u64), ..Default::default() });
    }
    theme::slider_captions(ui, "Smaller", "Sharper");
}

fn picture_tab(app: &mut App, ui: &mut egui::Ui, camera: &Camera, state: &State) {
    if let Some(range) = &camera.zoom {
        let mut value = app.ui.zoom.unwrap_or(state.zoom.max(range.min));
        theme::value_row(ui, "Zoom", &format!("{value:.1}×"));
        let r = theme::slider(ui, &mut value, range.min..=range.max, 0.1, "Zoom");
        if r.dragged() {
            app.ui.zoom = Some(value);
        }
        if r.drag_stopped() || (r.changed() && !r.dragged()) {
            app.ui.zoom = None;
            app.engine.send_config(ConfigUpdate { zoom: Some(value), ..Default::default() });
        }
    }

    if camera.features.iter().any(|f| f == "stabilization") {
        theme::row(ui, "Stabilization", |ui| {
            if theme::switch(ui, state.stabilization, "Stabilization").changed() {
                app.engine
                    .send_config(ConfigUpdate { stabilization: Some(!state.stabilization), ..Default::default() });
            }
        });
    }
    ui.label(RichText::new("Mirror and Light are under the preview.").size(12.0).color(theme::SUBTLE));

    ui.add_space(4.0);
    theme::divider(ui);
    ui.add_space(4.0);
    theme::section_label(ui, "Troubleshooting");
    ui.label(
        RichText::new("If the picture looks smeared or frozen, ask the phone for a fresh frame.")
            .size(12.0)
            .color(MUTED),
    );
    if theme::secondary_button(ui, "Refresh picture").clicked() {
        app.engine.request_keyframe();
    }
}

/// "Save captures to" and the recording file type, in the settings window.
fn captures_settings(app: &mut App, ui: &mut egui::Ui) {
    // A folder picked in the dialog (it runs on its own thread so the window keeps drawing).
    if app.ui.folder_picker.as_ref().is_some_and(|h| h.is_finished()) {
        if let Some(Some(dir)) = app.ui.folder_picker.take().and_then(|h| h.join().ok()) {
            app.ui.draft.captures_dir = Some(dir.to_string_lossy().to_string());
        }
    }
    let d = &mut app.ui.draft;
    let shown = match &d.captures_dir {
        Some(dir) => dir.clone(),
        None => "Videos and Pictures › Pocket Webcam".to_string(),
    };
    ui.label(RichText::new("Save captures to").color(TEXT));
    ui.horizontal(|ui| {
        ui.add(egui::Label::new(RichText::new(shown).color(MUTED)).truncate()).on_hover_text(
            "Recordings go to Videos › Pocket Webcam and snapshots to Pictures › Pocket Webcam, unless you pick a folder.",
        );
    });
    ui.horizontal(|ui| {
        let picking = app.ui.folder_picker.is_some();
        if ui.add_enabled_ui(!picking, |ui| theme::secondary_button(ui, "Choose folder…")).inner.clicked() {
            app.ui.folder_picker = Some(std::thread::spawn(|| rfd::FileDialog::new().set_title("Save captures to").pick_folder()));
        }
        if app.ui.draft.captures_dir.is_some() && theme::secondary_button(ui, "Use default").clicked() {
            app.ui.draft.captures_dir = None;
        }
    });
    let d = &mut app.ui.draft;
    theme::row(ui, "Recording format", |ui| {
        for c in [Container::Mkv, Container::Mp4] {
            if theme::pill(ui, d.capture_format == c, c.label()).clicked() {
                d.capture_format = c;
            }
        }
    });
    ui.label(
        RichText::new(
            "Recordings keep the phone's own video (H.264 or HEVC, whichever the stream uses), so they cost no extra CPU. \
             Looks and effects are added on this computer and aren't in recordings; snapshots include them.",
        )
        .size(12.0)
        .color(MUTED),
    );
}

pub fn settings_window(app: &mut App, ctx: &egui::Context, snap: &Snapshot) {
    if !app.ui.settings_open {
        return;
    }
    let mut open = true;
    let mut apply = false;
    egui::Window::new(theme::strong("Settings"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(400.0)
        .frame(theme::card_frame().inner_margin(Margin::same(16)))
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            let d = &mut app.ui.draft;
            // Effects and the last phone change outside this window; keep them current so Apply
            // never reverts them.
            d.effects = snap.settings.effects.clone();
            d.last_phone = snap.settings.last_phone.clone();
            d.last_phone_id = snap.settings.last_phone_id.clone();
            d.scenes = snap.settings.scenes.clone();
            theme::section_label(ui, "Virtual camera");
            theme::row(ui, "Size", |ui| {
                let sizes = [(1280, 720, "720p"), (1920, 1080, "1080p"), (2560, 1440, "1440p"), (3840, 2160, "4K")];
                let current = sizes
                    .iter()
                    .find(|s| s.0 == d.width && s.1 == d.height)
                    .map_or(format!("{} × {}", d.width, d.height), |s| s.2.to_string());
                egui::ComboBox::from_id_salt("size").selected_text(current).show_ui(ui, |ui| {
                    for (w, h, name) in sizes {
                        if ui.selectable_label(d.width == w && d.height == h, name).clicked() {
                            d.width = w;
                            d.height = h;
                        }
                    }
                });
            });
            theme::row(ui, "Frame rate", |ui| {
                for n in [60, 30] {
                    if theme::pill(ui, d.fps == n, &format!("{n}")).clicked() {
                        d.fps = n;
                    }
                }
            });
            ui.label(
                RichText::new("Video from the phone is scaled to fit this size, so changing the phone's resolution never breaks your video call.")
                    .size(12.0)
                    .color(MUTED),
            );
            ui.add_space(6.0);
            theme::row(ui, "Device", |ui| {
                ui.add(
                    TextEdit::singleline(&mut d.device)
                        .desired_width(150.0)
                        .min_size(egui::vec2(0.0, theme::CONTROL_H))
                        .vertical_align(egui::Align::Center),
                );
            });
            theme::row(ui, "Use the GPU to decode", |ui| {
                if theme::switch(ui, d.hw_decode, "Use the GPU to decode").changed() {
                    d.hw_decode = !d.hw_decode;
                }
            });
            ui.add_space(6.0);
            theme::section_label(ui, "Microphone");
            match &snap.mic {
                MicStatus::Ready => {
                    theme::row(ui, "Use the phone's mic when it connects", |ui| {
                        if theme::switch(ui, d.mic_auto, "Use the phone's mic when it connects").changed() {
                            d.mic_auto = !d.mic_auto;
                        }
                    });
                    ui.label(RichText::new("Volume boost").size(13.0).color(TEXT));
                    let mut db = f64::from(d.mic_gain_db);
                    if theme::slider(ui, &mut db, 0.0..=12.0, 1.0, "Volume boost").changed() {
                        d.mic_gain_db = db as f32;
                    }
                    theme::slider_captions(ui, "Off", "+12 dB");
                    ui.label(
                        RichText::new("Other apps see it as \"Pocket Webcam Microphone\". Video calls cancel echo themselves, so no setup is needed.")
                            .size(12.0)
                            .color(MUTED),
                    );
                }
                // Windows needs a Microsoft-signed driver for this; donations pay for the certificate.
                #[cfg(windows)]
                MicStatus::Unavailable { .. } => {
                    ui.label(
                        RichText::new("The phone's microphone isn't on Windows yet. Windows only adds a microphone through a driver that Microsoft has signed, and signing needs a certificate that donations pay for.")
                            .size(12.0)
                            .color(MUTED),
                    );
                    ui.hyperlink_to(RichText::new("Support Pocket Webcam").size(12.0), DONATE_URL);
                }
                #[cfg(not(windows))]
                MicStatus::Unavailable { reason } => {
                    ui.label(RichText::new(format!("The phone's microphone can't be used: {reason}")).size(12.0).color(MUTED));
                }
            }
            ui.add_space(6.0);
            theme::section_label(ui, "Captures");
            captures_settings(app, ui);
            let d = &mut app.ui.draft;
            ui.add_space(6.0);
            theme::section_label(ui, "Startup");
            theme::row(ui, "Connect to a phone automatically", |ui| {
                if theme::switch(ui, d.auto_connect, "Connect to a phone automatically").changed() {
                    d.auto_connect = !d.auto_connect;
                }
            });
            theme::row(ui, "Start when I log in", |ui| {
                if theme::switch(ui, app.ui.autostart, "Start when I log in").changed() {
                    let on = !app.ui.autostart;
                    let exe = std::env::current_exe().unwrap_or_else(|_| "pocketwebcam-app".into());
                    match pocketwebcam_receiver::autostart::set_enabled(on, &exe) {
                        Ok(()) => {
                            app.ui.autostart = on;
                            app.ui.autostart_error = None;
                        }
                        Err(e) => app.ui.autostart_error = Some(format!("Couldn't change this: {e}")),
                    }
                }
            });
            if let Some(e) = &app.ui.autostart_error {
                ui.label(RichText::new(e).size(12.0).color(RED));
            }
            if app.tray.is_some() {
                theme::row(ui, "Keep running in the tray when closed", |ui| {
                    if theme::switch(ui, d.close_to_tray, "Keep running in the tray when closed").changed() {
                        d.close_to_tray = !d.close_to_tray;
                    }
                });
            } else {
                ui.label(
                    RichText::new("No system tray found on this desktop, so closing the window quits the app.")
                        .size(12.0)
                        .color(MUTED),
                );
            }

            ui.add_space(10.0);
            theme::divider(ui);
            crate::pairing::paired_list(app, ui, snap);
            let d = &mut app.ui.draft;

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let changed = *d != snap.settings;
                ui.add_enabled_ui(changed, |ui| {
                    if theme::primary_button(ui, "Apply").clicked() {
                        apply = true;
                    }
                });
                let s = &snap.settings;
                let reconnects = (d.width, d.height, d.fps, d.hw_decode, &d.device) != (s.width, s.height, s.fps, s.hw_decode, &s.device);
                if reconnects {
                    ui.label(RichText::new("Applying briefly disconnects the phone.").size(12.0).color(MUTED));
                }
            });

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("Pocket Webcam {}", env!("CARGO_PKG_VERSION"))).size(12.0).color(MUTED));
                if ui.link(RichText::new("Open-source licenses").size(12.0)).clicked() {
                    app.ui.licenses_open = true;
                }
            });
        });
    if apply {
        let draft = app.ui.draft.clone();
        app.engine.apply_settings(draft);
        // Applying restarts the connection; go back to automatic if that's what's saved.
        if app.ui.draft.auto_connect {
            app.engine.connect_auto();
        }
    }
    if !open {
        app.ui.settings_open = false;
    }
}
