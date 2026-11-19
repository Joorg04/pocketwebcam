//! Connection health: a chip on the preview with the latency, and a popover with the last
//! minute of latency, frame rate and bitrate. Only measured values are shown; anything not
//! measured (yet) reads "—".

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Shape, Stroke, Vec2};
use pocketwebcam_receiver::engine::Snapshot;

use crate::theme::{self, ACCENT, BORDER, GREEN, MUTED, ORANGE, RED, SCRIM, SUBTLE, TEXT};
use crate::App;

/// How much history the sparklines show.
const WINDOW: Duration = Duration::from_secs(60);
/// One sample per second.
const EVERY: Duration = Duration::from_secs(1);
/// Latency colours: green below the first, amber up to the second, red above.
const GOOD_MS: f64 = 30.0;
const FAIR_MS: f64 = 80.0;

struct Sample {
    at: Instant,
    rtt_ms: Option<f64>,
    fps: f64,
    mbps: f64,
}

#[derive(Default)]
pub struct StatsState {
    samples: VecDeque<Sample>,
    /// Counters at the last sample, to turn them into rates.
    last: Option<(Instant, u64, u64)>,
}

/// Takes a sample every second while a phone is connected; clears the history when it goes.
pub fn update(app: &mut App, snap: &Snapshot) {
    let stats = &mut app.stats;
    let Some(phone) = &snap.phone else {
        stats.samples.clear();
        stats.last = None;
        return;
    };
    let now = Instant::now();
    match stats.last {
        // A new connection restarts the counters.
        Some((_, frames, bytes)) if phone.frames < frames || phone.bytes < bytes => stats.last = None,
        Some((at, frames, bytes)) if now - at >= EVERY => {
            let secs = (now - at).as_secs_f64();
            stats.samples.push_back(Sample {
                at: now,
                rtt_ms: phone.info.rtt_ms,
                fps: (phone.frames - frames) as f64 / secs,
                mbps: (phone.bytes - bytes) as f64 * 8.0 / secs / 1e6,
            });
            stats.last = Some((now, phone.frames, phone.bytes));
        }
        Some(_) => {}
        None => stats.last = Some((now, phone.frames, phone.bytes)),
    }
    while stats.samples.front().is_some_and(|s| now - s.at > WINDOW) {
        stats.samples.pop_front();
    }
}

fn latency_color(ms: f64) -> Color32 {
    if ms < GOOD_MS {
        GREEN
    } else if ms <= FAIR_MS {
        ORANGE
    } else {
        RED
    }
}

/// The latency chip at `anchor` (the preview's top-right corner); opens the popover.
pub fn chip(app: &mut App, ui: &mut egui::Ui, snap: &Snapshot, anchor: Pos2) {
    let Some(phone) = &snap.phone else { return };
    let rtt = phone.info.rtt_ms;
    let text = rtt.map_or("— ms".to_string(), |ms| format!("{ms:.0} ms"));
    let color = rtt.map_or(MUTED, latency_color);
    let font = theme::semibold(13.0);
    let text_w = ui.fonts_mut(|f| f.layout_no_wrap(text.clone(), font.clone(), TEXT).size().x);
    let rect = Align2::RIGHT_TOP.anchor_size(anchor, Vec2::new(12.0 + 14.0 + 6.0 + text_w + 12.0, 30.0));
    let response = ui.interact(rect, ui.id().with("stats_chip"), Sense::click());
    let quality = match rtt {
        Some(ms) if ms < GOOD_MS => "good",
        Some(ms) if ms <= FAIR_MS => "fair",
        Some(_) => "poor",
        None => "not measured yet",
    };
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Connection stats: latency {text}, {quality}"))
    });
    let response = response.on_hover_text("Connection health");
    if response.hovered() {
        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
    }
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(255), if response.hovered() { Color32::from_black_alpha(200) } else { SCRIM });
    // Signal bars: three for good, two for fair, one for poor (so colour is not the only cue).
    let bars = match rtt {
        Some(ms) if ms < GOOD_MS => 3,
        Some(ms) if ms <= FAIR_MS => 2,
        Some(_) => 1,
        None => 0,
    };
    let base = Pos2::new(rect.left() + 12.0, rect.center().y + 6.0);
    for i in 0..3 {
        let h = 4.0 + i as f32 * 4.0;
        let bar = Rect::from_min_max(Pos2::new(base.x + i as f32 * 5.0, base.y - h), Pos2::new(base.x + i as f32 * 5.0 + 3.0, base.y));
        let c = if i < bars { color } else { Color32::from_white_alpha(60) };
        painter.rect_filled(bar, CornerRadius::same(1), c);
    }
    painter.text(Pos2::new(rect.left() + 12.0 + 14.0 + 6.0, rect.center().y), Align2::LEFT_CENTER, text, font, TEXT);

    egui::Popup::from_toggle_button_response(&response)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .align(egui::RectAlign::BOTTOM_END)
        .gap(6.0)
        .width(320.0)
        .frame(theme::card_frame().inner_margin(egui::Margin::same(14)))
        .show(|ui| popover(app, ui, snap));
}

