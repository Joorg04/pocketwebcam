//! The Background part of the Effects tab: blur what's behind you, or swap it for a built-in
//! background or a picture of your own. The work happens on this computer, like the other effects.

use std::path::PathBuf;
use std::thread::JoinHandle;

use eframe::egui::{self, ColorImage, RichText, TextureHandle, TextureOptions};
use pocketwebcam_receiver::background::{builtin, check_image};
use pocketwebcam_receiver::effects::{Background, Effects, BUILTIN_PREFIX};
use pocketwebcam_receiver::engine::Snapshot;

use crate::effects::look_tile;
use crate::theme::{self, Segments, BG, MUTED, RED};
use crate::App;

/// Width of the built-in background thumbnails (the height follows the video's shape).
const THUMB_W: usize = 192;

/// What the picker thread hands back: the chosen file and whether it can be used.
type Picked = Option<(PathBuf, Result<(), String>)>;

/// Thumbnails, the file picker, and what to go back to when switching modes.
pub struct BgState {
    /// One texture per entry of `builtin::BUILTINS`.
    builtins: Vec<Option<TextureHandle>>,
    /// The thumbnail size the built-in textures were made at.
    builtin_size: Option<(usize, usize)>,
    /// The open "Choose image…" dialog, if any; it runs (and checks the file) on its own thread.
    picking: Option<JoinHandle<Picked>>,
    /// The last picture of their own the user picked, so its tile stays after switching away.
    user_path: Option<String>,
    /// The last image (built-in or own) and blur strength, picked again by Image and Blur.
    last_image: Option<String>,
    last_strength: f32,
    /// A short problem with the picked file.
    error: Option<String>,
}

impl BgState {
    pub fn new() -> Self {
        BgState {
            builtins: vec![None; builtin::BUILTINS.len()],
            builtin_size: None,
            picking: None,
            user_path: None,
            last_image: None,
            last_strength: Background::DEFAULT_BLUR,
            error: None,
        }
    }
}

/// Renders the built-in thumbnails, once per video shape.
fn update_thumbs(app: &mut App, ctx: &egui::Context, aspect: f32) {
    let th = ((THUMB_W as f32 / aspect.max(0.1)).round() as usize).clamp(2, THUMB_W * 4);
    if app.bg.builtin_size == Some((THUMB_W, th)) {
        return;
    }
    for (i, b) in builtin::BUILTINS.iter().enumerate() {
        if let Some(rgba) = builtin::render_rgba(b.id, THUMB_W, th) {
            let image = ColorImage::from_rgba_unmultiplied([THUMB_W, th], &rgba);
            match &mut app.bg.builtins[i] {
                Some(tex) => tex.set(image, TextureOptions::LINEAR),
                slot => *slot = Some(ctx.load_texture(format!("bg-{}", b.id), image, TextureOptions::LINEAR)),
            }
        }
    }
    app.bg.builtin_size = Some((THUMB_W, th));
}

/// Picks up the result of the "Choose image…" dialog once it closed.
fn poll_picker(app: &mut App, base: &Effects) {
    if !app.bg.picking.as_ref().is_some_and(|h| h.is_finished()) {
        return;
    }
    let Some(handle) = app.bg.picking.take() else { return };
    // `None` means cancelled, or no file dialog on this desktop: nothing to do.
    let Some((path, checked)) = handle.join().ok().flatten() else { return };
    if let Err(e) = checked {
        app.bg.error = Some(e);
        return;
    }
    match path.to_str() {
        Some(p) => {
            app.bg.error = None;
            app.bg.user_path = Some(p.to_string());
            app.fx.dragging = None;
            let background = Background::Image { path: p.to_string() };
            app.engine.set_effects(Effects { background, ..base.clone() }, true);
        }
        None => app.bg.error = Some("That file's name has characters this app can't use. Rename it and try again.".into()),
    }
}

/// Opens the file dialog on its own thread (it blocks until closed) and checks the file there.
fn start_picker(app: &mut App, ctx: &egui::Context) {
    app.bg.error = None;
    let ctx = ctx.clone();
    app.bg.picking = Some(std::thread::spawn(move || {
        // rfd gives no way to tell a cancelled dialog from a missing one.
        let path = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg"]).pick_file();
        let picked = path.map(|p| {
            let checked = check_image(&p);
            (p, checked)
        });
        ctx.request_repaint();
        picked
    }));
}

