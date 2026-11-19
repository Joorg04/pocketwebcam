//! Video effects: looks (presets) and picture adjustments applied to the virtual camera.
//!
//! Every effect is expressed as three 256-entry lookup tables, one each for the Y, U and
//! V bytes of a YUY2 frame, so applying it is a single pass over the frame with no
//! allocation. The tables are built once per change by [`compile`] and shared with the
//! video thread through [`EffectsControl`].
//!
//! Also holds the small helpers the effects gallery uses to turn a raw frame into
//! thumbnails ([`downscale_yuy2`], [`yuy2_to_rgba`]).

use std::str::FromStr;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Deserializer, Serialize};

/// A preset look. Its adjustments are added on top of the user's sliders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Look {
    Warm,
    Cool,
    BlackWhite,
    Vivid,
    Studio,
    Fade,
    /// No look; an unknown name in a settings file also reads as this. (Serde needs
    /// the catch-all variant last; gallery order comes from [`Look::ALL`].)
    #[default]
    #[serde(other)]
    None,
}

impl Look {
    /// Every look, in gallery order.
    pub const ALL: [Look; 7] = [
        Look::None,
        Look::Warm,
        Look::Cool,
        Look::BlackWhite,
        Look::Vivid,
        Look::Studio,
        Look::Fade,
    ];

    /// Name shown in the app.
    pub fn label(self) -> &'static str {
        match self {
            Look::None => "Original",
            Look::Warm => "Warm",
            Look::Cool => "Cool",
            Look::BlackWhite => "B&W",
            Look::Vivid => "Vivid",
            Look::Studio => "Studio",
            Look::Fade => "Fade",
        }
    }

    /// Short name used on the command line (`--look`).
    pub fn name(self) -> &'static str {
        match self {
            Look::None => "none",
            Look::Warm => "warm",
            Look::Cool => "cool",
            Look::BlackWhite => "bw",
            Look::Vivid => "vivid",
            Look::Studio => "studio",
            Look::Fade => "fade",
        }
    }
}

impl FromStr for Look {
    type Err = String;

    /// Parses a look name, case-insensitively (`bw`, `black_white`, `original`, ...).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim().to_ascii_lowercase();
        let look = match s.as_str() {
            "none" | "original" | "off" => Look::None,
            "warm" => Look::Warm,
            "cool" => Look::Cool,
            "bw" | "b&w" | "black_white" | "black-white" | "blackwhite" | "mono" => {
                Look::BlackWhite
            }
            "vivid" => Look::Vivid,
            "studio" => Look::Studio,
            "fade" => Look::Fade,
            _ => {
                let names: Vec<&str> = Look::ALL.iter().map(|l| l.name()).collect();
                return Err(format!(
                    "unknown look '{s}' (choose from: {})",
                    names.join(", ")
                ));
            }
        };
        Ok(look)
    }
}

/// Prefix of an [`Background::Image`] path that names a built-in background instead of
/// a file (`"builtin:<id>"`).
pub const BUILTIN_PREFIX: &str = "builtin:";

/// What happens behind the person in the picture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum Background {
    /// Background left as it is.
    #[default]
    Off,
    /// Background blurred; `strength` runs from 0.0 (light) to 1.0 (strongest).
    Blur { strength: f32 },
    /// Background replaced by a picture: an absolute file path, or `"builtin:<id>"`
    /// (see [`BUILTIN_PREFIX`]).
    Image { path: String },
}

impl Background {
    /// Blur strength used when none (or NaN) is given.
    pub const DEFAULT_BLUR: f32 = 0.6;

