//! Wire format from PROTOCOL.md: 1-byte type, 4-byte big-endian length, payload.

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

/// v2 runs everything inside TLS between paired devices; v1 (plain TCP) is gone.
pub const PROTOCOL_VERSION: u32 = 2;
pub const SERVICE_TYPE: &str = "_pocketwebcam._tcp.local.";
pub const DEFAULT_PORT: u16 = 7310;
const MAX_PAYLOAD: u32 = 16 * 1024 * 1024;

pub const HELLO: u8 = 0x01;
pub const CAPABILITIES: u8 = 0x02;
pub const SET_CONFIG: u8 = 0x03;
pub const STATE: u8 = 0x04;
pub const STREAM_FORMAT: u8 = 0x10;
pub const VIDEO_FRAME: u8 = 0x11;
pub const KEYFRAME_REQ: u8 = 0x12;
pub const AUDIO_FORMAT: u8 = 0x13;
pub const AUDIO_FRAME: u8 = 0x14;
pub const PAIR_REQUEST: u8 = 0x30;
pub const PAIR_PROOF: u8 = 0x31;
pub const PAIR_RESULT: u8 = 0x32;
pub const PING: u8 = 0x20;
pub const PONG: u8 = 0x21;
pub const ERROR: u8 = 0x7F;

pub struct Message {
    pub kind: u8,
    pub payload: Vec<u8>,
}

pub fn read_message(r: &mut impl Read) -> io::Result<Message> {
    let mut header = [0u8; 5];
    r.read_exact(&mut header)?;
    let len = u32::from_be_bytes([header[1], header[2], header[3], header[4]]);
    if len > MAX_PAYLOAD {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("payload too large: {len} bytes"),
        ));
    }
    let mut payload = vec![0u8; len as usize];
    r.read_exact(&mut payload)?;
    Ok(Message { kind: header[0], payload })
}

pub fn write_message(w: &mut impl Write, kind: u8, payload: &[u8]) -> io::Result<()> {
    let mut buf = Vec::with_capacity(5 + payload.len());
    buf.push(kind);
    buf.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    buf.extend_from_slice(payload);
    w.write_all(&buf)
}

pub fn write_json(w: &mut impl Write, kind: u8, value: &impl Serialize) -> io::Result<()> {
    write_message(w, kind, &serde_json::to_vec(value)?)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    pub protocol: u32,
    pub name: String,
    #[serde(default)]
    pub app: String,
    #[serde(default)]
    pub app_version: String,
    #[serde(default)]
    pub platform: String,
    /// A random identifier the phone app made up when it was first opened. It stays the
    /// same when the phone is renamed, so a remembered phone is recognised by it.
    /// Missing from phone apps older than the identifier, and from the desktop's own HELLO.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[allow(dead_code)] // full wire format; not every field is used yet
pub struct Capabilities {
    pub cameras: Vec<Camera>,
    #[serde(default)]
    pub codecs: Vec<String>,
    pub bitrate: Option<Range<u64>>,
    /// Missing on phones that can't send audio.
    pub audio: Option<AudioCapabilities>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)] // full wire format; not every field is used yet
pub struct AudioCapabilities {
    #[serde(default)]
    pub codecs: Vec<String>,
    #[serde(default)]
    pub sample_rates: Vec<u32>,
    #[serde(default)]
    pub channels: Vec<u32>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)] // full wire format; not every field is used yet
pub struct Camera {
    pub id: String,
    pub name: String,
    pub position: String,
    pub lens: String,
    pub zoom: Option<Range<f64>>,
    pub formats: Vec<Format>,
    #[serde(default)]
    pub features: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Format {
    pub width: u32,
    pub height: u32,
    pub fps: Vec<u32>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)] // full wire format; not every field is used yet
pub struct Range<T> {
    pub min: T,
    pub max: T,
}

/// SET_CONFIG: every field optional, only present keys change.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigUpdate {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub camera: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fps: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bitrate: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zoom: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub torch: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stabilization: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mirror: Option<bool>,
    /// Phone microphone on or off.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)] // full wire format; not every field is used yet
pub struct State {
    pub camera: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub codec: String,
    pub bitrate: u64,
    #[serde(default)]
    pub zoom: f64,
    #[serde(default)]
    pub torch: bool,
    #[serde(default)]
    pub stabilization: bool,
    #[serde(default)]
    pub mirror: bool,
    #[serde(default)]
    pub thermal: String,
    pub battery: Option<f64>,
    #[serde(default)]
    pub charging: bool,
    #[serde(default)]
    pub audio: bool,
    /// `granted`, `denied` or `undetermined`; empty from phones without audio.
    #[serde(default)]
    pub mic_permission: String,
    /// Why the phone's camera has stopped for now (`background`, `inUse`, `call`,
    /// `overheated`, `unknown`); empty while it runs.
    #[serde(default)]
    pub camera_paused: String,
}

