//! Look and feel: the dark + yellow style of the redesign (Inter, #FFC800 accent, 40 px
//! controls). Every colour, radius and size the app draws with is defined here.

use std::sync::Arc;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Rect, Response,
    RichText, Sense, Stroke, StrokeKind, Ui, Vec2, WidgetInfo, WidgetType,
};

use crate::icons::{self, Icon};

// Surfaces, darkest to lightest.
/// Page background, and the track behind segmented controls inside the panel.
pub const BG: Color32 = Color32::from_rgb(0x0e, 0x0e, 0x10);
/// Header and status bar.
pub const RAISED: Color32 = Color32::from_rgb(0x12, 0x12, 0x14);
/// Cards: the settings panel, popovers, windows.
pub const CARD: Color32 = Color32::from_rgb(0x14, 0x14, 0x16);
/// Controls that sit directly on the page: the lens track, Mirror / Light, the phone pill.
pub const ELEVATED: Color32 = Color32::from_rgb(0x18, 0x18, 0x1a);
/// Controls inside cards: preset cards, dropdowns, secondary buttons.
pub const CONTROL: Color32 = Color32::from_rgb(0x1c, 0x1b, 0x20);
pub const CONTROL_HOVER: Color32 = Color32::from_rgb(0x26, 0x25, 0x2b);

pub const BORDER: Color32 = Color32::from_rgb(0x23, 0x23, 0x27);
pub const BORDER_STRONG: Color32 = Color32::from_rgb(0x2c, 0x2c, 0x30);
pub const SLIDER_TRACK: Color32 = Color32::from_rgb(0x3b, 0x3b, 0x3b);

pub const TEXT: Color32 = Color32::from_rgb(0xf5, 0xf5, 0xf7);
pub const MUTED: Color32 = Color32::from_rgb(0xa1, 0xa1, 0xa8);
/// Captions and hints, one step quieter than `MUTED`.
pub const SUBTLE: Color32 = Color32::from_rgb(0x8a, 0x8a, 0x90);

pub const ACCENT: Color32 = Color32::from_rgb(0xff, 0xc8, 0x00);
/// Text and icons on a yellow fill.
pub const ACCENT_INK: Color32 = Color32::from_rgb(0x14, 0x11, 0x00);
/// Background of a selected card (yellow tint over `CARD`).
pub const ACCENT_TINT: Color32 = Color32::from_rgb(0x2b, 0x26, 0x13);

pub const GREEN: Color32 = Color32::from_rgb(0x22, 0xc5, 0x5e);
pub const ORANGE: Color32 = Color32::from_rgb(0xf5, 0x9e, 0x0b);
pub const RED: Color32 = Color32::from_rgb(0xef, 0x44, 0x44);
/// Chips drawn over the video.
pub const SCRIM: Color32 = Color32::from_rgba_premultiplied(0, 0, 0, 153);

/// Cards and the preview.
pub const RADIUS: u8 = 14;
/// Buttons, dropdowns, preset cards.
pub const RADIUS_CONTROL: u8 = 10;
/// Segments inside a segmented control.
pub const RADIUS_SEGMENT: u8 = 8;
/// Minimum height of anything clickable.
pub const CONTROL_H: f32 = 40.0;
/// Padding between a segmented control's track and its segments.
const TRACK_PAD: f32 = 4.0;

const INTER_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
const INTER_SEMIBOLD: &[u8] = include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf");

fn semibold_family() -> FontFamily {
    FontFamily::Name("semibold".into())
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, semibold_family())
}

/// Text in Inter SemiBold.
pub fn strong(text: impl Into<String>) -> RichText {
    RichText::new(text).family(semibold_family()).color(TEXT)
}

