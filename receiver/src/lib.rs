//! Pocket Webcam desktop receiver: finds a phone running the Pocket Webcam app on the
//! local network and exposes its camera as a virtual webcam.
//!
//! Two front ends share this library: the terminal receiver (`pocketwebcam`) and the
//! desktop window (`pocketwebcam-app`).

pub mod audio;
pub mod autostart;
#[cfg(feature = "background")]
pub mod background;
pub mod capture;
pub mod console;
pub mod discovery;
pub mod effects;
pub mod engine;
pub mod logging;
#[cfg(target_os = "linux")]
pub mod loopback;
pub mod pairing;
pub mod paths;
pub mod protocol;
pub mod scenes;
pub mod session;
pub mod settings;
pub mod tls;
pub mod video;
#[cfg(windows)]
pub mod wincam;