fn popover(app: &App, ui: &mut egui::Ui, snap: &Snapshot) {
    let Some(phone) = &snap.phone else { return };
    let state = phone.info.state.as_ref();
    let samples = &app.stats.samples;
    ui.set_width(292.0);
    theme::section_label(ui, "Connection health");

    let rtt: Vec<(Instant, Option<f64>)> = samples.iter().map(|s| (s.at, s.rtt_ms)).collect();
    let fps: Vec<(Instant, Option<f64>)> = samples.iter().map(|s| (s.at, Some(s.fps))).collect();
    let mbps: Vec<(Instant, Option<f64>)> = samples.iter().map(|s| (s.at, Some(s.mbps))).collect();
    let target_fps = state.map(|s| format!(" of {}", s.fps)).unwrap_or_default();
    let target_mbps = state.map(|s| format!(" of {:.0}", s.bitrate as f64 / 1e6)).unwrap_or_default();

    sparkline(ui, "Latency (round trip)", phone.info.rtt_ms.map(|v| format!("{v:.0} ms")), &rtt, "ms", 0);
    sparkline(ui, "Frame rate", samples.back().map(|s| format!("{:.0}{target_fps} fps", s.fps)), &fps, "fps", 0);
    sparkline(ui, "Bitrate", samples.back().map(|s| format!("{:.1}{target_mbps} Mbps", s.mbps)), &mbps, "Mbps", 1);

    ui.add_space(2.0);
    theme::divider(ui);
    ui.add_space(2.0);

    let total = phone.frames + phone.dropped;
    let dropped = if total > 0 {
        format!("{} ({:.1}%)", phone.dropped, phone.dropped as f64 * 100.0 / total as f64)
    } else {
        "—".to_string()
    };
    stat_row(ui, "Dropped frames", &dropped, Some("Frames this computer had to skip while waiting for a clean keyframe. Frames the phone skips on a busy network aren't reported by the phone yet."));
    let link = match phone.info.addr {
        Some(addr) if addr.ip().is_loopback() => "USB cable",
        Some(_) => "Wi-Fi",
        None => "—",
    };
    stat_row(ui, "Connection", link, None);
    if phone.info.caps.audio.is_some() {
        let mic = match (state, app.engine.mic_stats()) {
            (Some(s), Some(m)) if s.audio => format!("On · {:.0} ms buffered", m.buffered_ms),
            (Some(s), _) if s.mic_permission == "denied" => "Blocked on the phone".to_string(),
            (_, None) => "Unavailable".to_string(),
            _ => "Off".to_string(),
        };
        stat_row(ui, "Microphone", &mic, None);
        if let Some(m) = app.engine.mic_stats() {
            stat_row(
                ui,
                "Audio gaps",
                &(m.underruns + m.dropped).to_string(),
                Some("Times sound arrived late (a short silence) or in a burst (a short skip), usually from busy Wi-Fi."),
            );
        }
    }
    stat_row(ui, "Signal strength", "—", Some("Phones don't share Wi-Fi signal strength with apps."));
    let battery = state
        .and_then(|s| s.battery.map(|b| format!("{:.0}%{}", b * 100.0, if s.charging { " (charging)" } else { "" })))
        .unwrap_or_else(|| "—".to_string());
    stat_row(ui, "Phone battery", &battery, None);
    let thermal = match state.map(|s| s.thermal.as_str()) {
        Some("nominal") => "Normal",
        Some("fair") => "Warm",
        Some("serious") => "Hot",
        Some("critical") => "Too hot",
        _ => "—",
    };
    stat_row(ui, "Phone temperature", thermal, Some("The phone reports how hot it is as a level, not in degrees."));
}