pub fn apply(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert("inter".into(), Arc::new(FontData::from_static(INTER_REGULAR)));
    fonts.font_data.insert("inter-semibold".into(), Arc::new(FontData::from_static(INTER_SEMIBOLD)));
    let fallbacks = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "inter".into());
    fonts.families.insert(semibold_family(), [vec!["inter-semibold".to_string()], fallbacks].concat());
    ctx.set_fonts(fonts);

    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = CARD;
    v.window_stroke = Stroke::new(1.0, BORDER);
    v.extreme_bg_color = BG;
    v.faint_bg_color = RAISED;
    v.hyperlink_color = ACCENT;
    v.selection.bg_fill = ACCENT;
    v.selection.stroke = Stroke::new(1.0, ACCENT_INK);
    v.window_corner_radius = CornerRadius::same(RADIUS);
    v.menu_corner_radius = CornerRadius::same(RADIUS_CONTROL);
    v.popup_shadow = egui::epaint::Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(140) };

    let radius = CornerRadius::same(RADIUS_CONTROL);
    v.widgets.noninteractive.bg_fill = CARD;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.noninteractive.corner_radius = radius;
    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = radius;
        w.fg_stroke = Stroke::new(1.0, TEXT);
        w.expansion = 0.0;
    }
    v.widgets.inactive.bg_fill = CONTROL;
    v.widgets.inactive.weak_bg_fill = CONTROL;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER_STRONG);
    v.widgets.hovered.bg_fill = CONTROL_HOVER;
    v.widgets.hovered.weak_bg_fill = CONTROL_HOVER;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, BORDER_STRONG);
    v.widgets.active.bg_fill = CONTROL_HOVER;
    v.widgets.active.weak_bg_fill = CONTROL_HOVER;
    v.widgets.active.bg_stroke = Stroke::new(1.0, BORDER_STRONG);
    v.widgets.open.weak_bg_fill = CONTROL_HOVER;
    v.widgets.open.bg_stroke = Stroke::new(1.0, BORDER_STRONG);
    ctx.set_visuals(v);

    ctx.global_style_mut(|style| {
        use egui::TextStyle;
        style.text_styles = [
            (TextStyle::Small, FontId::proportional(12.0)),
            (TextStyle::Body, FontId::proportional(14.0)),
            (TextStyle::Button, FontId::proportional(14.0)),
            (TextStyle::Heading, FontId::new(18.0, semibold_family())),
            (TextStyle::Monospace, FontId::monospace(13.0)),
        ]
        .into();
        style.spacing.item_spacing = Vec2::new(8.0, 8.0);
        style.spacing.button_padding = Vec2::new(14.0, 8.0);
        style.spacing.interact_size.y = CONTROL_H;
        style.spacing.combo_height = 320.0;
    });
}

/// Draws a yellow ring around the widget that has keyboard focus. Call once per frame after
/// everything else, only while the user is navigating with the keyboard.
pub fn focus_ring(ctx: &egui::Context) {
    let Some(id) = ctx.memory(|m| m.focused()) else { return };
    let Some(response) = ctx.read_response(id) else { return };
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("focus_ring")));
    painter.rect_stroke(
        response.rect.expand(2.0),
        CornerRadius::same(RADIUS_CONTROL + 2),
        Stroke::new(2.0, ACCENT),
        StrokeKind::Outside,
    );
}

fn pointer_cursor(ui: &Ui, response: &Response) {
    if response.hovered() && response.enabled() {
        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
    }
}

/// The settings panel and popovers: `CARD` fill, 1 px border, 14 px radius.
pub fn card_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(RADIUS))
        .inner_margin(Margin::same(12))
}

/// "QUALITY PRESET"-style heading.
pub fn section_label(ui: &mut Ui, text: &str) {
    ui.label(strong(text.to_uppercase()).size(11.5).color(MUTED).extra_letter_spacing(0.6));
}

/// A thin horizontal rule.
pub fn divider(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, BORDER);
}

/// A row with a label on the left and a control on the right.
pub fn row(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.set_min_height(CONTROL_H);
        ui.label(RichText::new(label).color(TEXT));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), add);
    });
}

/// Label on the left, value in bold on the right (above a slider).
pub fn value_row(ui: &mut Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(TEXT));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(strong(value));
        });
    });
}

/// How a segmented control sizes its segments.
#[derive(Clone, Copy, PartialEq)]
pub enum Segments {
    /// Equal widths filling the available width (frame rate, codec, tabs).
    Fill,
    /// Each segment as wide as its text (lenses).
    Fit,
}

