//! Pairing in the window: the QR code to scan, the 6-digit code to compare, and the list
//! of paired phones (in Settings).

use std::sync::Mutex;
use std::time::Duration;

use eframe::egui::{self, Color32, CornerRadius, Margin, RichText, Sense, Stroke, Vec2};
use pocketwebcam_receiver::engine::{PairingView, Snapshot};
use pocketwebcam_receiver::pairing;
use pocketwebcam_receiver::session::PromptState;
use qrcode::{Color as QrColor, QrCode};

use crate::theme::{self, ACCENT, BORDER, CARD, MUTED, RAISED, TEXT};
use crate::App;

/// Modules of the last QR code drawn, so it isn't worked out again every frame.
static QR_CACHE: Mutex<Vec<(String, usize, Vec<bool>)>> = Mutex::new(Vec::new());

/// A QR code for `text` on a white card (scanners need dark-on-light, with a quiet zone).
pub fn draw_qr(ui: &mut egui::Ui, text: &str, size: f32) {
    let (n, dark) = {
        let mut cache = QR_CACHE.lock().unwrap();
        if let Some((_, n, dark)) = cache.iter().find(|(t, ..)| t == text) {
            (*n, dark.clone())
        } else {
            let code = QrCode::new(text).expect("the link fits in a QR code");
            let dark: Vec<bool> = code.to_colors().into_iter().map(|c| c == QrColor::Dark).collect();
            cache.push((text.to_string(), code.width(), dark.clone()));
            if cache.len() > 4 {
                cache.remove(0);
            }
            (code.width(), dark)
        }
    };
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "QR code"));
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(14), Color32::WHITE);
    let quiet = 2.5;
    let module = size / (n as f32 + quiet * 2.0);
    let origin = rect.min + Vec2::splat(quiet * module);
    for y in 0..n {
        for x in 0..n {
            if dark[y * n + x] {
                let min = origin + Vec2::new(x as f32 * module, y as f32 * module);
                // Slight overlap so rounding never leaves hairline gaps between modules.
                let r = egui::Rect::from_min_size(min, Vec2::splat(module + 0.6));
                painter.rect_filled(r, CornerRadius::ZERO, Color32::from_rgb(0x0b, 0x0b, 0x0c));
            }
        }
    }
}

fn countdown(left: Duration) -> String {
    let s = left.as_secs();
    format!("{}:{:02}", s / 60, s % 60)
}

/// The QR code to scan, how to scan it, and pairing by code for phones without a camera
/// view. Used by the pairing dialog and the first-run guide.
pub fn qr_panel(app: &mut App, ui: &mut egui::Ui, snap: &Snapshot, url: &str, left: Duration) {
    ui.horizontal(|ui| {
        if left.is_zero() {
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(196.0), Sense::hover());
            ui.painter().rect(rect, CornerRadius::same(14), RAISED, Stroke::new(1.0, BORDER), egui::StrokeKind::Inside);
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "Code expired", theme::semibold(14.0), MUTED);
        } else {
            draw_qr(ui, url, 196.0);
        }
        ui.add_space(10.0);
        ui.vertical(|ui| {
            ui.set_max_width(250.0);
            ui.label(RichText::new("On your phone:").size(15.0).color(TEXT));
            ui.add_space(4.0);
            for (n, step) in ["Open Pocket Webcam.", "Tap Pair with a computer.", "Point the camera at this code."]
                .iter()
                .enumerate()
            {
                ui.label(RichText::new(format!("{}.  {step}", n + 1)).color(TEXT));
            }
            ui.add_space(10.0);
            if left.is_zero() {
                if theme::primary_button(ui, "New code").clicked() {
                    app.engine.start_qr_pairing();
                }
            } else {
                ui.label(RichText::new(format!("This code works for {}.", countdown(left))).size(12.0).color(MUTED));
            }
            ui.add_space(4.0);
            ui.label(
                RichText::new("Pairing is once per phone. After that it connects by itself, and only paired phones can.")
                    .size(12.0)
                    .color(MUTED),
            );
        });
    });
    pair_by_code_list(app, ui, snap);
}