    /// A copy that is safe to use: blur strength clamped to 0..=1 (NaN becomes
    /// [`Self::DEFAULT_BLUR`]); an image with an empty, overlong (> 4096 bytes) or
    /// NUL-containing path becomes [`Background::Off`].
    pub fn sanitized(&self) -> Background {
        match self {
            Background::Off => Background::Off,
            Background::Blur { strength } => Background::Blur {
                strength: if strength.is_nan() {
                    Self::DEFAULT_BLUR
                } else {
                    strength.clamp(0.0, 1.0)
                },
            },
            Background::Image { path } => {
                if path.trim().is_empty() || path.len() > 4096 || path.contains('\0') {
                    Background::Off
                } else {
                    Background::Image { path: path.clone() }
                }
            }
        }
    }

    pub fn is_off(&self) -> bool {
        matches!(self, Background::Off)
    }
}

/// Reads a background, giving [`Background::Off`] for anything unrecognised so the rest
/// of the effects survive.
fn lenient_background<'de, D: Deserializer<'de>>(d: D) -> Result<Background, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value::<Background>(value).unwrap_or_default().sanitized())
}

/// The effects the user picked. Sliders run from -1.0 to 1.0, with 0 meaning unchanged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Effects {
    pub look: Look,
    pub brightness: f32,
    pub contrast: f32,
    pub saturation: f32,
    /// Negative is cooler (bluer), positive is warmer (more orange).
    pub temperature: f32,
    /// Background blur or replacement. A broken value reads as [`Background::Off`].
    #[serde(deserialize_with = "lenient_background")]
    pub background: Background,
}

impl Default for Effects {
    fn default() -> Self {
        Effects {
            look: Look::None,
            brightness: 0.0,
            contrast: 0.0,
            saturation: 0.0,
            temperature: 0.0,
            background: Background::Off,
        }
    }
}

/// Clamps a slider to -1..=1; NaN becomes 0.
fn slider(v: f32) -> f32 {
    if v.is_nan() {
        0.0
    } else {
        v.clamp(-1.0, 1.0)
    }
}

impl Effects {
    /// A copy with every slider clamped to its range and NaN replaced by 0, and the
    /// background sanitized (see [`Background::sanitized`]).
    pub fn sanitized(&self) -> Effects {
        Effects {
            look: self.look,
            brightness: slider(self.brightness),
            contrast: slider(self.contrast),
            saturation: slider(self.saturation),
            temperature: slider(self.temperature),
            background: self.background.sanitized(),
        }
    }

    /// True when the effects leave the picture unchanged: no colour change and no
    /// background effect.
    pub fn is_identity(&self) -> bool {
        compile(self).identity && self.background.sanitized().is_off()
    }

    /// True when no look is picked and every slider is at zero (what Reset goes back to).
    /// The background is ignored: Reset in the Effects card is about looks and sliders.
    pub fn is_default(&self) -> bool {
        self.look == Look::None
            && [self.brightness, self.contrast, self.saturation, self.temperature].iter().all(|&v| v == 0.0)
    }
}

/// Effects turned into per-byte lookup tables for YUY2, ready for [`apply_yuy2`].
#[derive(Debug, Clone)]
pub struct Compiled {
    y: [u8; 256],
    u: [u8; 256],
    v: [u8; 256],
    identity: bool,
    background: Background,
}

impl Compiled {
    /// True when applying these tables would change nothing (frames are left untouched).
    /// Only about the colour tables; the background is checked separately through
    /// [`Compiled::background`].
    pub fn identity(&self) -> bool {
        self.identity
    }

    /// The (sanitized) background effect to apply.
    pub fn background(&self) -> &Background {
        &self.background
    }
}

/// Clamps and rounds a table value into a legal limited-range byte.
fn to_byte(v: f32, lo: u8, hi: u8) -> u8 {
    v.round().clamp(lo as f32, hi as f32) as u8
}

