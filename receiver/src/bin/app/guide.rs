//! The first-run guide: three short steps, shown once and reopenable from the header.

use eframe::egui::{self, CornerRadius, Margin, RichText, Sense, Stroke, Vec2};
use pocketwebcam_receiver::engine::{OutputStatus, PairingView, Snapshot};

use crate::controls::{camera_setup_help, CAMERA_LABEL};
use crate::theme::{self, ACCENT, BORDER, CARD, GREEN, MUTED, ORANGE, RAISED, TEXT};
use crate::pairing;
use crate::App;

/// Where the QR code points. Store links change on the website's /get page, so the app
/// doesn't need an update at launch.
pub const GET_APP_URL: &str = "https://pocketwebcam.com/get";
/// Shown under the QR code, so the address can be typed by hand.
const GET_APP_SHORT: &str = "pocketwebcam.com/get";
const STEPS: usize = 3;

pub fn open(app: &mut App) {
    app.ui.guide_step = 0;
    app.ui.guide_open = true;
}

pub fn show(app: &mut App, ctx: &egui::Context, snap: &Snapshot) {
    if !app.ui.guide_open {
        return;
    }
    let step = app.ui.guide_step.min(STEPS - 1);
    if step != 1 {
        take_down_qr(app, snap);
    }
    let mut finished = false;
    let mut go: Option<usize> = None;

    let modal = egui::Modal::new(egui::Id::new("first_run_guide"))
        .frame(
            egui::Frame::new()
                .fill(CARD)
                .stroke(Stroke::new(1.0, BORDER))
                .corner_radius(CornerRadius::same(theme::RADIUS + 6))
                .inner_margin(Margin::same(26)),
        )
        .show(ctx, |ui| {
            ui.set_width(500.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("STEP {} OF {STEPS}", step + 1)).size(11.0).strong().color(MUTED));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.link(RichText::new("Skip guide").color(MUTED)).clicked() {
                        finished = true;
                    }
                });
            });
            ui.add_space(6.0);

            match step {
                0 => step_get_app(ui),
                1 => step_pair(app, ui, snap),
                _ => step_pick_camera(app, ui, snap),
            }

            ui.add_space(18.0);
            ui.horizontal(|ui| {
                for i in 0..STEPS {
                    let (rect, _) = ui.allocate_exact_size(Vec2::new(if i == step { 22.0 } else { 8.0 }, 8.0), Sense::hover());
                    let color = if i == step { ACCENT } else { BORDER };
                    ui.painter().rect_filled(rect, CornerRadius::same(255), color);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if step + 1 == STEPS {
                        if theme::primary_button(ui, "Done").clicked() {
                            finished = true;
                        }
                    } else if theme::primary_button(ui, "Next").clicked() {
                        go = Some(step + 1);
                    }
                    if step > 0 && theme::secondary_button(ui, "Back").clicked() {
                        go = Some(step - 1);
                    }
                });
            });
        });

    if modal.should_close() {
        finished = true;
    }
    if let Some(s) = go {
        app.ui.guide_step = s;
    }
    if finished {
        take_down_qr(app, snap);
        app.ui.guide_open = false;
        app.ui.guide_step = 0;
        if !snap.settings.onboarded {
            let mut settings = snap.settings.clone();
            settings.onboarded = true;
            app.engine.apply_settings(settings);
        }
    }
}

fn title(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(24.0).strong().color(TEXT));
    ui.add_space(4.0);
}

fn step_get_app(ui: &mut egui::Ui) {
    title(ui, "Get the app on your phone");
    ui.horizontal(|ui| {
        pairing::draw_qr(ui, GET_APP_URL, 176.0);
        ui.add_space(8.0);
        ui.vertical(|ui| {
            ui.label(RichText::new("Scan this with your phone's camera.").size(16.0).color(TEXT));
            ui.add_space(4.0);
            ui.label(RichText::new("Or open this address on your phone:").color(MUTED));
            ui.label(RichText::new(GET_APP_SHORT).monospace().color(ACCENT));
            ui.add_space(10.0);
            ui.label(RichText::new("Free. No account, no watermark, no time limit.").color(MUTED));
        });
    });
}