/// The Background section: Off | Blur | Image, then the blur strength or the image tiles.
pub fn section(app: &mut App, ui: &mut egui::Ui, snap: &Snapshot) {
    let ctx = ui.ctx().clone();
    let aspect = snap.settings.width as f32 / snap.settings.height.max(1) as f32;
    let base = app.fx.dragging.clone().unwrap_or_else(|| snap.settings.effects.clone());
    poll_picker(app, &base);
    if app.bg.picking.is_some() {
        ctx.request_repaint();
    }

    let current = base.background.clone();
    match &current {
        Background::Image { path } => {
            if !path.starts_with(BUILTIN_PREFIX) {
                app.bg.user_path = Some(path.clone());
            }
            app.bg.last_image = Some(path.clone());
        }
        Background::Blur { strength } => app.bg.last_strength = *strength,
        Background::Off => {}
    }
    let set = |app: &mut App, background: Background| {
        app.fx.dragging = None;
        app.bg.error = None;
        app.engine.set_effects(Effects { background, ..base.clone() }, true);
    };

    theme::section_label(ui, "Background");
    let modes: Vec<String> = ["Off", "Blur", "Image"].iter().map(|s| s.to_string()).collect();
    let mode = match current {
        Background::Off => 0,
        Background::Blur { .. } => 1,
        Background::Image { .. } => 2,
    };
    match theme::segmented(ui, "background", &modes, Some(mode), BG, Segments::Fill) {
        Some(0) if mode != 0 => set(app, Background::Off),
        Some(1) if mode != 1 => set(app, Background::Blur { strength: app.bg.last_strength }),
        Some(2) if mode != 2 => {
            let path = app.bg.last_image.clone().unwrap_or_else(|| format!("{BUILTIN_PREFIX}{}", builtin::BUILTINS[0].id));
            set(app, Background::Image { path });
        }
        _ => {}
    }

    match &current {
        Background::Off => {}
        Background::Blur { strength } => {
            let mut pct = (*strength as f64 * 100.0).round();
            theme::value_row(ui, "Strength", &format!("{pct:.0}"));
            let r = theme::slider(ui, &mut pct, 0.0..=100.0, 1.0, "Blur strength");
            let cur = Effects { background: Background::Blur { strength: (pct / 100.0) as f32 }, ..base.clone() };
            if r.drag_stopped() || (r.changed() && !r.dragged()) {
                app.fx.dragging = None;
                app.engine.set_effects(cur, true);
            } else if r.changed() {
                if cur != base {
                    app.engine.set_effects(cur.clone(), false);
                }
                app.fx.dragging = Some(cur);
            }
        }
        Background::Image { path: current_path } => {
            update_thumbs(app, &ctx, aspect);
            // (texture, label, path)
            let mut tiles: Vec<(Option<TextureHandle>, String, String)> = builtin::BUILTINS
                .iter()
                .enumerate()
                .map(|(i, b)| (app.bg.builtins[i].clone(), b.label.to_string(), format!("{BUILTIN_PREFIX}{}", b.id)))
                .collect();
            if let Some(path) = &app.bg.user_path {
                tiles.push((None, "Your image".into(), path.clone()));
            }
            let gap = 8.0;
            let tile_w = ((ui.available_width() - 2.0 * gap) / 3.0).floor();
            let mut picked = None;
            for row in tiles.chunks(3) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    for (tex, label, path) in row {
                        let selected = path == current_path;
                        if look_tile(ui, tex.as_ref(), label, selected, tile_w, aspect).clicked() && !selected {
                            picked = Some(path.clone());
                        }
                    }
                });
            }
            if let Some(path) = picked {
                set(app, Background::Image { path });
            }

            ui.horizontal(|ui| {
                let open = app.bg.picking.is_some();
                ui.add_enabled_ui(!open, |ui| {
                    if theme::secondary_button(ui, "Choose image…").clicked() && !open {
                        start_picker(app, &ctx);
                    }
                });
                if open {
                    ui.label(RichText::new("Choosing…").size(12.0).color(MUTED));
                }
            });
        }
    }
    if let Some(e) = &app.bg.error {
        ui.label(RichText::new(e).size(12.0).color(RED));
    }
}