/// Builds the lookup tables for `e`: the look's adjustments are added to the sliders,
/// clamped, then mapped onto limited-range Y (16..=235) and U/V (16..=240).
pub fn compile(e: &Effects) -> Compiled {
    let e = e.sanitized();
    let (mut b, mut c, mut s, mut t) = (e.brightness, e.contrast, e.saturation, e.temperature);
    // How much colour survives; only B&W changes it.
    let mut chroma = 1.0;
    match e.look {
        Look::None => {}
        Look::Warm => {
            t += 0.35;
            s += 0.05;
        }
        Look::Cool => t -= 0.35,
        Look::BlackWhite => {
            chroma = 0.0;
            s = 0.0;
            t = 0.0;
            c += 0.1;
        }
        Look::Vivid => {
            s += 0.4;
            c += 0.15;
        }
        Look::Studio => {
            c += 0.12;
            b += 0.04;
            s += 0.08;
        }
        Look::Fade => {
            c -= 0.25;
            b += 0.06;
            s -= 0.3;
        }
    }
    let (b, c, s, t) = (slider(b), slider(c), slider(s), slider(t));

    let identity = b == 0.0 && c == 0.0 && s == 0.0 && t == 0.0 && chroma == 1.0;
    if identity {
        let mut same = [0u8; 256];
        for (i, out) in same.iter_mut().enumerate() {
            *out = i as u8;
        }
        return Compiled {
            y: same,
            u: same,
            v: same,
            identity: true,
            background: e.background,
        };
    }

    // Contrast pivots on mid-grey of the limited range, (16 + 235) / 2.
    const PIVOT: f32 = 125.5;
    let k = 1.0 + c;
    let lift = b * 40.0;
    let gain = chroma * (1.0 + s);
    let shift = t * 14.0;

    let mut y = [0u8; 256];
    let mut u = [0u8; 256];
    let mut v = [0u8; 256];
    for i in 0..256 {
        let x = i as f32;
        y[i] = to_byte((x - PIVOT) * k + PIVOT + lift, 16, 235);
        let chroma_val = (x - 128.0) * gain;
        u[i] = to_byte(128.0 + chroma_val - shift, 16, 240);
        v[i] = to_byte(128.0 + chroma_val + shift, 16, 240);
    }
    Compiled {
        y,
        u,
        v,
        identity: false,
        background: e.background,
    }
}

/// Applies `c` to a YUY2 buffer in place (byte order Y0 U Y1 V). A trailing partial
/// macropixel is left alone.
pub fn apply_yuy2(c: &Compiled, data: &mut [u8]) {
    if c.identity {
        return;
    }
    for px in data.as_chunks_mut::<4>().0 {
        px[0] = c.y[px[0] as usize];
        px[1] = c.u[px[1] as usize];
        px[2] = c.y[px[2] as usize];
        px[3] = c.v[px[3] as usize];
    }
}

/// The current effects, shared between the app and the video thread. Changing them
/// rebuilds the tables once; the video thread only clones an `Arc` per frame.
pub struct EffectsControl {
    state: Mutex<(Effects, Arc<Compiled>)>,
}

impl EffectsControl {
    pub fn new(effects: Effects) -> Self {
        let effects = effects.sanitized();
        let compiled = Arc::new(compile(&effects));
        EffectsControl {
            state: Mutex::new((effects, compiled)),
        }
    }

    /// Replaces the effects (after sanitizing them) and rebuilds the tables.
    pub fn set(&self, effects: Effects) {
        let effects = effects.sanitized();
        let compiled = Arc::new(compile(&effects));
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = (effects, compiled);
    }

    pub fn get(&self) -> Effects {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .0
            .clone()
    }

    /// The tables for the current effects.
    pub fn compiled(&self) -> Arc<Compiled> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .1
            .clone()
    }
}

impl Default for EffectsControl {
    fn default() -> Self {
        EffectsControl::new(Effects::default())
    }
}

/// A black YUY2 buffer of `w` x `h` pixels.
fn black_yuy2(w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 2];
    for px in out.as_chunks_mut::<4>().0 {
        px.copy_from_slice(&[16, 128, 16, 128]);
    }
    if out.len() % 4 == 2 {
        let n = out.len();
        out[n - 2..].copy_from_slice(&[16, 128]);
    }
    out
}