/// Removes the pairing QR code the guide put up (not one the user opened elsewhere).
fn take_down_qr(app: &mut App, snap: &Snapshot) {
    if std::mem::take(&mut app.ui.guide_pairing) && matches!(snap.pairing, Some(PairingView::Qr { .. })) {
        app.engine.cancel_pairing();
    }
}

fn step_pair(app: &mut App, ui: &mut egui::Ui, snap: &Snapshot) {
    title(ui, "Pair your phone");
    let connected = snap.phone.as_ref().and_then(|p| p.info.hello.as_ref()).map(|h| h.name.clone());
    if let Some(name) = connected {
        take_down_qr(app, snap);
        egui::Frame::new()
            .fill(RAISED)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(CornerRadius::same(theme::RADIUS))
            .inner_margin(Margin::same(14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| theme::status_dot(ui, GREEN, &format!("Paired and connected to {name}")));
            });
        ui.add_space(8.0);
        ui.label(RichText::new("From now on it connects by itself whenever the app is open.").size(13.0).color(MUTED));
        return;
    }
    match &snap.pairing {
        Some(PairingView::Qr { url, left }) => {
            let (url, left) = (url.clone(), *left);
            pairing::qr_panel(app, ui, snap, &url, left);
        }
        Some(PairingView::Code { .. }) => {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(14.0));
                ui.label(RichText::new("Pairing with a code…").color(MUTED));
            });
        }
        None => {
            app.engine.start_qr_pairing();
            app.ui.guide_pairing = true;
        }
    }
    ui.add_space(8.0);
    ui.label(
        RichText::new(
            "Both devices need to be on the same Wi-Fi. If your phone asks to find devices on your local network, tap Allow.",
        )
        .size(13.0)
        .color(MUTED),
    );
}

fn step_pick_camera(app: &mut App, ui: &mut egui::Ui, snap: &Snapshot) {
    title(ui, "Pick it in your video app");
    ui.label(
        RichText::new("In Zoom, Meet, Teams, OBS or your browser, choose this as your camera:").size(15.0).color(TEXT),
    );
    ui.add_space(8.0);
    ui.label(RichText::new(CAMERA_LABEL).size(20.0).strong().color(ACCENT));
    ui.add_space(10.0);

    match &snap.output {
        OutputStatus::VirtualCamera { .. } => {
            ui.horizontal(|ui| theme::status_dot(ui, GREEN, "The virtual camera is ready."));
        }
        OutputStatus::PreviewOnly { reason } => {
            egui::Frame::new()
                .fill(RAISED)
                .stroke(Stroke::new(1.0, ORANGE))
                .corner_radius(CornerRadius::same(theme::RADIUS))
                .inner_margin(Margin::same(12))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new("One thing to set up first").strong().color(ORANGE));
                    ui.label(RichText::new(reason).size(13.0).color(MUTED));
                    camera_setup_help(app, ui);
                });
        }
        OutputStatus::Busy { reason } => {
            egui::Frame::new()
                .fill(RAISED)
                .stroke(Stroke::new(1.0, ORANGE))
                .corner_radius(CornerRadius::same(theme::RADIUS))
                .inner_margin(Margin::same(12))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new("Another program is using the virtual camera").strong().color(ORANGE));
                    ui.label(RichText::new(reason).size(13.0).color(MUTED));
                });
        }
        OutputStatus::Starting => {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(14.0));
                ui.label(RichText::new("Starting the virtual camera…").color(MUTED));
            });
        }
        OutputStatus::Stopped => {
            ui.horizontal(|ui| {
                theme::status_dot(ui, MUTED, "The virtual camera is stopped. Press Start virtual camera to turn it on.")
            });
        }
    }
    ui.add_space(10.0);
    ui.label(
        RichText::new("Change lens, resolution and more any time from this window. Closing it keeps the camera running in the tray.")
            .size(13.0)
            .color(MUTED),
    );
}
