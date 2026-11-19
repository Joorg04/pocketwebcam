//! The system tray icon (StatusNotifierItem, so it works with Waybar, KDE, the Omarchy
//! shell and anything else that hosts tray icons).
//!
//! Menu actions run right here on the tray's thread and talk to the engine directly, so
//! they keep working while the window is hidden.

use std::thread;
use std::time::Duration;

use ksni::blocking::{Handle, TrayMethods};
use pocketwebcam_receiver::engine::{Engine, Wanted};

use crate::window::WindowControl;

const LOGO_PNG: &[u8] = include_bytes!("../../../../brand/profile-400.png");

pub struct PocketTray {
    engine: Engine,
    window: WindowControl,
    status: String,
    connected: bool,
    icon: Vec<ksni::Icon>,
}

impl ksni::Tray for PocketTray {
    fn id(&self) -> String {
        "pocketwebcam".into()
    }

    fn title(&self) -> String {
        format!("Pocket Webcam: {}", self.status)
    }

    fn category(&self) -> ksni::Category {
        ksni::Category::ApplicationStatus
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.icon.clone()
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.window.toggle();
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::{MenuItem, StandardItem};

        let mut items: Vec<ksni::MenuItem<Self>> = vec![
            StandardItem { label: self.status.clone(), enabled: false, ..Default::default() }.into(),
            MenuItem::Separator,
            StandardItem {
                label: if self.window.is_visible() { "Hide window" } else { "Show window" }.into(),
                activate: Box::new(|t: &mut Self| t.window.toggle()),
                ..Default::default()
            }
            .into(),
        ];
        if self.connected {
            items.push(
                StandardItem {
                    label: "Disconnect phone".into(),
                    activate: Box::new(|t: &mut Self| t.engine.disconnect()),
                    ..Default::default()
                }
                .into(),
            );
        } else {
            items.push(
                StandardItem {
                    label: "Look for a phone".into(),
                    activate: Box::new(|t: &mut Self| t.engine.connect_auto()),
                    ..Default::default()
                }
                .into(),
            );
        }
        items.push(MenuItem::Separator);
        items.push(
            StandardItem {
                label: "Open log folder".into(),
                activate: Box::new(|_| crate::strip::open_file(&pocketwebcam_receiver::logging::log_dir())),
                ..Default::default()
            }
            .into(),
        );
        items.push(
            StandardItem {
                label: "Quit Pocket Webcam".into(),
                activate: Box::new(|_| std::process::exit(0)),
                ..Default::default()
            }
            .into(),
        );
        items
    }
}

/// Keeps the tray icon alive for as long as it is held.
pub struct TrayGuard(#[allow(dead_code)] Handle<PocketTray>);

/// Starts the tray icon. `None` when there's no tray on this desktop; the window should then
/// quit normally when closed instead of disappearing with no way back.
pub fn spawn(engine: Engine, window: WindowControl) -> Option<TrayGuard> {
    let tray = PocketTray {
        engine: engine.clone(),
        window,
        status: "Waiting for a phone".into(),
        connected: false,
        icon: icons(),
    };
    let handle = match tray.spawn() {
        Ok(h) => h,
        Err(e) => {
            log::warn!("no tray icon: {e}");
            return None;
        }
    };

    // Keep the menu's status line current, independent of the window.
    let updater = handle.clone();
    thread::spawn(move || loop {
        let snap = engine.snapshot();
        let (status, connected) = match (&snap.phone, &snap.connecting) {
            (Some(p), _) => {
                let name = p.info.hello.as_ref().map_or("phone", |h| h.name.as_str());
                (format!("Connected to {name}"), true)
            }
            (None, Some(name)) => (format!("Connecting to {name}…"), false),
            (None, None) if snap.wanted == Wanted::Nothing => ("Disconnected".to_string(), false),
            (None, None) => ("Waiting for a phone".to_string(), false),
        };
        let alive = updater.update(|t: &mut PocketTray| {
            if t.status != status || t.connected != connected {
                t.status = status;
                t.connected = connected;
            }
        });
        if alive.is_none() {
            break;
        }
        thread::sleep(Duration::from_secs(1));
    });
    Some(TrayGuard(handle))
}

/// The logo at the sizes tray hosts ask for, as ARGB32.
fn icons() -> Vec<ksni::Icon> {
    let Ok(img) = image::load_from_memory(LOGO_PNG) else { return Vec::new() };
    let rgba = img.to_rgba8();
    [22u32, 32, 48, 64]
        .into_iter()
        .map(|size| {
            let scaled = image::imageops::resize(&rgba, size, size, image::imageops::FilterType::Lanczos3);
            let mut data = scaled.into_raw();
            for px in data.as_chunks_mut::<4>().0 {
                px.rotate_right(1); // RGBA -> ARGB
            }
            ksni::Icon { width: size as i32, height: size as i32, data }
        })
        .collect()
}