/// Width a `Segments::Fit` control needs for `items`.
pub fn segmented_width(ui: &Ui, items: &[String]) -> f32 {
    let font = semibold(14.0);
    let text: f32 = items
        .iter()
        .map(|t| ui.fonts_mut(|f| f.layout_no_wrap(t.clone(), font.clone(), TEXT).size().x) + 28.0)
        .sum();
    text + TRACK_PAD * 2.0
}

/// A row of options on a dark track; the selected one is filled yellow. Returns the index
/// that was clicked (also when it was already selected).
pub fn segmented(
    ui: &mut Ui,
    id_salt: &str,
    items: &[String],
    selected: Option<usize>,
    track: Color32,
    sizing: Segments,
) -> Option<usize> {
    let font = semibold(14.0);
    let widths: Vec<f32> = match sizing {
        Segments::Fill => {
            let inner = ui.available_width() - TRACK_PAD * 2.0;
            vec![inner / items.len().max(1) as f32; items.len()]
        }
        Segments::Fit => items
            .iter()
            .map(|t| ui.fonts_mut(|f| f.layout_no_wrap(t.clone(), font.clone(), TEXT).size().x) + 28.0)
            .collect(),
    };
    let total = Vec2::new(widths.iter().sum::<f32>() + TRACK_PAD * 2.0, CONTROL_H + TRACK_PAD * 2.0);
    let (track_rect, _) = ui.allocate_exact_size(total, Sense::hover());
    let track_stroke = if track == BG { Stroke::NONE } else { Stroke::new(1.0, BORDER) };
    ui.painter().rect(track_rect, CornerRadius::same(RADIUS_CONTROL + 2), track, track_stroke, StrokeKind::Inside);

    let mut clicked = None;
    let mut x = track_rect.left() + TRACK_PAD;
    for (i, (text, w)) in items.iter().zip(widths).enumerate() {
        let rect = Rect::from_min_size(egui::pos2(x, track_rect.top() + TRACK_PAD), Vec2::new(w, CONTROL_H));
        x += w;
        let response = ui.interact(rect, ui.id().with(id_salt).with(i), Sense::click());
        let is_selected = selected == Some(i);
        response.widget_info(|| WidgetInfo::selected(WidgetType::RadioButton, true, is_selected, text));
        pointer_cursor(ui, &response);
        let (fill, ink) = if is_selected {
            (ACCENT, ACCENT_INK)
        } else if response.hovered() {
            (CONTROL_HOVER, TEXT)
        } else {
            (Color32::TRANSPARENT, MUTED.lerp_to_gamma(TEXT, 0.6))
        };
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(RADIUS_SEGMENT), fill);
        painter.text(rect.center(), Align2::CENTER_CENTER, text, font.clone(), ink);
        if response.clicked() {
            clicked = Some(i);
        }
    }
    clicked
}

/// A selectable card with a title and a small subtitle (quality presets). Selected = yellow
/// border, tinted fill and yellow subtitle.
pub fn preset_card(ui: &mut Ui, title: &str, subtitle: &str, selected: bool, enabled: bool, width: f32) -> Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width, 56.0), if enabled { Sense::click() } else { Sense::hover() });
    response.widget_info(|| WidgetInfo::selected(WidgetType::RadioButton, enabled, selected, format!("{title}, {subtitle}")));
    pointer_cursor(ui, &response);
    let (fill, stroke) = if selected {
        (ACCENT_TINT, ACCENT)
    } else if response.hovered() && enabled {
        (CONTROL_HOVER, BORDER_STRONG)
    } else {
        (CONTROL, BORDER)
    };
    let fade = if enabled { 1.0 } else { 0.4 };
    let painter = ui.painter();
    painter.rect(rect, CornerRadius::same(RADIUS_CONTROL), fill, Stroke::new(1.0, stroke), StrokeKind::Inside);
    let left = rect.left() + 12.0;
    painter.text(
        egui::pos2(left, rect.top() + 11.0),
        Align2::LEFT_TOP,
        title,
        semibold(14.0),
        TEXT.gamma_multiply(fade),
    );
    painter.text(
        egui::pos2(left, rect.top() + 31.0),
        Align2::LEFT_TOP,
        subtitle,
        FontId::proportional(12.0),
        (if selected { ACCENT } else { MUTED }).gamma_multiply(fade),
    );
    response
}

