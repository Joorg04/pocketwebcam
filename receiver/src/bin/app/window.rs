//! Showing and hiding the window from the tray.
//!
//! winit cannot hide a Wayland window, so on Hyprland the window is moved to a hidden
//! special workspace instead; everywhere else the normal visibility commands are used.
//! These calls are made from the tray's thread, never from the UI loop: a hidden
//! Wayland window stops receiving frame callbacks, which pauses the UI loop.
//!
//! On Windows eframe also stops updating a hidden window, and it only carries out
//! viewport commands during an update, so a hidden window could never be shown again
//! that way. There the window is shown and hidden with Win32 calls on its HWND.

use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use eframe::egui;

/// Matches the window by its app id (a regex, so the dots match themselves and more).
const CLASS: &str = "com.pocketwebcam.app";
const HIDDEN_WORKSPACE: &str = "special:pocketwebcam";

#[derive(Clone)]
pub struct WindowControl {
    ctx: egui::Context,
    visible: Arc<AtomicBool>,
    hyprland: bool,
    /// The window's HWND once it exists (0 before).
    #[cfg(windows)]
    hwnd: Arc<std::sync::atomic::AtomicIsize>,
}

impl WindowControl {
    pub fn new(ctx: egui::Context) -> Self {
        WindowControl {
            ctx,
            visible: Arc::new(AtomicBool::new(true)),
            hyprland: std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some(),
            #[cfg(windows)]
            hwnd: Arc::new(std::sync::atomic::AtomicIsize::new(0)),
        }
    }

    /// Remembers the native window, so it can be shown again while eframe is paused.
    #[cfg(windows)]
    pub fn set_native(&self, cc: &eframe::CreationContext<'_>) {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if let Ok(handle) = cc.window_handle() {
            if let RawWindowHandle::Win32(h) = handle.as_raw() {
                self.hwnd.store(h.hwnd.get(), Ordering::Relaxed);
            }
        }
    }

    pub fn is_visible(&self) -> bool {
        #[cfg(windows)]
        if let Some(hwnd) = self.native() {
            // SAFETY: IsWindowVisible only reads the window's state.
            return unsafe { win::IsWindowVisible(hwnd) != 0 };
        }
        self.visible.load(Ordering::Relaxed)
    }

    #[cfg(windows)]
    fn native(&self) -> Option<win::HWND> {
        let raw = self.hwnd.load(Ordering::Relaxed);
        (raw != 0).then_some(raw as win::HWND)
    }

    pub fn toggle(&self) {
        if self.is_visible() {
            self.hide();
        } else {
            self.show();
        }
    }

    pub fn hide(&self) {
        #[cfg(windows)]
        if let Some(hwnd) = self.native() {
            // SAFETY: ShowWindow on our own top-level window; fine from any thread.
            unsafe { win::ShowWindow(hwnd, win::SW_HIDE) };
            self.visible.store(false, Ordering::Relaxed);
            return;
        }
        let done = self.hyprland && move_window(HIDDEN_WORKSPACE, false);
        if !done {
            self.ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        self.visible.store(false, Ordering::Relaxed);
    }

    pub fn show(&self) {
        #[cfg(windows)]
        if let Some(hwnd) = self.native() {
            win::bring_forward(hwnd);
            self.visible.store(true, Ordering::Relaxed);
            self.ctx.request_repaint();
            return;
        }
        let done = self.hyprland && {
            let workspace = active_workspace().unwrap_or_else(|| "e+0".to_string());
            move_window(&workspace, true)
        };
        if !done {
            self.ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            self.ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            self.ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        self.visible.store(true, Ordering::Relaxed);
        self.ctx.request_repaint();
    }
}

/// Runs one `hyprctl` command; true if Hyprland answered "ok".
fn hyprctl(args: &[&str]) -> bool {
    Command::new("hyprctl")
        .args(args)
        .stderr(Stdio::null())
        .output()
        .is_ok_and(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "ok")
}

/// Moves our window to `workspace` (and focuses it when `focus`).
///
/// Hyprland 0.5x+ takes Lua in `hyprctl dispatch`; older versions take dispatcher names.
/// The Lua form is tried first.
fn move_window(workspace: &str, focus: bool) -> bool {
    let lua = format!(
        "hl.dsp.window.move({{ workspace = \"{workspace}\", follow = false, window = \"class:{CLASS}\" }})"
    );
    let moved = hyprctl(&["dispatch", &lua])
        || hyprctl(&[
            "dispatch",
            if focus { "movetoworkspace" } else { "movetoworkspacesilent" },
            &format!("{workspace},class:{CLASS}"),
        ]);
    if !moved {
        return false;
    }
    if focus {
        let lua = format!("hl.dsp.focus({{ window = \"class:{CLASS}\" }})");
        let _ = hyprctl(&["dispatch", &lua]) || hyprctl(&["dispatch", "focuswindow", &format!("class:{CLASS}")]);
    }
    true
}

fn active_workspace() -> Option<String> {
    let out = Command::new("hyprctl").args(["activeworkspace", "-j"]).output().ok()?;
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    json["id"].as_i64().map(|id| id.to_string())
}

/// Brings an already-running instance's window forward (used by a second launch).
#[cfg(windows)]
pub fn raise_existing() {
    let title: Vec<u16> = "Pocket Webcam".encode_utf16().chain(Some(0)).collect();
    // SAFETY: FindWindowW only reads the NUL-terminated title.
    let hwnd = unsafe { win::FindWindowW(std::ptr::null(), title.as_ptr()) };
    if !hwnd.is_null() {
        win::bring_forward(hwnd);
    }
}

#[cfg(windows)]
mod win {
    pub use windows_sys::Win32::Foundation::HWND;
    pub use windows_sys::Win32::UI::WindowsAndMessaging::{
        FindWindowW, IsWindowVisible, SetForegroundWindow, ShowWindow, SW_HIDE, SW_RESTORE, SW_SHOW,
    };

    /// Shows, un-minimises and focuses a window.
    pub fn bring_forward(hwnd: HWND) {
        // SAFETY: plain window calls on a valid top-level window.
        unsafe {
            ShowWindow(hwnd, SW_SHOW);
            ShowWindow(hwnd, SW_RESTORE);
            SetForegroundWindow(hwnd);
        }
    }
}

/// Brings an already-running instance's window forward (used by a second launch).
#[cfg(not(windows))]
pub fn raise_existing() {
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
        let workspace = active_workspace().unwrap_or_else(|| "e+0".to_string());
        move_window(&workspace, true);
    }
}