fn stat_row(ui: &mut egui::Ui, label: &str, value: &str, hint: Option<&str>) {
    ui.horizontal(|ui| {
        let r = ui.label(RichText::new(label).size(13.0).color(MUTED));
        if let Some(hint) = hint {
            r.on_hover_text(hint);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(theme::strong(value).size(13.0));
        });
    });
}

/// A one-minute line chart: title and current value above, the line below, a hover readout.
fn sparkline(ui: &mut egui::Ui, title: &str, current: Option<String>, points: &[(Instant, Option<f64>)], unit: &str, decimals: usize) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).size(13.0).color(TEXT));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(theme::strong(current.unwrap_or_else(|| "—".into())).size(13.0));
        });
    });
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 40.0), Sense::hover());
    let painter = ui.painter();
    painter.line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, BORDER));

    let now = Instant::now();
    let max = points.iter().filter_map(|p| p.1).fold(0.0_f64, f64::max).max(1e-6) * 1.15;
    let to_pos = |at: Instant, v: f64| -> Pos2 {
        let age = (now - at).as_secs_f32().min(WINDOW.as_secs_f32());
        let x = rect.right() - age / WINDOW.as_secs_f32() * rect.width();
        let y = rect.bottom() - (v / max) as f32 * (rect.height() - 4.0);
        Pos2::new(x, y)
    };
    if points.iter().all(|p| p.1.is_none()) {
        painter.text(rect.center(), Align2::CENTER_CENTER, "Measuring…", FontId::proportional(12.0), SUBTLE);
        return;
    }
    // Gaps (unmeasured seconds) break the line instead of being drawn as zero.
    let mut run: Vec<Pos2> = Vec::new();
    for &(at, v) in points {
        match v {
            Some(v) => run.push(to_pos(at, v)),
            None if !run.is_empty() => {
                painter.add(Shape::line(std::mem::take(&mut run), Stroke::new(2.0, ACCENT)));
            }
            None => {}
        }
    }
    if run.len() == 1 {
        painter.circle_filled(run[0], 2.5, ACCENT);
    } else if !run.is_empty() {
        painter.add(Shape::line(run, Stroke::new(2.0, ACCENT)));
    }
    painter.text(rect.left_top(), Align2::LEFT_TOP, "60 s", FontId::proportional(10.0), SUBTLE);

    // Hover: crosshair on the nearest sample and its value.
    if let Some(pointer) = response.hover_pos() {
        let nearest = points
            .iter()
            .filter_map(|&(at, v)| v.map(|v| (at, v, to_pos(at, v))))
            .min_by(|a, b| (a.2.x - pointer.x).abs().total_cmp(&(b.2.x - pointer.x).abs()));
        if let Some((at, v, pos)) = nearest {
            painter.line_segment([Pos2::new(pos.x, rect.top()), Pos2::new(pos.x, rect.bottom())], Stroke::new(1.0, MUTED));
            painter.circle(pos, 4.0, ACCENT, Stroke::new(2.0, theme::CARD));
            let ago = (now - at).as_secs();
            response.on_hover_text(format!("{v:.decimals$} {unit} · {ago} s ago"));
        }
    }
}