/// An on/off button with an icon and a label, for controls next to the preview (Mirror,
/// Light). On = yellow border and tint. With `label_visible` false only the icon shows and the
/// label becomes the tooltip.
pub fn tool_button(ui: &mut Ui, icon: Icon, label: &str, on: bool, label_visible: bool) -> Response {
    let font = semibold(14.0);
    let text_w = if label_visible { ui.fonts_mut(|f| f.layout_no_wrap(label.into(), font.clone(), TEXT).size().x) } else { 0.0 };
    let width = if label_visible { 14.0 + 18.0 + 8.0 + text_w + 16.0 } else { CONTROL_H };
    let (rect, mut response) = ui.allocate_exact_size(Vec2::new(width, CONTROL_H), Sense::click());
    response.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, true, on, label));
    if !label_visible {
        response = response.on_hover_text(label);
    }
    pointer_cursor(ui, &response);
    let (fill, stroke, ink) = if on {
        (ACCENT_TINT, ACCENT, ACCENT)
    } else if response.hovered() {
        (CONTROL_HOVER, BORDER_STRONG, TEXT)
    } else {
        (ELEVATED, BORDER, TEXT)
    };
    let painter = ui.painter();
    painter.rect(rect, CornerRadius::same(RADIUS_CONTROL), fill, Stroke::new(1.0, stroke), StrokeKind::Inside);
    if label_visible {
        let icon_rect = Rect::from_center_size(egui::pos2(rect.left() + 14.0 + 9.0, rect.center().y), Vec2::splat(18.0));
        icons::paint(painter, icon_rect, icon, ink);
        painter.text(egui::pos2(icon_rect.right() + 8.0, rect.center().y), Align2::LEFT_CENTER, label, font, ink);
    } else {
        icons::paint(painter, Rect::from_center_size(rect.center(), Vec2::splat(18.0)), icon, ink);
    }
    response
}

/// A square icon-only button with a tooltip and an accessible label.
pub fn icon_button(ui: &mut Ui, icon: Icon, label: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(CONTROL_H), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, label));
    pointer_cursor(ui, &response);
    let fill = if response.hovered() { CONTROL_HOVER } else { RAISED };
    let painter = ui.painter();
    painter.rect(rect, CornerRadius::same(RADIUS_CONTROL), fill, Stroke::new(1.0, BORDER_STRONG), StrokeKind::Inside);
    icons::paint(painter, Rect::from_center_size(rect.center(), Vec2::splat(18.0)), icon, TEXT);
    response.on_hover_text(label)
}

pub fn primary_button(ui: &mut Ui, text: &str) -> Response {
    ui.add(
        egui::Button::new(strong(text).color(ACCENT_INK))
            .fill(ACCENT)
            .stroke(Stroke::NONE)
            .min_size(Vec2::new(0.0, CONTROL_H))
            .corner_radius(CornerRadius::same(RADIUS_CONTROL)),
    )
}

/// Height of the panel's full-width main action.
pub const FOOTER_BUTTON_H: f32 = 44.0;

/// The panel footer's main action: full width, yellow, dark text.
pub fn wide_primary_button(ui: &mut Ui, text: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), FOOTER_BUTTON_H), Sense::click());
    let enabled = ui.is_enabled();
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, text));
    pointer_cursor(ui, &response);
    let fill = if response.hovered() && enabled { ACCENT.lerp_to_gamma(Color32::WHITE, 0.15) } else { ACCENT };
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(RADIUS_CONTROL), fill.gamma_multiply(if enabled { 1.0 } else { 0.5 }));
    painter.text(rect.center(), Align2::CENTER_CENTER, text, semibold(15.0), ACCENT_INK);
    response
}

pub fn secondary_button(ui: &mut Ui, text: &str) -> Response {
    ui.add(
        egui::Button::new(RichText::new(text).color(TEXT))
            .min_size(Vec2::new(0.0, CONTROL_H))
            .corner_radius(CornerRadius::same(RADIUS_CONTROL)),
    )
}

