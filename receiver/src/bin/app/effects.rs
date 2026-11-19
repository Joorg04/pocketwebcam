//! The Effects tab: a gallery of looks with live thumbnails, plus brightness, contrast,
//! saturation and warmth. Effects are applied on this computer, so the virtual camera and the
//! main preview both show them.

use eframe::egui::{
    self, Align2, ColorImage, CornerRadius, FontId, RichText, Sense, Stroke, StrokeKind, TextureHandle,
    TextureOptions, Vec2,
};
use pocketwebcam_receiver::effects::{apply_yuy2, compile, yuy2_to_rgba, Background, Effects, Look};
use pocketwebcam_receiver::engine::Snapshot;

use crate::theme::{self, ACCENT, BORDER, MUTED};
use crate::App;

/// Gallery thumbnails and slider state.
pub struct FxState {
    /// One texture per entry of `Look::ALL`.
    thumbs: Vec<Option<TextureHandle>>,
    /// Which raw thumbnail frame the textures were made from.
    thumb_seq: Option<u64>,
    /// The slider values the textures were made with.
    thumb_fx: Option<Effects>,
    /// Slider values while the user is still dragging (saved on release).
    pub(crate) dragging: Option<Effects>,
    /// Whether the card was drawn this frame.
    drawn: bool,
}

impl FxState {
    /// Forgets a slider drag in progress (something else just set the effects).
    pub fn reset_drag(&mut self) {
        self.dragging = None;
    }

    pub fn new() -> Self {
        FxState { thumbs: vec![None; Look::ALL.len()], thumb_seq: None, thumb_fx: None, dragging: None, drawn: false }
    }
}

/// Re-renders the look thumbnails when a new small frame arrived or the sliders moved.
pub fn update_thumbs(app: &mut App, ctx: &egui::Context, current: &Effects) {
    let Some(src) = app.engine.preview().raw_thumb() else { return };
    // The background doesn't show in the look thumbnails, so changing it doesn't re-render them.
    let current = &Effects { background: Background::Off, ..app.fx.dragging.as_ref().unwrap_or(current).clone() };
    if app.fx.thumb_seq == Some(src.seq) && app.fx.thumb_fx.as_ref() == Some(current) {
        return;
    }
    if src.width == 0 || src.height == 0 || src.yuy2.len() < src.width * src.height * 2 {
        return;
    }
    for (i, look) in Look::ALL.into_iter().enumerate() {
        let mut yuy2 = src.yuy2.clone();
        apply_yuy2(&compile(&Effects { look, ..current.clone() }), &mut yuy2);
        let rgba = yuy2_to_rgba(&yuy2, src.width, src.height);
        let image = ColorImage::from_rgba_unmultiplied([src.width, src.height], &rgba);
        match &mut app.fx.thumbs[i] {
            Some(tex) => tex.set(image, TextureOptions::LINEAR),
            slot => *slot = Some(ctx.load_texture(format!("look-{}", look.name()), image, TextureOptions::LINEAR)),
        }
    }
    app.fx.thumb_seq = Some(src.seq);
    app.fx.thumb_fx = Some(current.clone());
}

/// Saves a drag that ended without the slider seeing the release (the card went away, or the
/// pointer let go elsewhere), so the value the user saw is kept. Call after the sidebar.
pub fn finish_drag(app: &mut App, ctx: &egui::Context) {
    let drawn = std::mem::take(&mut app.fx.drawn);
    if app.fx.dragging.is_some() && (!drawn || !ctx.input(|i| i.pointer.any_down())) {
        if let Some(fx) = app.fx.dragging.take() {
            app.engine.set_effects(fx, true);
        }
    }
}

