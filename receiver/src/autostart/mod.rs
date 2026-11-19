//! "Start when I log in". The system's own record is the setting (an autostart file on
//! Linux, a registry value on Windows), so there's nothing to keep in sync.

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{is_enabled, set_enabled};

#[cfg(not(windows))]
mod xdg;
#[cfg(not(windows))]
pub use xdg::{is_enabled, set_enabled};