/// The old pill-shaped option button, still used by the settings window.
pub fn pill(ui: &mut Ui, selected: bool, text: &str) -> Response {
    let (fill, ink, stroke) = if selected {
        (ACCENT, ACCENT_INK, Stroke::NONE)
    } else {
        (CONTROL, TEXT, Stroke::new(1.0, BORDER_STRONG))
    };
    ui.add(
        egui::Button::new(strong(text).color(ink))
            .fill(fill)
            .stroke(stroke)
            .min_size(Vec2::new(0.0, CONTROL_H))
            .corner_radius(CornerRadius::same(RADIUS_CONTROL)),
    )
}

/// An on/off switch. Returns the response; `changed()` is set when it was clicked.
pub fn switch(ui: &mut Ui, on: bool, label: &str) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(Vec2::new(44.0, CONTROL_H), Sense::click());
    response.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, true, on, label));
    pointer_cursor(ui, &response);
    let rect = Rect::from_center_size(rect.center(), Vec2::new(44.0, 24.0));
    let t = ui.ctx().animate_bool_responsive(response.id, on);
    let track = if on { ACCENT } else { SLIDER_TRACK };
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(255), track);
    let knob_x = egui::lerp((rect.left() + 12.0)..=(rect.right() - 12.0), t);
    painter.circle_filled(egui::pos2(knob_x, rect.center().y), 9.0, if on { ACCENT_INK } else { TEXT });
    if response.clicked() {
        response.mark_changed();
    }
    response
}

/// A full-width slider: grey track, yellow fill and knob. Arrow keys move it by `step` when
/// focused. `changed()` fires while it moves; `drag_stopped()` when a drag ends.
pub fn slider(ui: &mut Ui, value: &mut f64, range: std::ops::RangeInclusive<f64>, step: f64, label: &str) -> Response {
    let (min, max) = (*range.start(), *range.end());
    let (rect, mut response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.0), Sense::click_and_drag());
    let shown = *value;
    response.widget_info(|| {
        let mut info = WidgetInfo::labeled(WidgetType::Slider, true, label);
        info.value = Some(shown);
        info
    });
    pointer_cursor(ui, &response);
    let knob_r = 9.0;
    let left = rect.left() + knob_r;
    let right = rect.right() - knob_r;
    let snap = |v: f64| ((v / step).round() * step).clamp(min, max);

    let before = *value;
    if let Some(pos) = response.interact_pointer_pos() {
        let t = ((pos.x - left) / (right - left)).clamp(0.0, 1.0) as f64;
        *value = snap(min + t * (max - min));
    }
    if response.has_focus() {
        let (dec, inc) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowLeft) || i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::ArrowRight) || i.key_pressed(egui::Key::ArrowUp),
            )
        });
        if dec {
            *value = snap(*value - step);
        }
        if inc {
            *value = snap(*value + step);
        }
    }
    if *value != before {
        response.mark_changed();
    }

    let t = if max > min { ((*value - min) / (max - min)) as f32 } else { 0.0 };
    let knob_x = egui::lerp(left..=right, t);
    let cy = rect.center().y;
    let painter = ui.painter();
    let track = Rect::from_min_max(egui::pos2(left, cy - 3.0), egui::pos2(right, cy + 3.0));
    painter.rect_filled(track, CornerRadius::same(255), SLIDER_TRACK);
    painter.rect_filled(Rect::from_min_max(track.min, egui::pos2(knob_x, track.max.y)), CornerRadius::same(255), ACCENT);
    painter.circle_filled(egui::pos2(knob_x, cy), knob_r, ACCENT);
    response
}

/// Small captions under both ends of a slider ("Smaller" ... "Sharper").
pub fn slider_captions(ui: &mut Ui, left: &str, right: &str) {
    ui.add_space(-ui.spacing().item_spacing.y);
    ui.horizontal(|ui| {
        ui.label(RichText::new(left).size(12.0).color(SUBTLE));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(right).size(12.0).color(SUBTLE));
        });
    });
}