/// The Effects tab: look gallery, adjustment sliders, Reset and the background.
pub fn section(app: &mut App, ui: &mut egui::Ui, snap: &Snapshot) {
    app.fx.drawn = true;
    let base = app.fx.dragging.clone().unwrap_or_else(|| snap.settings.effects.clone());
    let aspect = snap.settings.width as f32 / snap.settings.height.max(1) as f32;

    theme::section_label(ui, "Looks");
    let gap = 8.0;
    let tile_w = ((ui.available_width() - 2.0 * gap) / 3.0).floor();
    for (row, looks) in Look::ALL.chunks(3).enumerate() {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for (col, &look) in looks.iter().enumerate() {
                let tex = app.fx.thumbs[row * 3 + col].as_ref();
                if look_tile(ui, tex, look.label(), look == base.look, tile_w, aspect).clicked() && look != base.look {
                    app.fx.dragging = None;
                    app.engine.set_effects(Effects { look, ..base.clone() }, true);
                }
            }
        });
    }

    ui.add_space(4.0);
    theme::divider(ui);
    ui.add_space(4.0);
    theme::section_label(ui, "Adjust");
    let mut cur = base.clone();
    let mut live = false;
    let mut commit = false;
    let fields: [(&str, &mut f32); 4] = [
        ("Brightness", &mut cur.brightness),
        ("Contrast", &mut cur.contrast),
        ("Saturation", &mut cur.saturation),
        ("Warmth", &mut cur.temperature),
    ];
    for (label, field) in fields {
        let mut pct = (*field as f64 * 100.0).round();
        theme::value_row(ui, label, &format!("{pct:.0}"));
        let r = theme::slider(ui, &mut pct, -100.0..=100.0, 1.0, label);
        if r.changed() {
            *field = (pct / 100.0) as f32;
            live = true;
        }
        if r.drag_stopped() || (r.changed() && !r.dragged()) {
            commit = true;
        }
    }
    if commit {
        app.fx.dragging = None;
        app.engine.set_effects(cur, true);
    } else if live {
        if cur != base {
            app.engine.set_effects(cur.clone(), false);
        }
        app.fx.dragging = Some(cur);
    }

    ui.add_enabled_ui(!base.is_default(), |ui| {
        if theme::secondary_button(ui, "Reset").clicked() {
            app.fx.dragging = None;
            // Reset is about looks and sliders; Background has its own Off.
            app.engine.set_effects(Effects { background: base.background.clone(), ..Effects::default() }, true);
        }
    });

    #[cfg(feature = "background")]
    {
        ui.add_space(4.0);
        theme::divider(ui);
        ui.add_space(4.0);
        crate::background::section(app, ui, snap);
    }

    ui.label(
        RichText::new("Effects are added on this computer, so they work with any phone and every video app.")
            .size(12.0)
            .color(MUTED),
    );
}

/// A clickable thumbnail with its name underneath; the selected one gets a yellow outline.
pub(crate) fn look_tile(
    ui: &mut egui::Ui,
    tex: Option<&TextureHandle>,
    label: &str,
    selected: bool,
    width: f32,
    aspect: f32,
) -> egui::Response {
    let image_h = (width / aspect.max(0.1)).round();
    let label_h = 20.0;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, image_h + label_h), Sense::click());
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, selected, label));
    if response.hovered() {
        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
    }
    let image_rect = egui::Rect::from_min_size(rect.min, Vec2::new(width, image_h));
    let radius = CornerRadius::same(theme::RADIUS_SEGMENT);
    let painter = ui.painter();
    match tex {
        Some(tex) => egui::Image::new(tex).corner_radius(radius).paint_at(ui, image_rect),
        None => {
            painter.rect_filled(image_rect, radius, theme::CONTROL);
        }
    }
    let stroke = if selected {
        Stroke::new(2.0, ACCENT)
    } else if response.hovered() {
        Stroke::new(1.0, theme::BORDER_STRONG)
    } else {
        Stroke::new(1.0, BORDER)
    };
    painter.rect_stroke(image_rect, radius, stroke, StrokeKind::Inside);
    painter.text(
        egui::pos2(rect.center().x, image_rect.bottom() + 3.0),
        Align2::CENTER_TOP,
        label,
        FontId::proportional(12.0),
        if selected { ACCENT } else { MUTED },
    );
    response
}