impl State {
    /// What to tell the user while the phone's camera is paused; `None` while it runs.
    pub fn camera_paused_hint(&self) -> Option<&'static str> {
        Some(match self.camera_paused.as_str() {
            "" => return None,
            "background" => "Pocket Webcam isn't open on the phone's screen",
            "inUse" => "Another app on the phone is using the camera",
            "call" => "The phone is in a call, so its camera is paused",
            "overheated" => "The phone paused its camera because it's too hot",
            _ => "The phone paused its camera",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct StreamFormat {
    pub codec: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    #[serde(default)]
    pub rotation: u32,
    /// Flip the upright image horizontally (added for the Android app; absent = false).
    #[serde(default)]
    pub mirror: bool,
}

/// AUDIO_FORMAT: shape of the AUDIO_FRAME samples that follow.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioFormat {
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u32,
}

impl AudioFormat {
    /// The only format v1 receivers play.
    pub fn is_supported(&self) -> bool {
        self.codec == "pcm_s16le" && self.sample_rate == 48_000 && self.channels == 1
    }
}

/// PAIR_REQUEST (desktop to phone): the first message on a connection to a phone that
/// hasn't been paired with this computer yet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PairRequest {
    /// `qr` (the phone scanned this computer's code) or `code` (compare 6 digits).
    pub mode: String,
    /// This computer's name, for the phone's list of paired computers.
    pub name: String,
}

/// PAIR_PROOF (phone to desktop, QR pairing): HMAC that shows the phone scanned the code.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PairProof {
    pub proof: String,
}

/// PAIR_RESULT (both ways): whether this side accepts the pairing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PairResult {
    pub accepted: bool,
    /// Why not: `declined`, `timeout`, `wrong_proof`, `wrong_computer`, `no_code`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ErrorMsg {
    pub code: String,
    #[serde(default)]
    pub message: String,
}

pub struct VideoFrame<'a> {
    pub keyframe: bool,
    pub pts_us: u64,
    pub data: &'a [u8],
}

pub fn parse_video_frame(payload: &[u8]) -> Option<VideoFrame<'_>> {
    if payload.len() < 9 {
        return None;
    }
    let pts = u64::from_be_bytes(payload[1..9].try_into().ok()?);
    Some(VideoFrame { keyframe: payload[0] & 1 != 0, pts_us: pts, data: &payload[9..] })
}

pub struct AudioFrame<'a> {
    pub pts_us: u64,
    /// Interleaved little-endian samples.
    pub samples: &'a [u8],
}