/// A small coloured status dot followed by text.
pub fn status_dot(ui: &mut Ui, color: Color32, text: &str) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
    ui.label(RichText::new(text).color(TEXT));
}

/// Like `status_dot`, for right-to-left layouts (the first item added ends up rightmost).
pub fn status_dot_rtl(ui: &mut Ui, color: Color32, text: &str) {
    ui.label(RichText::new(text).color(TEXT));
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
}

/// A chip over the video: translucent black, fully rounded, optional coloured dot in front.
pub fn overlay_chip(painter: &egui::Painter, anchor: egui::Pos2, align: Align2, dot: Option<Color32>, text: &str, font: FontId) {
    let galley = painter.layout_no_wrap(text.to_string(), font, TEXT);
    let dot_w = if dot.is_some() { 14.0 } else { 0.0 };
    let size = galley.size() + Vec2::new(20.0 + dot_w, 10.0);
    let rect = align.anchor_size(anchor, size);
    painter.rect_filled(rect, CornerRadius::same(255), SCRIM);
    if let Some(color) = dot {
        painter.circle_filled(egui::pos2(rect.left() + 14.0, rect.center().y), 3.5, color);
    }
    painter.galley(egui::pos2(rect.left() + 10.0 + dot_w, rect.center().y - galley.size().y / 2.0), galley, TEXT);
}

/// A right-click menu for `response` that also opens from the keyboard (Shift+F10 while the
/// widget has focus), so it is never mouse-only.
pub fn context_menu(response: &Response, add: impl FnOnce(&mut Ui)) {
    let keyboard = response.has_focus() && response.ctx.input(|i| i.modifiers.shift && i.key_pressed(egui::Key::F10));
    let command = if response.secondary_clicked() || keyboard {
        Some(egui::containers::SetOpenCommand::Bool(true))
    } else if response.clicked() {
        Some(egui::containers::SetOpenCommand::Bool(false))
    } else {
        None
    };
    egui::Popup::menu(response)
        .open_memory(command)
        .gap(4.0)
        .frame(card_frame().inner_margin(Margin::same(6)))
        .show(|ui| {
            ui.set_min_width(200.0);
            add(ui);
        });
}

/// "Record" with a red dot; while recording, "Stop" with a pulsing dot and the running time.
pub fn record_button(ui: &mut Ui, recording: Option<std::time::Duration>, enabled: bool) -> Response {
    let font = semibold(14.0);
    let text = match recording {
        Some(t) => format!("Stop  {:02}:{:02}", t.as_secs() / 60, t.as_secs() % 60),
        None => "Record".to_string(),
    };
    let text_w = ui.fonts_mut(|f| f.layout_no_wrap(text.clone(), font.clone(), TEXT).size().x);
    let size = Vec2::new(14.0 + 14.0 + 8.0 + text_w + 16.0, CONTROL_H);
    let (rect, response) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    let label = if recording.is_some() { "Stop recording" } else { "Record" };
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, label));
    pointer_cursor(ui, &response);
    let fade = if enabled { 1.0 } else { 0.4 };
    let (fill, stroke) = match (recording.is_some(), response.hovered() && enabled) {
        (true, _) => (CONTROL, RED),
        (false, true) => (CONTROL_HOVER, BORDER_STRONG),
        (false, false) => (CONTROL, BORDER),
    };
    let painter = ui.painter();
    painter.rect(rect, CornerRadius::same(RADIUS_CONTROL), fill, Stroke::new(1.0, stroke), StrokeKind::Inside);
    // A slow pulse while recording.
    let pulse = match recording {
        Some(t) => 0.55 + 0.45 * (t.as_secs_f32() * std::f32::consts::PI).cos().abs(),
        None => 1.0,
    };
    let dot = Rect::from_center_size(egui::pos2(rect.left() + 14.0 + 7.0, rect.center().y), Vec2::splat(14.0));
    icons::paint(painter, dot, Icon::Record, RED.gamma_multiply(pulse * fade));
    painter.text(
        egui::pos2(dot.right() + 8.0, rect.center().y),
        Align2::LEFT_CENTER,
        text,
        font,
        TEXT.gamma_multiply(fade),
    );
    response
}