/// Scales a `sw` x `sh` YUY2 frame down to `dw` x `dh` by picking the nearest source
/// macropixel. `dw` should be even. Always returns `dw * dh * 2` bytes; a source buffer
/// that is too short gives a black frame.
pub fn downscale_yuy2(src: &[u8], sw: usize, sh: usize, dw: usize, dh: usize) -> Vec<u8> {
    let mut out = black_yuy2(dw, dh);
    let (src_pairs, dst_pairs) = (sw / 2, dw / 2);
    if src_pairs == 0 || sh == 0 || dst_pairs == 0 || src.len() < sw * sh * 2 {
        return out;
    }
    for dy in 0..dh {
        let sy = dy * sh / dh;
        for dx in 0..dst_pairs {
            let sx = dx * src_pairs / dst_pairs;
            let s = sy * sw * 2 + sx * 4;
            let d = dy * dw * 2 + dx * 4;
            out[d..d + 4].copy_from_slice(&src[s..s + 4]);
        }
    }
    out
}

/// Converts a `w` x `h` YUY2 frame to RGBA (BT.601, limited range, opaque). Always
/// returns `w * h * 4` bytes; pixels missing from a short source come out black.
pub fn yuy2_to_rgba(src: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 4];
    for px in out.as_chunks_mut::<4>().0 {
        px[3] = 255;
    }
    for row in 0..h {
        for x in 0..w {
            let base = row * w * 2 + (x / 2) * 4;
            if base + 3 >= src.len() {
                continue;
            }
            let y = 1.164 * (src[base + (x & 1) * 2] as f32 - 16.0);
            let u = src[base + 1] as f32 - 128.0;
            let v = src[base + 3] as f32 - 128.0;
            let o = (row * w + x) * 4;
            out[o] = (y + 1.596 * v).round().clamp(0.0, 255.0) as u8;
            out[o + 1] = (y - 0.392 * u - 0.813 * v).round().clamp(0.0, 255.0) as u8;
            out[o + 2] = (y + 2.017 * u).round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame with every possible byte value in every position.
    fn all_bytes() -> Vec<u8> {
        (0..=255u8).cycle().take(256 * 4 * 4).collect()
    }

    fn with_look(look: Look) -> Effects {
        Effects {
            look,
            ..Effects::default()
        }
    }

    #[test]
    fn identity_leaves_bytes_unchanged() {
        let c = compile(&Effects::default());
        assert!(c.identity());
        assert!(Effects::default().is_identity());
        let before = all_bytes();
        let mut frame = before.clone();
        apply_yuy2(&c, &mut frame);
        assert_eq!(frame, before);
    }

    #[test]
    fn black_white_removes_colour() {
        let c = compile(&Effects {
            saturation: 1.0,
            temperature: 1.0,
            ..with_look(Look::BlackWhite)
        });
        let mut frame = all_bytes();
        apply_yuy2(&c, &mut frame);
        for px in frame.as_chunks::<4>().0 {
            assert_eq!(px[1], 128);
            assert_eq!(px[3], 128);
        }
    }

    #[test]
    fn warm_raises_v_and_lowers_u() {
        let c = compile(&with_look(Look::Warm));
        assert!(c.v[128] > 128);
        assert!(c.u[128] < 128);
        let cool = compile(&with_look(Look::Cool));
        assert!(cool.v[128] < 128);
        assert!(cool.u[128] > 128);
    }

    #[test]
    fn brightness_raises_y_within_range() {
        let c = compile(&Effects {
            brightness: 1.0,
            ..Effects::default()
        });
        for (i, &y) in c.y.iter().enumerate().take(236).skip(16) {
            assert!(y as usize >= i);
            assert!(y <= 235);
        }
        assert!(c.y[100] > 100);
    }

    #[test]
    fn tables_stay_in_legal_range() {
        let extremes = [-1.0, -0.5, 0.0, 0.5, 1.0, f32::NAN, 5.0];
        for look in Look::ALL {
            for &b in &extremes {
                for &k in &extremes {
                    for &s in &extremes {
                        let e = Effects {
                            look,
                            brightness: b,
                            contrast: k,
                            saturation: s,
                            temperature: -b,
                            ..Effects::default()
                        };
                        let c = compile(&e);
                        if c.identity() {
                            continue;
                        }
                        for i in 0..256 {
                            assert!((16..=235).contains(&c.y[i]));
                            assert!((16..=240).contains(&c.u[i]));
                            assert!((16..=240).contains(&c.v[i]));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn sanitized_clamps_and_drops_nan() {
        let e = Effects {
            brightness: f32::NAN,
            contrast: 3.0,
            saturation: -3.0,
            ..Effects::default()
        };
        let s = e.sanitized();
        assert_eq!(s.brightness, 0.0);
        assert_eq!(s.contrast, 1.0);
        assert_eq!(s.saturation, -1.0);
    }

    #[test]
    fn is_default_only_without_look_or_sliders() {
        assert!(Effects::default().is_default());
        assert!(!with_look(Look::Warm).is_default());
        assert!(!Effects { temperature: 0.1, ..Effects::default() }.is_default());
        // A look plus sliders that cancel it out still counts as changed.
        let e = Effects { look: Look::Vivid, saturation: -0.5, ..Effects::default() };
        assert!(!e.is_default());
    }

    #[test]
    fn effects_json_round_trip() {
        let e = Effects {
            look: Look::BlackWhite,
            brightness: 0.25,
            contrast: -0.5,
            saturation: 0.1,
            temperature: 0.3,
            background: Background::Blur { strength: 0.4 },
        };
        let json = serde_json::to_string(&e).unwrap();
        assert_eq!(serde_json::from_str::<Effects>(&json).unwrap(), e);
    }

    #[test]
    fn background_json_round_trip() {
        for bg in [
            Background::Off,
            Background::Blur { strength: 0.25 },
            Background::Image { path: "/home/me/Pictures/beach.jpg".into() },
            Background::Image { path: format!("{BUILTIN_PREFIX}office") },
        ] {
            let e = Effects { background: bg.clone(), ..Effects::default() };
            let json = serde_json::to_string(&e).unwrap();
            assert_eq!(serde_json::from_str::<Effects>(&json).unwrap().background, bg);
        }
        let json = serde_json::to_string(&Background::Blur { strength: 0.5 }).unwrap();
        assert_eq!(json, r#"{"mode":"blur","strength":0.5}"#);
    }

    #[test]
    fn garbage_background_reads_as_off_but_keeps_look() {
        for bg in [
            r#"{"mode":"zzz"}"#,
            r#""blur""#,
            r#"{"mode":"blur"}"#,
            r#"{"mode":"blur","strength":"lots"}"#,
            r#"{"mode":"image"}"#,
            r#"{"mode":"image","path":42}"#,
            r#"{"mode":"image","path":"  "}"#,
            "null",
            "7",
        ] {
            let json = format!(r#"{{"look":"warm","brightness":0.5,"background":{bg}}}"#);
            let e: Effects = serde_json::from_str(&json).unwrap();
            assert_eq!(e.look, Look::Warm, "{bg}");
            assert_eq!(e.brightness, 0.5, "{bg}");
            assert_eq!(e.background, Background::Off, "{bg}");
        }
    }

    #[test]
    fn background_sanitized_fixes_strength_and_paths() {
        let s = |strength| Background::Blur { strength }.sanitized();
        assert_eq!(s(f32::NAN), Background::Blur { strength: Background::DEFAULT_BLUR });
        assert_eq!(s(3.0), Background::Blur { strength: 1.0 });
        assert_eq!(s(-1.0), Background::Blur { strength: 0.0 });
        assert_eq!(s(0.3), Background::Blur { strength: 0.3 });
        // Out-of-range strength in a file is clamped on load.
        let e: Effects = serde_json::from_str(r#"{"background":{"mode":"blur","strength":9}}"#).unwrap();
        assert_eq!(e.background, Background::Blur { strength: 1.0 });

        let img = |path: &str| Background::Image { path: path.into() }.sanitized();
        assert_eq!(img(""), Background::Off);
        assert_eq!(img(" \t"), Background::Off);
        assert_eq!(img("/tmp/a\0b.png"), Background::Off);
        assert_eq!(img(&"x".repeat(4097)), Background::Off);
        assert_eq!(img("/tmp/a.png"), Background::Image { path: "/tmp/a.png".into() });
        assert!(Background::Off.sanitized().is_off());
        assert!(!s(0.5).is_off());

        let e = Effects { background: Background::Image { path: String::new() }, ..Effects::default() };
        assert!(e.sanitized().background.is_off());
    }

    #[test]
    fn old_json_with_blur_fields_loads() {
        let e: Effects =
            serde_json::from_str(r#"{"look":"cool","contrast":0.2,"blur":true,"blur_strength":0.4}"#).unwrap();
        assert_eq!(e.look, Look::Cool);
        assert_eq!(e.contrast, 0.2);
        assert_eq!(e.background, Background::Off);
    }

    #[test]
    fn background_counts_for_identity_but_not_default() {
        let e = Effects { background: Background::Blur { strength: 0.5 }, ..Effects::default() };
        assert!(e.is_default());
        assert!(!e.is_identity());
        let c = compile(&e);
        // The colour tables alone change nothing.
        assert!(c.identity());
        assert_eq!(c.background(), &Background::Blur { strength: 0.5 });
        // Compiled carries the sanitized background.
        let e = Effects { background: Background::Blur { strength: f32::NAN }, ..with_look(Look::Warm) };
        assert_eq!(compile(&e).background(), &Background::Blur { strength: Background::DEFAULT_BLUR });
        assert!(compile(&Effects::default()).background().is_off());
    }

    #[test]
    fn unknown_look_reads_as_none() {
        let e: Effects = serde_json::from_str(r#"{"look":"future_thing"}"#).unwrap();
        assert_eq!(e.look, Look::None);
    }

    #[test]
    fn empty_json_gives_defaults() {
        let e: Effects = serde_json::from_str("{}").unwrap();
        assert_eq!(e, Effects::default());
    }

    #[test]
    fn downscale_has_expected_length() {
        let (sw, sh) = (64, 36);
        let src = vec![100u8; sw * sh * 2];
        assert_eq!(downscale_yuy2(&src, sw, sh, 16, 9).len(), 16 * 9 * 2);
        // A short source must not panic.
        assert_eq!(downscale_yuy2(&src[..10], sw, sh, 16, 9).len(), 16 * 9 * 2);
    }

    #[test]
    fn rgba_has_expected_length() {
        let src = black_yuy2(16, 9);
        let rgba = yuy2_to_rgba(&src, 16, 9);
        assert_eq!(rgba.len(), 16 * 9 * 4);
        assert_eq!(&rgba[..4], &[0, 0, 0, 255]);
        assert_eq!(yuy2_to_rgba(&src[..7], 16, 9).len(), 16 * 9 * 4);
    }

    #[test]
    fn look_parses_from_str() {
        assert_eq!("bw".parse::<Look>().unwrap(), Look::BlackWhite);
        assert_eq!("Black_White".parse::<Look>().unwrap(), Look::BlackWhite);
        assert_eq!("WARM".parse::<Look>().unwrap(), Look::Warm);
        for look in Look::ALL {
            assert_eq!(look.name().parse::<Look>().unwrap(), look);
        }
        assert!("sepia".parse::<Look>().is_err());
    }
}