/// Phones on the network that aren't paired, each with a button to pair by code.
fn pair_by_code_list(app: &mut App, ui: &mut egui::Ui, snap: &Snapshot) {
    let unpaired: Vec<String> =
        snap.phones.iter().filter(|p| p.compatible() && !snap.is_paired(p)).map(|p| p.name.clone()).collect();
    if unpaired.is_empty() {
        return;
    }
    ui.add_space(12.0);
    egui::CollapsingHeader::new(RichText::new("Can't scan? Pair with a code instead").color(MUTED))
        .id_salt("pair_by_code")
        .show(ui, |ui| {
            for name in unpaired {
                ui.horizontal(|ui| {
                    ui.set_min_height(theme::CONTROL_H);
                    theme::status_dot(ui, MUTED, &name);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::secondary_button(ui, "Pair with code").clicked() {
                            app.engine.connect_phone(&name);
                        }
                    });
                });
            }
        });
}

fn modal_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(theme::RADIUS + 6))
        .inner_margin(Margin::same(26))
}

/// The pairing dialogs. Call after the guide, so a code question shows on top of it.
pub fn show(app: &mut App, ctx: &egui::Context, snap: &Snapshot) {
    match &snap.pairing {
        // The guide shows the QR code itself.
        Some(PairingView::Qr { url, left }) if !app.ui.guide_open => {
            let (url, left) = (url.clone(), *left);
            let modal = egui::Modal::new(egui::Id::new("pair_qr")).frame(modal_frame()).show(ctx, |ui| {
                ui.set_width(500.0);
                ui.label(RichText::new("Pair a phone").size(24.0).strong().color(TEXT));
                ui.add_space(10.0);
                qr_panel(app, ui, snap, &url, left);
                ui.add_space(14.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::secondary_button(ui, "Close").clicked() {
                        app.engine.cancel_pairing();
                    }
                });
            });
            if modal.should_close() {
                app.engine.cancel_pairing();
            }
        }
        Some(PairingView::Code { phone, state }) if state.code.is_some() && !state.finished => {
            let (phone, state) = (phone.clone(), state.clone());
            let modal = egui::Modal::new(egui::Id::new("pair_code")).frame(modal_frame()).show(ctx, |ui| {
                ui.set_width(440.0);
                code_question(app, ui, &phone, &state);
            });
            if modal.should_close() {
                app.engine.cancel_pairing();
            }
        }
        _ => {}
    }
}

fn code_question(app: &mut App, ui: &mut egui::Ui, phone: &str, state: &PromptState) {
    let code = state.code.as_deref().unwrap_or_default();
    ui.label(RichText::new(format!("Pair with {phone}")).size(24.0).strong().color(TEXT));
    ui.add_space(6.0);
    ui.label(RichText::new("Your phone shows a code too. Check that it's the same:").color(TEXT));
    ui.add_space(12.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(pairing::spaced(code)).size(44.0).monospace().strong().color(ACCENT));
    });
    ui.add_space(12.0);
    let waiting_for_phone = state.answer == Some(true) && !state.phone_accepted;
    if waiting_for_phone {
        ui.horizontal(|ui| {
            ui.add(egui::Spinner::new().size(14.0));
            ui.label(RichText::new("Now tap Pair on the phone.").color(MUTED));
        });
    } else if state.phone_accepted {
        ui.label(RichText::new("The phone said yes. Do the codes match?").color(MUTED));
    } else {
        ui.label(
            RichText::new("If they match, tap Pair on the phone and confirm here. If they don't, cancel: something else answered.")
                .size(13.0)
                .color(MUTED),
        );
    }
    ui.add_space(16.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if !waiting_for_phone && theme::primary_button(ui, "The codes match").clicked() {
            app.engine.answer_code(true);
        }
        if theme::secondary_button(ui, "Cancel").clicked() {
            app.engine.cancel_pairing();
        }
    });
}

/// The paired phones, for the Settings window, each with a Forget button.
pub fn paired_list(app: &mut App, ui: &mut egui::Ui, snap: &Snapshot) {
    theme::section_label(ui, "Paired phones");
    if snap.paired.is_empty() {
        ui.label(RichText::new("No phones yet.").color(MUTED));
    }
    let mut paired = snap.paired.clone();
    paired.sort_by(|a, b| a.name.cmp(&b.name));
    for p in paired {
        ui.horizontal(|ui| {
            ui.set_min_height(theme::CONTROL_H);
            ui.label(RichText::new(&p.name).color(TEXT));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::secondary_button(ui, "Forget")
                    .on_hover_text("This phone can't connect again until you pair it again.")
                    .clicked()
                {
                    app.engine.forget_phone(&p.fingerprint);
                }
            });
        });
    }
    ui.horizontal(|ui| {
        if theme::secondary_button(ui, "Pair a phone").clicked() {
            app.ui.settings_open = false;
            app.engine.start_qr_pairing();
        }
    });
}