pub fn parse_audio_frame(payload: &[u8]) -> Option<AudioFrame<'_>> {
    if payload.len() < 8 || !(payload.len() - 8).is_multiple_of(2) {
        return None;
    }
    let pts = u64::from_be_bytes(payload[..8].try_into().ok()?);
    Some(AudioFrame { pts_us: pts, samples: &payload[8..] })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let mut buf = Vec::new();
        write_message(&mut buf, PING, b"{\"t\":1}").unwrap();
        let msg = read_message(&mut buf.as_slice()).unwrap();
        assert_eq!(msg.kind, PING);
        assert_eq!(msg.payload, b"{\"t\":1}");
    }

    #[test]
    fn rejects_oversized() {
        let header = [VIDEO_FRAME, 0xFF, 0xFF, 0xFF, 0xFF];
        assert!(read_message(&mut header.as_slice()).is_err());
    }

    #[test]
    fn video_frame_header() {
        let mut p = vec![1u8];
        p.extend_from_slice(&42u64.to_be_bytes());
        p.extend_from_slice(&[0, 0, 0, 1, 0x65]);
        let f = parse_video_frame(&p).unwrap();
        assert!(f.keyframe);
        assert_eq!(f.pts_us, 42);
        assert_eq!(f.data, &[0, 0, 0, 1, 0x65]);
    }

    #[test]
    fn audio_frame_header() {
        let mut p = 7u64.to_be_bytes().to_vec();
        p.extend_from_slice(&[1, 0, 0xFF, 0xFF]);
        let f = parse_audio_frame(&p).unwrap();
        assert_eq!(f.pts_us, 7);
        assert_eq!(f.samples, &[1, 0, 0xFF, 0xFF]);
        assert!(parse_audio_frame(&p[..9]).is_none(), "odd sample bytes");
        assert!(parse_audio_frame(&p[..7]).is_none(), "short header");
    }

    #[test]
    fn audio_format_and_state_keys() {
        let f: AudioFormat =
            serde_json::from_str(r#"{"codec":"pcm_s16le","sampleRate":48000,"channels":1}"#).unwrap();
        assert!(f.is_supported());
        let s: State = serde_json::from_str(
            r#"{"camera":"c","width":1,"height":1,"fps":30,"codec":"h264","bitrate":1,"audio":true,"micPermission":"denied"}"#,
        )
        .unwrap();
        assert!(s.audio);
        assert_eq!(s.mic_permission, "denied");
        // Phones from before audio leave the keys out.
        let old: State =
            serde_json::from_str(r#"{"camera":"c","width":1,"height":1,"fps":30,"codec":"h264","bitrate":1}"#).unwrap();
        assert!(!old.audio);
        assert_eq!(old.camera_paused_hint(), None);
        let caps: Capabilities = serde_json::from_str(r#"{"cameras":[]}"#).unwrap();
        assert!(caps.audio.is_none());
    }

    #[test]
    fn camera_paused_reasons() {
        let state = |extra: &str| -> State {
            serde_json::from_str(&format!(
                r#"{{"camera":"c","width":1,"height":1,"fps":30,"codec":"h264","bitrate":1{extra}}}"#
            ))
            .unwrap()
        };
        assert_eq!(state("").camera_paused_hint(), None);
        assert_eq!(state(r#","cameraPaused":"""#).camera_paused_hint(), None);
        assert!(state(r#","cameraPaused":"inUse""#).camera_paused_hint().unwrap().contains("Another app"));
        // A reason from a newer phone app still says that the camera is paused.
        assert!(state(r#","cameraPaused":"somethingNew""#).camera_paused_hint().is_some());
    }

    #[test]
    fn hello_id_is_optional() {
        let old: Hello = serde_json::from_str(r#"{"protocol":1,"name":"Pixel 8"}"#).unwrap();
        assert_eq!(old.id, None);
        let new: Hello = serde_json::from_str(r#"{"protocol":1,"name":"Pixel 8","id":"abc"}"#).unwrap();
        assert_eq!(new.id.as_deref(), Some("abc"));
        // The desktop has no id of its own and leaves the key out.
        let ours = Hello { protocol: 1, name: "pc".into(), app: String::new(), app_version: String::new(), platform: String::new(), id: None };
        assert!(!serde_json::to_string(&ours).unwrap().contains("\"id\""));
    }

    /// A shared sample message from `protocol-fixtures/` (see the README there).
    macro_rules! fixture {
        ($name:literal) => {
            include_str!(concat!("../../protocol-fixtures/", $name))
        };
    }

    fn json(text: &str) -> serde_json::Value {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn fixtures_from_the_phone_are_understood() {
        let hello: Hello = serde_json::from_str(fixture!("hello_phone.json")).unwrap();
        assert_eq!((hello.protocol, hello.name.as_str(), hello.platform.as_str()), (2, "Nic's iPhone", "ios"));
        assert_eq!(hello.id.as_deref(), Some("3f2c8a52-6d1e-4b0a-9a57-0c1d2e3f4a5b"));

        let caps: Capabilities = serde_json::from_str(fixture!("capabilities.json")).unwrap();
        assert_eq!(caps.cameras.len(), 2);
        let wide = &caps.cameras[0];
        assert_eq!((wide.name.as_str(), wide.position.as_str(), wide.lens.as_str()), ("Wide", "back", "wide"));
        assert_eq!(wide.zoom.as_ref().map(|z| (z.min, z.max)), Some((1.0, 10.0)));
        assert_eq!((wide.formats[0].width, wide.formats[0].height), (3840, 2160));
        assert_eq!(wide.formats[1].fps, [24, 25, 30, 60, 120, 240]);
        assert!(wide.features.iter().any(|f| f == "torch"));
        assert_eq!(caps.codecs, ["h264", "hevc"]);
        assert_eq!(caps.bitrate.as_ref().map(|b| (b.min, b.max)), Some((1_000_000, 50_000_000)));
        assert_eq!(caps.audio.as_ref().map(|a| a.sample_rates.clone()), Some(vec![48_000]));

        let state: State = serde_json::from_str(fixture!("state.json")).unwrap();
        assert_eq!((state.width, state.height, state.fps, state.codec.as_str()), (1920, 1080, 30, "hevc"));
        assert_eq!((state.bitrate, state.zoom, state.torch, state.stabilization, state.mirror), (12_000_000, 2.5, true, true, false));
        assert_eq!((state.thermal.as_str(), state.battery, state.charging), ("fair", Some(0.82), true));
        assert_eq!((state.audio, state.mic_permission.as_str(), state.camera_paused.as_str()), (true, "granted", "inUse"));

        // The oldest phone app sends only these keys; everything newer has a default.
        let old: State = serde_json::from_str(fixture!("state_minimal.json")).unwrap();
        assert_eq!((old.width, old.zoom, old.torch, old.battery, old.audio), (1280, 0.0, false, None, false));
        assert_eq!((old.thermal.as_str(), old.mic_permission.as_str(), old.camera_paused_hint()), ("", "", None));

        let format: StreamFormat = serde_json::from_str(fixture!("stream_format.json")).unwrap();
        assert_eq!(
            format,
            StreamFormat { codec: "h264".into(), width: 1920, height: 1080, fps: 30, rotation: 90, mirror: false }
        );
        let format: StreamFormat = serde_json::from_str(fixture!("stream_format_mirror.json")).unwrap();
        assert_eq!((format.rotation, format.mirror, format.fps), (270, true, 60));

        let audio: AudioFormat = serde_json::from_str(fixture!("audio_format.json")).unwrap();
        assert!(audio.is_supported());
        let err: ErrorMsg = serde_json::from_str(fixture!("error.json")).unwrap();
        assert_eq!(err.code, "busy");

        let proof: PairProof = serde_json::from_str(fixture!("pair_proof.json")).unwrap();
        assert_eq!(proof.proof.len(), 43);
        let no: PairResult = serde_json::from_str(fixture!("pair_result.json")).unwrap();
        assert_eq!(no, PairResult { accepted: false, reason: Some("declined".into()) });
    }

    #[test]
    fn what_we_send_matches_the_fixtures() {
        let hello = Hello {
            protocol: PROTOCOL_VERSION,
            name: "nic-pc".into(),
            app: "Pocket Webcam Receiver".into(),
            app_version: "0.1.0".into(),
            platform: "linux".into(),
            id: None,
        };
        assert_eq!(serde_json::to_value(&hello).unwrap(), json(fixture!("hello_desktop.json")));
        let request = PairRequest { mode: "qr".into(), name: "nic-pc".into() };
        assert_eq!(serde_json::to_value(&request).unwrap(), json(fixture!("pair_request.json")));

        let everything = ConfigUpdate {
            camera: Some("com.apple.avfoundation.avcapturedevice.built-in_video:0".into()),
            width: Some(1920),
            height: Some(1080),
            fps: Some(30),
            codec: Some("h264".into()),
            bitrate: Some(12_000_000),
            zoom: Some(2.5),
            torch: Some(true),
            stabilization: Some(true),
            mirror: Some(false),
            audio: Some(true),
        };
        assert_eq!(serde_json::to_value(&everything).unwrap(), json(fixture!("set_config.json")));
        let one = ConfigUpdate { fps: Some(60), ..Default::default() };
        assert_eq!(serde_json::to_value(&one).unwrap(), json(fixture!("set_config_partial.json")));
    }

    #[test]
    fn fixture_frames_read_and_write_byte_for_byte() {
        let frames = json(fixture!("frames.json"));
        let messages = frames["messages"].as_array().unwrap();
        assert!(messages.len() >= 6);
        for m in messages {
            let name = m["name"].as_str().unwrap();
            let hex: String = m["hex"].as_str().unwrap().split_whitespace().collect();
            let bytes: Vec<u8> = (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect();

            let msg = read_message(&mut bytes.as_slice()).unwrap();
            assert_eq!(u64::from(msg.kind), m["type"].as_u64().unwrap(), "{name}");
            let mut written = Vec::new();
            write_message(&mut written, msg.kind, &msg.payload).unwrap();
            assert_eq!(written, bytes, "{name}");

            if let Some(v) = m.get("video") {
                let frame = parse_video_frame(&msg.payload).unwrap();
                assert_eq!(frame.keyframe, v["keyframe"].as_bool().unwrap(), "{name}");
                assert_eq!(frame.pts_us, v["ptsMicros"].as_u64().unwrap(), "{name}");
                let data: String = frame.data.iter().map(|b| format!("{b:02x}")).collect();
                assert_eq!(data, v["dataHex"].as_str().unwrap(), "{name}");
            }
            if let Some(a) = m.get("audio") {
                let frame = parse_audio_frame(&msg.payload).unwrap();
                assert_eq!(frame.pts_us, a["ptsMicros"].as_u64().unwrap(), "{name}");
                let samples: Vec<i64> =
                    frame.samples.as_chunks::<2>().0.iter().map(|s| i64::from(i16::from_le_bytes(*s))).collect();
                let wanted: Vec<i64> = a["samples"].as_array().unwrap().iter().map(|s| s.as_i64().unwrap()).collect();
                assert_eq!(samples, wanted, "{name}");
            }
            if let Some(j) = m.get("json") {
                assert_eq!(&serde_json::from_slice::<serde_json::Value>(&msg.payload).unwrap(), j, "{name}");
            }
        }
    }

    #[test]
    fn config_update_skips_unset() {
        let u = ConfigUpdate { fps: Some(60), ..Default::default() };
        assert_eq!(serde_json::to_string(&u).unwrap(), "{\"fps\":60}");
    }
}
