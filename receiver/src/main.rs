//! Pocket Webcam desktop receiver: finds a phone running the Pocket Webcam app on the
//! local network and exposes its camera as a virtual webcam.

use std::path::Path;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{bail, Result};
use clap::{Parser, ValueEnum};
use gstreamer as gst;
use log::{info, warn};

use pocketwebcam_receiver::audio::VirtualMic;
use pocketwebcam_receiver::effects::{Background, Effects, EffectsControl, Look};
use pocketwebcam_receiver::discovery::Phone;
use pocketwebcam_receiver::pairing::{self, Secret};
use pocketwebcam_receiver::session::{self, CodePrompt, Current, NotPaired, Options, Pairing, PairingFailed, Trust};
use pocketwebcam_receiver::video::{Output, Sink};
use pocketwebcam_receiver::{console, discovery, protocol};

#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// List phones on the network and exit.
    #[arg(long)]
    list: bool,

    /// Connect to this address instead of discovering (IP or IP:PORT).
    #[arg(long)]
    host: Option<String>,

    /// Only connect to a phone whose name contains this text.
    #[arg(long)]
    phone: Option<String>,

    /// Where the video goes.
    #[arg(long, value_enum, default_value_t = OutputKind::V4l2)]
    output: OutputKind,

    /// v4l2loopback device for `--output v4l2`.
    #[arg(long, default_value = "/dev/video10")]
    device: String,

    /// Virtual camera resolution. Phone video is scaled (letterboxed) to fit.
    #[arg(long, default_value = "1920x1080", value_parser = parse_size)]
    size: (u32, u32),

    /// Frame rate the virtual camera advertises to apps.
    #[arg(long, default_value_t = 30)]
    fps: u32,

    /// Decode on the GPU via VA-API (falls back to software if unavailable).
    #[arg(long)]
    hw_decode: bool,

    /// Colour look for the virtual camera: none, warm, cool, bw, vivid, studio, fade.
    #[arg(long, default_value = "none", value_parser = Look::from_str)]
    look: Look,

    /// Background effect: off or blur (blurs what's behind you).
    #[arg(long, value_enum, default_value_t = BackgroundKind::Off)]
    background: BackgroundKind,

    /// Replace the background with a picture (overrides `--background`). A file path, or
    /// `builtin:<id>` for a built-in one: studio, warm-wall, dusk, ocean, mint.
    #[arg(long, value_name = "PATH")]
    background_image: Option<String>,

    /// Pair a phone with this computer: shows a QR code to scan with the Pocket Webcam app.
    /// With `--host` or `--phone`, pairs that phone by comparing a 6-digit code instead.
    #[arg(long)]
    pair: bool,

    /// Don't turn on the phone's microphone or create "Pocket Webcam Microphone".
    #[arg(long)]
    no_mic: bool,

    /// Windows: remove the "Pocket Webcam" camera from Windows' camera list and exit (the
    /// uninstaller runs this).
    #[cfg(windows)]
    #[arg(long)]
    remove_camera: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum OutputKind {
    /// Virtual webcam (v4l2loopback).
    V4l2,
    /// Preview window, for testing without v4l2loopback.
    Window,
    /// Discard video, for headless testing.
    Null,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum BackgroundKind {
    /// Background left as it is.
    Off,
    /// Background blurred.
    Blur,
}

/// Turns `--background` / `--background-image` into a [`Background`]. An image wins over
/// `--background`; a file path is made absolute and must point to an existing file.
fn background_from_args(kind: BackgroundKind, image: Option<&str>) -> Result<Background> {
    #[cfg(not(feature = "background"))]
    {
        if kind != BackgroundKind::Off || image.is_some() {
            bail!(
                "this pocketwebcam was built without background support; rebuild without --no-default-features (or add --features background)"
            );
        }
        Ok(Background::Off)
    }
    #[cfg(feature = "background")]
    {
        use pocketwebcam_receiver::background::builtin::BUILTINS;
        use pocketwebcam_receiver::effects::BUILTIN_PREFIX;

        if let Some(image) = image {
            if let Some(id) = image.strip_prefix(BUILTIN_PREFIX) {
                if !BUILTINS.iter().any(|b| b.id == id) {
                    let ids: Vec<&str> = BUILTINS.iter().map(|b| b.id).collect();
                    bail!("unknown built-in background {id:?}; pick one of: {}", ids.join(", "));
                }
                return Ok(Background::Image { path: image.to_string() });
            }
            let path = std::path::absolute(image)
                .map_err(|e| anyhow::anyhow!("background image {image:?}: {e}"))?;
            if !path.is_file() {
                bail!("background image {} does not exist or is not a file", path.display());
            }
            let Some(path) = path.to_str() else {
                bail!("background image path {} is not valid UTF-8", path.display());
            };
            return Ok(Background::Image { path: path.to_string() });
        }
        Ok(match kind {
            BackgroundKind::Off => Background::Off,
            BackgroundKind::Blur => Background::Blur { strength: Background::DEFAULT_BLUR },
        })
    }
}

fn parse_size(s: &str) -> Result<(u32, u32), String> {
    console::parse_resolution(s).map_err(|e| e.to_string())
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_target(false)
        .init();
    let mut args = Args::parse();
    #[cfg(windows)]
    if args.remove_camera {
        return pocketwebcam_receiver::wincam::remove_camera();
    }
    pocketwebcam_receiver::paths::use_bundled_gstreamer();
    gst::init()?;

    let trust = Arc::new(Trust::load()?);
    let is_paired = |p: &Phone| p.fingerprint.is_some_and(|fp| trust.paired.contains(&fp));

    if args.list {
        let phones = discovery::browse(Duration::from_secs(3))?;
        if phones.is_empty() {
            println!("No phones found. Is the Pocket Webcam app open and on the same Wi-Fi?");
        }
        for p in phones {
            let status = if !p.compatible() {
                "app too old: update it"
            } else if is_paired(&p) {
                "paired"
            } else {
                "not paired: run with --pair"
            };
            println!("{}  {}  ({status})", p.name, p.addr);
        }
        return Ok(());
    }

    let sink = match args.output {
        OutputKind::V4l2 => {
            if !Path::new(&args.device).exists() {
                bail!(
                    "{} does not exist. Load v4l2loopback first:\n  {}",
                    args.device,
                    pocketwebcam_receiver::paths::loopback_setup_command()
                );
            }
            Sink::V4l2(args.device.clone())
        }
        OutputKind::Window => Sink::Window,
        OutputKind::Null => Sink::Null,
    };
    let (width, height) = args.size;
    let background = background_from_args(args.background, args.background_image.as_deref())?;
    let effects = Arc::new(EffectsControl::new(Effects { look: args.look, background, ..Default::default() }));
    let output = Output::new(&sink, width, height, args.fps, None, effects)?;
    match &sink {
        Sink::V4l2(dev) => info!("virtual camera ready on {dev} ({width}x{height})"),
        _ => info!("output ready ({width}x{height})"),
    }

    let current: Current = Arc::new(Mutex::new(None));
    // While pairing, stdin answers the code question instead.
    if !args.pair {
        console::spawn(current.clone());
        info!("type `help` for commands");
    }

    let mic = if args.no_mic {
        None
    } else {
        VirtualMic::create().map(Arc::new).map_err(|e| warn!("no virtual microphone: {e:#}")).ok()
    };
    let mut opts = Options {
        hw_decode: args.hw_decode,
        recorder: None,
        mic,
        mic_auto: !args.no_mic,
        on_hello: None,
        trust: trust.clone(),
        pairing: Pairing::None,
        phone_name: String::new(),
        phone_id: None,
        on_paired: None,
    };
    let qr_secret = (args.pair && args.host.is_none() && args.phone.is_none()).then(|| {
        let secret = Secret::random();
        let url = pairing::qr_url(&trust.me, &secret, &session::hostname());
        show_qr(&url);
        log::debug!("pairing link (for tools/fake_phone.py --scan): {url}");
        secret
    });
    let mut searching_logged = false;
    let mut unpaired_logged = std::collections::BTreeSet::new();
    loop {
        let (addr, name, id) = match &args.host {
            Some(host) => {
                let addr = discovery::parse_host(host, protocol::DEFAULT_PORT)?;
                (addr, addr.to_string(), None)
            }
            None => {
                if !searching_logged {
                    info!("searching for phones on the network...");
                    searching_logged = true;
                }
                let phones = discovery::browse(Duration::from_secs(3))?;
                let wanted = |p: &&Phone| {
                    args.phone.as_ref().is_none_or(|f| p.name.to_lowercase().contains(&f.to_lowercase()))
                };
                let found = match &qr_secret {
                    Some(secret) => phones.iter().find(|p| p.pair_tag.as_deref() == Some(secret.tag().as_str())),
                    None if args.pair => phones.iter().filter(wanted).find(|p| p.compatible()),
                    None => phones.iter().filter(wanted).find(|p| p.compatible() && is_paired(p)),
                };
                let Some(phone) = found else {
                    for p in phones.iter().filter(wanted) {
                        if unpaired_logged.insert(p.name.clone()) {
                            if !p.compatible() {
                                warn!("{} has an older Pocket Webcam app; update it on the phone", p.name);
                            } else if qr_secret.is_none() && !is_paired(p) {
                                warn!("{} isn't paired with this computer; run `pocketwebcam --pair`", p.name);
                            }
                        }
                    }
                    continue;
                };
                info!("found {} at {}", phone.name, phone.addr);
                (phone.addr, phone.name.clone(), phone.id.clone())
            }
        };

        let prompt = Arc::new(CodePrompt::default());
        opts.pairing = match &qr_secret {
            _ if !args.pair => Pairing::None,
            Some(secret) => Pairing::Qr(*secret),
            None => {
                ask_for_code(prompt.clone());
                Pairing::Code(prompt.clone())
            }
        };
        (opts.phone_name, opts.phone_id) = (name, id);
        match session::run(addr, &output, &opts, &current) {
            Ok(()) => info!("disconnected"),
            Err(e) if e.downcast_ref::<PairingFailed>().is_some() => {
                warn!("{e}");
                std::process::exit(1);
            }
            Err(e) if e.downcast_ref::<NotPaired>().is_some() => {
                warn!("{e}; run `pocketwebcam --pair` to pair it");
                if args.host.is_some() {
                    std::process::exit(1);
                }
            }
            Err(e) => warn!("{e:#}"),
        }
        prompt.answer(false); // ends a code question that never got its code
        // Paired now (or never asked to): from here on it's a normal paired phone.
        args.pair = false;
        searching_logged = false;
        thread::sleep(Duration::from_secs(2));
    }
}

/// Prints a QR code for the phone to scan, in the terminal (two rows of modules per line).
fn show_qr(url: &str) {
    let code = qrcode::QrCode::new(url).expect("a pairing link fits in a QR code");
    let width = code.width();
    let dark = |x: usize, y: usize| x < width && y < width && code[(x, y)] == qrcode::Color::Dark;
    // Drawn as light blocks on the terminal's background, with a quiet zone of 2: the
    // right way round on a dark terminal (the usual case). On a light terminal, use the
    // desktop app's QR code or pair with --host/--phone instead.
    println!();
    println!("Scan this with the Pocket Webcam app on your phone (Settings > Pair with a computer):");
    println!();
    let quiet = 2isize;
    let range = -quiet..width as isize + quiet;
    for y in range.clone().step_by(2) {
        let mut line = String::from("  ");
        for x in range.clone() {
            let at = |y: isize| x >= 0 && y >= 0 && dark(x as usize, y as usize);
            line.push(match (at(y), at(y + 1)) {
                (false, false) => '\u{2588}',
                (false, true) => '\u{2580}',
                (true, false) => '\u{2584}',
                (true, true) => ' ',
            });
        }
        println!("{line}");
    }
    println!();
    info!("waiting for a phone to scan the code (valid for {} minutes)", pairing::QR_VALID_FOR.as_secs() / 60);
}

/// Asks on the terminal whether the phone shows the same code.
fn ask_for_code(prompt: Arc<CodePrompt>) {
    thread::spawn(move || {
        let code = loop {
            let state = prompt.state();
            if state.finished {
                return;
            }
            if let Some(code) = state.code {
                break code;
            }
            thread::sleep(Duration::from_millis(50));
        };
        println!();
        println!("  Pairing code:  {}", pairing::spaced(&code));
        println!();
        println!("Does the phone show the same code? Tap Pair on the phone, then type y and Enter here.");
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        prompt.answer(line.trim().eq_ignore_ascii_case("y") || line.trim().eq_ignore_ascii_case("yes"));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file that is sure to exist while the tests run.
    const EXISTING: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");
    const MISSING: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/no-such-background.jpg");

    #[test]
    fn off_is_off() {
        assert_eq!(background_from_args(BackgroundKind::Off, None).unwrap(), Background::Off);
    }

    #[cfg(feature = "background")]
    #[test]
    fn blur_uses_default_strength() {
        assert_eq!(
            background_from_args(BackgroundKind::Blur, None).unwrap(),
            Background::Blur { strength: Background::DEFAULT_BLUR }
        );
    }

    #[cfg(feature = "background")]
    #[test]
    fn existing_image_file() {
        assert_eq!(
            background_from_args(BackgroundKind::Off, Some(EXISTING)).unwrap(),
            // Made absolute the same way, so Windows' `\` separators match too.
            Background::Image { path: std::path::absolute(EXISTING).unwrap().to_str().unwrap().to_string() }
        );
    }

    #[cfg(feature = "background")]
    #[test]
    fn relative_image_becomes_absolute() {
        // Tests run with the crate directory as the working directory.
        let Background::Image { path } = background_from_args(BackgroundKind::Off, Some("Cargo.toml")).unwrap()
        else {
            panic!("expected an image");
        };
        assert!(Path::new(&path).is_absolute());
        assert!(Path::new(&path).is_file());
    }

    #[cfg(feature = "background")]
    #[test]
    fn missing_image_errors() {
        let err = background_from_args(BackgroundKind::Off, Some(MISSING)).unwrap_err();
        assert!(err.to_string().contains("does not exist"), "{err}");
    }

    #[cfg(feature = "background")]
    #[test]
    fn directory_is_not_an_image() {
        assert!(background_from_args(BackgroundKind::Off, Some(env!("CARGO_MANIFEST_DIR"))).is_err());
    }

    #[cfg(feature = "background")]
    #[test]
    fn builtin_valid_and_invalid() {
        assert_eq!(
            background_from_args(BackgroundKind::Off, Some("builtin:ocean")).unwrap(),
            Background::Image { path: "builtin:ocean".to_string() }
        );
        let err = background_from_args(BackgroundKind::Off, Some("builtin:nope")).unwrap_err();
        assert!(err.to_string().contains("studio"), "{err}");
        assert!(background_from_args(BackgroundKind::Off, Some("builtin:")).is_err());
    }

    #[cfg(feature = "background")]
    #[test]
    fn image_overrides_blur() {
        assert_eq!(
            background_from_args(BackgroundKind::Blur, Some("builtin:studio")).unwrap(),
            Background::Image { path: "builtin:studio".to_string() }
        );
    }

    #[cfg(not(feature = "background"))]
    #[test]
    fn without_feature_anything_but_off_errors() {
        for (kind, image) in [
            (BackgroundKind::Blur, None),
            (BackgroundKind::Off, Some(EXISTING)),
            (BackgroundKind::Off, Some(MISSING)),
            (BackgroundKind::Blur, Some("builtin:studio")),
        ] {
            let err = background_from_args(kind, image).unwrap_err();
            assert!(err.to_string().contains("without background support"), "{err}");
        }
    }
}
