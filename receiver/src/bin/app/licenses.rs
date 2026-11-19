//! The open-source licenses window (Settings → Open-source licenses): the text of
//! packaging/THIRD-PARTY-NOTICES.txt, which scripts/third-party-notices.sh writes.

use std::sync::OnceLock;

use eframe::egui::{self, FontId, Margin, RichText, ScrollArea};

use crate::theme::{self, MUTED, TEXT};
use crate::App;

const NOTICES: &str = include_str!("../../../packaging/THIRD-PARTY-NOTICES.txt");

/// The notices split into lines, so only the visible ones are laid out (it's ~9000 lines).
fn lines() -> &'static [&'static str] {
    static LINES: OnceLock<Vec<&'static str>> = OnceLock::new();
    LINES.get_or_init(|| NOTICES.lines().collect())
}

pub fn window(app: &mut App, ctx: &egui::Context) {
    if !app.ui.licenses_open {
        return;
    }
    let mut open = true;
    egui::Window::new(theme::strong("Open-source licenses"))
        .open(&mut open)
        .collapsible(false)
        .default_size([620.0, 520.0])
        .frame(theme::card_frame().inner_margin(Margin::same(16)))
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label(
                RichText::new(format!(
                    "Pocket Webcam {} is built on the work of others. Thank you to everyone listed here.",
                    env!("CARGO_PKG_VERSION")
                ))
                .color(TEXT),
            );
            ui.add_space(8.0);
            let lines = lines();
            let font = FontId::monospace(11.5);
            let row = ui.fonts_mut(|f| f.row_height(&font));
            ScrollArea::vertical().auto_shrink(false).show_rows(ui, row, lines.len(), |ui, range| {
                for line in &lines[range] {
                    ui.label(RichText::new(*line).font(font.clone()).color(MUTED));
                }
            });
        });
    if !open {
        app.ui.licenses_open = false;
    }
}
