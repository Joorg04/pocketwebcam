//! The system tray icon on Windows (notification area), with the same menu as on Linux.
//!
//! The icon lives on its own thread with its own Win32 message loop, so the menu keeps
//! working while the window is hidden (eframe pauses then). Menu actions run on that
//! thread and talk to the engine directly; a once-a-second timer keeps the status line
//! and the menu labels current.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;

use pocketwebcam_receiver::engine::{Engine, Wanted};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, SetTimer, TranslateMessage, MSG, WM_TIMER,
};

use crate::window::WindowControl;

const LOGO_PNG: &[u8] = include_bytes!("../../../../brand/profile-400.png");

/// Keeps the tray icon alive for as long as it is held (it lives until the app quits).
pub struct TrayGuard;

thread_local! {
    /// The icon, so Quit can remove it first (otherwise a dead icon lingers until hovered).
    static TRAY: RefCell<Option<TrayIcon>> = const { RefCell::new(None) };
}

/// Starts the tray icon. `None` if Windows refused it; the window should then quit
/// normally when closed instead of disappearing with no way back.
pub fn spawn(engine: Engine, window: WindowControl) -> Option<TrayGuard> {
    let (ready, started) = mpsc::channel();
    thread::spawn(move || run(engine, window, ready));
    match started.recv() {
        Ok(Ok(())) => Some(TrayGuard),
        Ok(Err(e)) => {
            log::warn!("no tray icon: {e}");
            None
        }
        Err(_) => None,
    }
}

fn run(engine: Engine, window: WindowControl, ready: mpsc::Sender<Result<(), String>>) {
    let status = MenuItem::new("Waiting for a phone", false, None);
    let show = MenuItem::new("Hide window", true, None);
    let connect = MenuItem::new("Look for a phone", true, None);
    let logs = MenuItem::new("Open log folder", true, None);
    let quit = MenuItem::new("Quit Pocket Webcam", true, None);
    let menu = Menu::new();
    let built = menu
        .append_items(&[
            &status,
            &PredefinedMenuItem::separator(),
            &show,
            &connect,
            &PredefinedMenuItem::separator(),
            &logs,
            &quit,
        ])
        .map_err(|e| e.to_string())
        .and_then(|()| {
            TrayIconBuilder::new()
                .with_menu(Box::new(menu))
                .with_menu_on_left_click(false)
                .with_tooltip("Pocket Webcam")
                .with_icon(icon().ok_or("logo did not load")?)
                .build()
                .map_err(|e| e.to_string())
        });
    let tray = match built {
        Ok(t) => t,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    TRAY.with(|t| *t.borrow_mut() = Some(tray));

    // Whether a phone is connected, as last shown; picks what the connect item does.
    let connected = Arc::new(AtomicBool::new(false));
    {
        let (engine, window, connected) = (engine.clone(), window.clone(), connected.clone());
        let (show_id, connect_id, quit_id) = (show.id().clone(), connect.id().clone(), quit.id().clone());
        let logs_id = logs.id().clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if event.id == show_id {
                window.toggle();
            } else if event.id == connect_id {
                if connected.load(Ordering::Relaxed) {
                    engine.disconnect();
                } else {
                    engine.connect_auto();
                }
            } else if event.id == logs_id {
                crate::strip::open_file(&pocketwebcam_receiver::logging::log_dir());
            } else if event.id == quit_id {
                TRAY.with(|t| t.borrow_mut().take());
                std::process::exit(0);
            }
        }));
    }
    {
        let window = window.clone();
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                window.toggle();
            }
        }));
    }
    let _ = ready.send(Ok(()));

    // SAFETY: a thread timer (no window) and the standard message loop for this thread.
    unsafe {
        SetTimer(std::ptr::null_mut(), 0, 1000, None);
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            if msg.message == WM_TIMER {
                let (text, is_connected) = status_text(&engine);
                status.set_text(text);
                connected.store(is_connected, Ordering::Relaxed);
                connect.set_text(if is_connected { "Disconnect phone" } else { "Look for a phone" });
                show.set_text(if window.is_visible() { "Hide window" } else { "Show window" });
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// The status line, as on Linux, and whether a phone is connected.
fn status_text(engine: &Engine) -> (String, bool) {
    let snap = engine.snapshot();
    match (&snap.phone, &snap.connecting) {
        (Some(p), _) => {
            let name = p.info.hello.as_ref().map_or("phone", |h| h.name.as_str());
            (format!("Connected to {name}"), true)
        }
        (None, Some(name)) => (format!("Connecting to {name}…"), false),
        (None, None) if snap.wanted == Wanted::Nothing => ("Disconnected".to_string(), false),
        (None, None) => ("Waiting for a phone".to_string(), false),
    }
}

/// The logo at notification-area size.
fn icon() -> Option<Icon> {
    let img = image::load_from_memory(LOGO_PNG).ok()?.to_rgba8();
    let scaled = image::imageops::resize(&img, 32, 32, image::imageops::FilterType::Lanczos3);
    Icon::from_rgba(scaled.into_raw(), 32, 32).ok()
}
