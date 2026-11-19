//! One connection to one phone: TLS handshake, pairing, message loop, keepalive.

use std::io::BufReader;
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use log::{debug, info, warn};
use serde_json::json;

use crate::audio::VirtualMic;
use crate::capture::Recorder;
use crate::pairing::{self, Fingerprint, PairedPhones, Secret};
use crate::protocol::*;
use crate::tls::{Link, LinkReader};
use crate::video::{Decoder, Output};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const DEAD_AFTER: Duration = Duration::from_secs(10);
const PING_EVERY: Duration = Duration::from_secs(2);
/// How long the phone gets to answer a QR pairing (it already has the code, so it's quick).
const QR_ANSWER_WITHIN: Duration = Duration::from_secs(15);
/// A decoder queue that stays too long for this long means this computer really is too
/// slow. A new stream starts with a burst of frames while the decoder is still warming
/// up, and that burst is gone again well within this time.
const BEHIND_FOR: Duration = Duration::from_millis(750);
/// This many times the queue limit (about two seconds of video) is too much at once.
const FAR_BEHIND: u64 = 8;
/// How long "this computer is too slow" stays up after the decoder last fell behind.
const OVERLOAD_SHOWN_FOR: Duration = Duration::from_secs(30);

/// Keeps a slow decoder from adding delay. The phone only drops frames when the network
/// is slow; a computer that decodes slower than the phone sends would otherwise queue
/// frames without limit, and the picture would lag further and further behind.
///
/// When too many frames have been waiting for a while, the caller throws the queue away
/// and starts over from a fresh keyframe: a short freeze instead of growing delay.
#[derive(Default)]
struct Catchup {
    /// Since when the queue has been over the limit without a break.
    over_since: Option<Instant>,
}

impl Catchup {
    /// `queued` frames are waiting in the decoder at `now`; more than `limit` is too many.
    /// True when it's time to throw them away.
    fn fell_behind(&mut self, queued: u64, limit: u64, now: Instant) -> bool {
        if queued <= limit {
            self.over_since = None;
            return false;
        }
        let since = *self.over_since.get_or_insert(now);
        if queued < limit * FAR_BEHIND && now.duration_since(since) < BEHIND_FOR {
            return false;
        }
        self.over_since = None;
        true
    }
}

/// What we know about the connected phone. Shared with the command console.
#[derive(Default, Clone)]
pub struct Info {
    pub hello: Option<Hello>,
    pub caps: Capabilities,
    pub state: Option<State>,
    pub format: Option<StreamFormat>,
    pub audio_format: Option<AudioFormat>,
    pub rtt_ms: Option<f64>,
    /// Where the phone is: a loopback address means a USB cable via `adb forward`.
    pub addr: Option<SocketAddr>,
}

/// Handle for talking to the connected phone from other threads.
pub struct Remote {
    link: Arc<Link>,
    started: Instant,
    alive: AtomicBool,
    pub info: Mutex<Info>,
    pub frames: AtomicU64,
    pub dropped: AtomicU64,
    /// Encoded video bytes received, for the measured bitrate.
    pub bytes: AtomicU64,
    /// Audio chunks received.
    pub audio_chunks: AtomicU64,
    /// When the decoder last fell behind (see `Catchup`).
    overloaded_at: Mutex<Option<Instant>>,
}

impl Remote {
    /// This computer recently couldn't decode the video as fast as the phone sends it.
    pub fn decode_overloaded(&self) -> bool {
        self.overloaded_at.lock().unwrap().is_some_and(|t| t.elapsed() < OVERLOAD_SHOWN_FOR)
    }

    pub fn set_config(&self, update: &ConfigUpdate) -> Result<()> {
        write_json(&mut &*self.link, SET_CONFIG, update)?;
        Ok(())
    }

    /// Drops the connection; `run` returns shortly after.
    pub fn close(&self) {
        self.link.shutdown();
    }

    pub fn request_keyframe(&self) -> Result<()> {
        write_message(&mut &*self.link, KEYFRAME_REQ, &[])?;
        Ok(())
    }

    fn now_us(&self) -> u64 {
        self.started.elapsed().as_micros() as u64
    }

    fn ping(&self) -> Result<()> {
        write_json(&mut &*self.link, PING, &json!({ "t": self.now_us() }))?;
        Ok(())
    }
}

pub type Current = Arc<Mutex<Option<Arc<Remote>>>>;

/// Called with the phone's HELLO once it has been accepted.
pub type OnHello = Box<dyn Fn(&Hello) + Send + Sync>;
/// Called with the phone's fingerprint right after it was paired.
pub type OnPaired = Box<dyn Fn(&Fingerprint) + Send + Sync>;

/// What every connection needs to be encrypted and to know paired phones.
pub struct Trust {
    pub tls: Arc<rustls::ClientConfig>,
    /// This computer's certificate fingerprint.
    pub me: Fingerprint,
    pub paired: Arc<PairedPhones>,
}

impl Trust {
    /// This computer's identity from the config folder (made on first use) and the
    /// saved paired phones.
    pub fn load() -> Result<Trust> {
        let dir = crate::paths::config_dir().ok_or_else(|| anyhow!("no config folder to keep this computer's key in"))?;
        let identity = pairing::Identity::load_or_create(&dir.join("identity.pem"))?;
        Self::new(&identity, Arc::new(PairedPhones::load()))
    }

    pub fn new(identity: &pairing::Identity, paired: Arc<PairedPhones>) -> Result<Trust> {
        Ok(Trust { tls: crate::tls::client_config(identity)?, me: identity.fingerprint, paired })
    }
}

/// How a connection may pair if the phone isn't paired with this computer yet.
#[derive(Clone)]
pub enum Pairing {
    /// Paired phones only.
    None,
    /// The phone scanned this computer's QR code, made with this secret.
    Qr(Secret),
    /// Both screens show a 6-digit code; the window asks through the prompt.
    Code(Arc<CodePrompt>),
}

/// The phone isn't paired with this computer (or has forgotten it).
#[derive(Debug)]
pub struct NotPaired;

impl std::fmt::Display for NotPaired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("this phone isn't paired with this computer")
    }
}

impl std::error::Error for NotPaired {}

/// Pairing didn't work; the message is for the user. Don't retry by yourself.
#[derive(Debug)]
pub struct PairingFailed(pub String);

impl std::fmt::Display for PairingFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PairingFailed {}

fn failed(why: impl Into<String>) -> anyhow::Error {
    PairingFailed(why.into()).into()
}

/// The 6-digit code comparison, shared between the connection and the window.
#[derive(Default)]
pub struct CodePrompt {
    state: Mutex<PromptState>,
    changed: Condvar,
    link: Mutex<Option<Arc<Link>>>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PromptState {
    /// Set once the phone is connected and the code is known; show it until `finished`.
    pub code: Option<String>,
    /// The phone's person tapped Pair.
    pub phone_accepted: bool,
    /// This computer's person answered.
    pub answer: Option<bool>,
    pub finished: bool,
}

impl CodePrompt {
    pub fn state(&self) -> PromptState {
        self.state.lock().unwrap().clone()
    }

    /// The person at the computer confirmed (or cancelled) that the codes match.
    pub fn answer(&self, yes: bool) {
        self.state.lock().unwrap().answer = Some(yes);
        self.changed.notify_all();
        if !yes {
            // Stops waiting for the phone, too.
            if let Some(link) = self.link.lock().unwrap().as_ref() {
                link.shutdown();
            }
        }
    }

    fn update(&self, change: impl FnOnce(&mut PromptState)) {
        change(&mut self.state.lock().unwrap());
        self.changed.notify_all();
    }

    fn wait_for_answer(&self, deadline: Instant) -> Option<bool> {
        let mut state = self.state.lock().unwrap();
        while state.answer.is_none() {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            state = self.changed.wait_timeout(state, left).unwrap().0;
        }
        state.answer
    }
}

pub struct Options {
    pub hw_decode: bool,
    /// Receives every encoded frame while recording.
    pub recorder: Option<Arc<Recorder>>,
    /// Where the phone's microphone goes; `None` ignores audio.
    pub mic: Option<Arc<VirtualMic>>,
    /// Turn the phone's mic on as soon as it says it has one.
    pub mic_auto: bool,
    pub on_hello: Option<OnHello>,
    pub trust: Arc<Trust>,
    /// Allowed when the phone turns out not to be paired.
    pub pairing: Pairing,
    /// The phone's name and identifier from the list, saved with a new pairing.
    pub phone_name: String,
    pub phone_id: Option<String>,
    pub on_paired: Option<OnPaired>,
}

/// Connects to `addr` and streams until the phone disconnects or errors.
pub fn run(addr: SocketAddr, output: &Arc<Output>, opts: &Options, current: &Current) -> Result<()> {
    let stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)
        .with_context(|| format!("connecting to {addr}"))?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(DEAD_AFTER))?;
    let (link, reader) = Link::handshake(stream, opts.trust.tls.clone())?;
    let mut reader = BufReader::with_capacity(1 << 20, reader);
    if !opts.trust.paired.contains(&link.peer()) {
        let paired = pair(&link, &mut reader, opts);
        if let Pairing::Code(prompt) = &opts.pairing {
            prompt.update(|s| s.finished = true);
        }
        if let Err(e) = paired {
            link.shutdown();
            return Err(e);
        }
    }

    let remote = Arc::new(Remote {
        link: link.clone(),
        started: Instant::now(),
        alive: AtomicBool::new(true),
        info: Mutex::new(Info { addr: Some(addr), ..Info::default() }),
        frames: AtomicU64::new(0),
        dropped: AtomicU64::new(0),
        bytes: AtomicU64::new(0),
        audio_chunks: AtomicU64::new(0),
        overloaded_at: Mutex::new(None),
    });

    write_json(
        &mut &*remote.link,
        HELLO,
        &Hello {
            protocol: PROTOCOL_VERSION,
            name: hostname(),
            app: "Pocket Webcam Receiver".into(),
            app_version: env!("CARGO_PKG_VERSION").into(),
            platform: std::env::consts::OS.into(),
            id: None,
        },
    )?;

    *current.lock().unwrap() = Some(remote.clone());
    let pinger = {
        let remote = remote.clone();
        thread::spawn(move || {
            while remote.alive.load(Ordering::Relaxed) {
                if remote.ping().is_err() {
                    break;
                }
                thread::sleep(PING_EVERY);
            }
        })
    };

    let result = message_loop(reader, &remote, output, opts);
    if let Some(recorder) = &opts.recorder {
        recorder.connection_ended();
    }

    remote.alive.store(false, Ordering::Relaxed);
    remote.link.shutdown();
    *current.lock().unwrap() = None;
    let _ = pinger.join();
    result
}

fn message_loop(mut reader: BufReader<LinkReader>, remote: &Remote, output: &Arc<Output>, opts: &Options) -> Result<()> {
    let mut decoder: Option<Decoder> = None;
    let mut need_keyframe = true;
    let mut asked_for_audio = false;
    let mut catchup = Catchup::default();

    loop {
        let msg = match read_message(&mut reader) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => bail!("phone disconnected"),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                bail!("phone stopped responding")
            }
            Err(e) => return Err(e.into()),
        };

        match msg.kind {
            VIDEO_FRAME => {
                let Some(frame) = parse_video_frame(&msg.payload) else { continue };
                remote.bytes.fetch_add(frame.data.len() as u64, Ordering::Relaxed);
                if let Some(recorder) = &opts.recorder {
                    recorder.push(&frame);
                }
                let Some(dec) = decoder.as_ref() else { continue };
                let queued = dec.queued();
                if catchup.fell_behind(queued, dec.queue_limit(), Instant::now()) {
                    warn!("this computer can't decode the video fast enough ({queued} frames behind); skipping ahead");
                    *remote.overloaded_at.lock().unwrap() = Some(Instant::now());
                    remote.dropped.fetch_add(queued + 1, Ordering::Relaxed);
                    // A new decoder drops everything the old one had waiting.
                    let format = remote.info.lock().unwrap().format.clone();
                    drop(decoder.take());
                    decoder = format.and_then(|f| Decoder::new(&f, output.clone(), opts.hw_decode).ok());
                    need_keyframe = true;
                    remote.request_keyframe()?;
                    continue;
                }
                let Some(dec) = decoder.as_mut() else { continue };
                if need_keyframe && !frame.keyframe {
                    remote.dropped.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                need_keyframe = false;
                remote.frames.fetch_add(1, Ordering::Relaxed);
                if let Err(e) = dec.push(&frame) {
                    // Rebuild from scratch and resync on the next keyframe.
                    warn!("decode error: {e:#}; resyncing");
                    let format = remote.info.lock().unwrap().format.clone();
                    decoder = format.and_then(|f| Decoder::new(&f, output.clone(), opts.hw_decode).ok());
                    need_keyframe = true;
                    catchup = Catchup::default();
                    remote.request_keyframe()?;
                }
            }
            STREAM_FORMAT => {
                let format: StreamFormat = serde_json::from_slice(&msg.payload)?;
                if let Some(recorder) = &opts.recorder {
                    recorder.format_changed(&format);
                }
                drop(decoder.take()); // stop the old pipeline before starting the new one
                decoder = Some(Decoder::new(&format, output.clone(), opts.hw_decode)?);
                need_keyframe = true;
                catchup = Catchup::default();
                remote.info.lock().unwrap().format = Some(format);
            }
            AUDIO_FRAME => {
                let Some(frame) = parse_audio_frame(&msg.payload) else { continue };
                remote.audio_chunks.fetch_add(1, Ordering::Relaxed);
                let playable = remote.info.lock().unwrap().audio_format.as_ref().is_some_and(AudioFormat::is_supported);
                if let (true, Some(mic)) = (playable, &opts.mic) {
                    mic.push(frame.samples);
                }
                if let (true, Some(recorder)) = (playable, &opts.recorder) {
                    recorder.push_audio(&frame);
                }
            }
            AUDIO_FORMAT => {
                let format: AudioFormat = serde_json::from_slice(&msg.payload)?;
                if !format.is_supported() {
                    warn!("phone sends audio as {format:?}, which this version can't play");
                }
                remote.info.lock().unwrap().audio_format = Some(format);
            }
            HELLO => {
                let hello: Hello = serde_json::from_slice(&msg.payload)?;
                if hello.protocol != PROTOCOL_VERSION {
                    let _ = write_json(
                        &mut &*remote.link,
                        ERROR,
                        &json!({ "code": "version", "message": "unsupported protocol version" }),
                    );
                    bail!("phone speaks protocol v{}, we speak v{PROTOCOL_VERSION}", hello.protocol);
                }
                info!("connected to {} ({} {})", hello.name, hello.app, hello.app_version);
                opts.trust.paired.refresh(&remote.link.peer(), &hello.name, hello.id.as_deref());
                if let Some(on_hello) = &opts.on_hello {
                    on_hello(&hello);
                }
                remote.info.lock().unwrap().hello = Some(hello);
            }
            CAPABILITIES => {
                let caps: Capabilities = serde_json::from_slice(&msg.payload)?;
                let lenses: Vec<&str> = caps.cameras.iter().map(|c| c.name.as_str()).collect();
                info!("cameras: {}  (type `lens` to switch)", lenses.join(", "));
                if caps.audio.is_some() && opts.mic.is_some() && opts.mic_auto && !asked_for_audio {
                    asked_for_audio = true;
                    remote.set_config(&ConfigUpdate { audio: Some(true), ..Default::default() })?;
                }
                remote.info.lock().unwrap().caps = caps;
            }
            STATE => {
                let state: State = serde_json::from_slice(&msg.payload)?;
                if matches!(state.thermal.as_str(), "serious" | "critical") {
                    warn!("phone is {} hot; consider lowering resolution or fps", state.thermal);
                }
                debug!("state: {state:?}");
                remote.info.lock().unwrap().state = Some(state);
            }
            PING => write_message(&mut &*remote.link, PONG, &msg.payload)?,
            PONG => {
                if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&msg.payload) {
                    if let Some(t) = v["t"].as_u64() {
                        let rtt = remote.now_us().saturating_sub(t) as f64 / 1000.0;
                        remote.info.lock().unwrap().rtt_ms = Some(rtt);
                    }
                }
            }
            ERROR => {
                let err: ErrorMsg = serde_json::from_slice(&msg.payload)?;
                if err.code == "not_paired" {
                    // The phone forgot this computer: so does this computer, until they pair again.
                    opts.trust.paired.forget(&remote.link.peer());
                    return Err(NotPaired.into());
                }
                bail!("phone error ({}): {}", err.code, err.message);
            }
            _ => {} // unknown types are skipped, per PROTOCOL.md
        }
    }
}

/// Pairs with the phone at the other end of `link`, as `opts.pairing` allows, and saves it.
fn pair(link: &Arc<Link>, reader: &mut BufReader<LinkReader>, opts: &Options) -> Result<()> {
    let phone = link.peer();
    let me = opts.trust.me;
    let send = |kind: u8, value: &dyn erased::Json| -> Result<()> { Ok(write_message(&mut &**link, kind, &value.bytes()?)?) };
    let result = |accepted: bool, reason: Option<&str>| PairResult { accepted, reason: reason.map(str::to_string) };
    match &opts.pairing {
        Pairing::None => return Err(NotPaired.into()),
        Pairing::Qr(secret) => {
            info!("pairing with {} by QR code", opts.phone_name);
            send(PAIR_REQUEST, &PairRequest { mode: "qr".into(), name: hostname() })?;
            link.set_read_timeout(Some(QR_ANSWER_WITHIN))?;
            let msg = read_message(reader).map_err(|e| failed(format!("The phone didn't answer the pairing ({e}).")))?;
            match msg.kind {
                PAIR_PROOF => {
                    let p: PairProof = serde_json::from_slice(&msg.payload)?;
                    let bytes = pairing::decode_proof(&p.proof).unwrap_or_default();
                    if !pairing::proof_is_valid(secret, &phone, &me, &bytes) {
                        let _ = send(PAIR_RESULT, &result(false, Some("wrong_proof")));
                        return Err(failed("The phone's answer didn't match this computer's code. Scan it again."));
                    }
                }
                PAIR_RESULT => return Err(phone_refused(&msg.payload)),
                ERROR => return Err(failed(format!("The phone couldn't pair: {}", error_text(&msg.payload)))),
                other => bail!("unexpected message 0x{other:02x} while pairing"),
            }
            send(PAIR_RESULT, &result(true, None))?;
        }
        Pairing::Code(prompt) => {
            let code = pairing::code(&me, &phone);
            info!("pairing with {} by code", opts.phone_name);
            *prompt.link.lock().unwrap() = Some(link.clone());
            prompt.update(|s| s.code = Some(code));
            let deadline = Instant::now() + pairing::CODE_VALID_FOR;
            send(PAIR_REQUEST, &PairRequest { mode: "code".into(), name: hostname() })?;
            link.set_read_timeout(Some(pairing::CODE_VALID_FOR))?;
            let msg = match read_message(reader) {
                Ok(msg) => msg,
                Err(_) if prompt.state().answer == Some(false) => return Err(failed("Pairing cancelled.")),
                Err(e) => return Err(failed(format!("The phone didn't answer in time ({e})."))),
            };
            match msg.kind {
                PAIR_RESULT => {
                    let r: PairResult = serde_json::from_slice(&msg.payload)?;
                    if !r.accepted {
                        return Err(phone_refused(&msg.payload));
                    }
                }
                ERROR => return Err(failed(format!("The phone couldn't pair: {}", error_text(&msg.payload)))),
                other => bail!("unexpected message 0x{other:02x} while pairing"),
            }
            prompt.update(|s| s.phone_accepted = true);
            match prompt.wait_for_answer(deadline) {
                Some(true) => send(PAIR_RESULT, &result(true, None))?,
                Some(false) => {
                    let _ = send(PAIR_RESULT, &result(false, Some("declined")));
                    return Err(failed("Pairing cancelled."));
                }
                None => {
                    let _ = send(PAIR_RESULT, &result(false, Some("timeout")));
                    return Err(failed("Pairing timed out. Try again."));
                }
            }
        }
    }
    opts.trust.paired.add(&phone, &opts.phone_name, opts.phone_id.as_deref());
    info!("paired with {} ({phone})", opts.phone_name);
    link.set_read_timeout(Some(DEAD_AFTER))?;
    if let Some(on_paired) = &opts.on_paired {
        on_paired(&phone);
    }
    Ok(())
}

/// The phone said no (PAIR_RESULT with `accepted: false`).
fn phone_refused(payload: &[u8]) -> anyhow::Error {
    let reason = serde_json::from_slice::<PairResult>(payload).ok().and_then(|r| r.reason).unwrap_or_default();
    failed(match reason.as_str() {
        "declined" => "Pairing was declined on the phone.",
        "timeout" => "The phone stopped waiting. Try again.",
        "no_code" | "wrong_computer" => {
            "The phone hasn't scanned this computer's code. Scan the code shown here, or pair with the 6-digit code."
        }
        _ => "The phone didn't accept the pairing.",
    })
}

fn error_text(payload: &[u8]) -> String {
    serde_json::from_slice::<ErrorMsg>(payload).map(|e| e.message).unwrap_or_default()
}

/// Lets `pair` send any of the pairing messages through one closure.
mod erased {
    pub trait Json {
        fn bytes(&self) -> serde_json::Result<Vec<u8>>;
    }
    impl<T: serde::Serialize> Json for T {
        fn bytes(&self) -> serde_json::Result<Vec<u8>> {
            serde_json::to_vec(self)
        }
    }
}

/// This computer's name, as the phone shows it ("Live on …").
pub fn hostname() -> String {
    let name = if cfg!(windows) {
        std::env::var("COMPUTERNAME").ok()
    } else {
        std::fs::read_to_string("/proc/sys/kernel/hostname").ok().or_else(|| std::env::var("HOSTNAME").ok())
    };
    name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| "Computer".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::pairing::Identity;
    use crate::tls::test_phone;

    fn options(trust: Arc<Trust>, pairing: Pairing) -> Options {
        Options {
            hw_decode: false,
            recorder: None,
            mic: None,
            mic_auto: false,
            on_hello: None,
            trust,
            pairing,
            phone_name: "Test Phone".into(),
            phone_id: Some("phone-id".into()),
            on_paired: None,
        }
    }

    fn trust() -> Arc<Trust> {
        let (desktop, _) = Identity::generate().unwrap();
        Arc::new(Trust::new(&desktop, Arc::new(PairedPhones::load_from(None))).unwrap())
    }

    fn json<T: serde::de::DeserializeOwned>(msg: &Message) -> T {
        serde_json::from_slice(&msg.payload).unwrap()
    }

    /// Connects to a test phone and runs the pairing step only.
    fn pair_with(opts: &Options, addr: SocketAddr) -> (Result<()>, Fingerprint) {
        let sock = TcpStream::connect(addr).unwrap();
        sock.set_read_timeout(Some(DEAD_AFTER)).unwrap();
        let (link, reader) = Link::handshake(sock, opts.trust.tls.clone()).unwrap();
        let mut reader = BufReader::new(reader);
        let result = pair(&link, &mut reader, opts);
        (result, link.peer())
    }

    #[test]
    fn a_phone_that_scanned_the_qr_code_gets_paired() {
        let trust = trust();
        let secret = Secret::random();
        let (phone, _) = Identity::generate().unwrap();
        let phone_fp = phone.fingerprint;
        let (addr, server) = test_phone::spawn(phone, move |mut a| {
            let req: PairRequest = json(&read_message(&mut a.stream).unwrap());
            assert_eq!(req.mode, "qr");
            let proof = pairing::proof(&secret, &phone_fp, &a.desktop);
            write_json(&mut a.stream, PAIR_PROOF, &PairProof { proof: pairing::encode_proof(&proof) }).unwrap();
            json::<PairResult>(&read_message(&mut a.stream).unwrap())
        });
        let opts = options(trust.clone(), Pairing::Qr(secret));
        let (result, peer) = pair_with(&opts, addr);
        result.unwrap();
        assert!(server.join().unwrap().accepted);
        assert!(trust.paired.contains(&peer));
        assert_eq!(trust.paired.list()[0].id.as_deref(), Some("phone-id"));
    }

    #[test]
    fn a_proof_for_another_computer_is_refused() {
        // A man in the middle relays the QR secret but has its own certificate towards the
        // phone, so the phone's proof names the wrong computer.
        let trust = trust();
        let secret = Secret::random();
        let (phone, _) = Identity::generate().unwrap();
        let phone_fp = phone.fingerprint;
        let (addr, server) = test_phone::spawn(phone, move |mut a| {
            read_message(&mut a.stream).unwrap();
            let proof = pairing::proof(&secret, &phone_fp, &Fingerprint([0xEE; 32]));
            write_json(&mut a.stream, PAIR_PROOF, &PairProof { proof: pairing::encode_proof(&proof) }).unwrap();
            json::<PairResult>(&read_message(&mut a.stream).unwrap())
        });
        let opts = options(trust.clone(), Pairing::Qr(secret));
        let (result, peer) = pair_with(&opts, addr);
        assert!(result.unwrap_err().downcast_ref::<PairingFailed>().is_some());
        let answer = server.join().unwrap();
        assert_eq!((answer.accepted, answer.reason.as_deref()), (false, Some("wrong_proof")));
        assert!(!trust.paired.contains(&peer));
    }

    #[test]
    fn matching_codes_pair_once_both_people_agree() {
        let trust = trust();
        let me = trust.me;
        let (phone, _) = Identity::generate().unwrap();
        let phone_fp = phone.fingerprint;
        let (addr, server) = test_phone::spawn(phone, move |mut a| {
            let req: PairRequest = json(&read_message(&mut a.stream).unwrap());
            assert_eq!(req.mode, "code");
            write_json(&mut a.stream, PAIR_RESULT, &PairResult { accepted: true, reason: None }).unwrap();
            let answer: PairResult = json(&read_message(&mut a.stream).unwrap());
            (pairing::code(&a.desktop, &phone_fp), answer)
        });
        let prompt = Arc::new(CodePrompt::default());
        let person = {
            let prompt = prompt.clone();
            thread::spawn(move || {
                // The person reads the code off the screen, compares it and confirms.
                let shown = loop {
                    if let Some(code) = prompt.state().code {
                        break code;
                    }
                    thread::sleep(Duration::from_millis(5));
                };
                prompt.answer(true);
                shown
            })
        };
        let opts = options(trust.clone(), Pairing::Code(prompt.clone()));
        let (result, peer) = pair_with(&opts, addr);
        result.unwrap();
        let (phone_code, answer) = server.join().unwrap();
        assert_eq!(person.join().unwrap(), phone_code, "both screens show the same code");
        assert_eq!(phone_code, pairing::code(&me, &peer));
        assert!(answer.accepted && prompt.state().phone_accepted);
        assert!(trust.paired.contains(&peer));
    }

    #[test]
    fn cancelling_the_code_on_the_computer_pairs_nothing() {
        let trust = trust();
        let (phone, _) = Identity::generate().unwrap();
        let (addr, server) = test_phone::spawn(phone, |mut a| {
            read_message(&mut a.stream).unwrap();
            // The phone's person never answers; the computer gives up first.
            read_message(&mut a.stream).is_err()
        });
        let prompt = Arc::new(CodePrompt::default());
        let canceller = {
            let prompt = prompt.clone();
            thread::spawn(move || {
                while prompt.state().code.is_none() {
                    thread::sleep(Duration::from_millis(5));
                }
                prompt.answer(false);
            })
        };
        let opts = options(trust.clone(), Pairing::Code(prompt));
        let (result, peer) = pair_with(&opts, addr);
        canceller.join().unwrap();
        assert_eq!(result.unwrap_err().to_string(), "Pairing cancelled.");
        assert!(server.join().unwrap(), "the phone sees the connection close");
        assert!(!trust.paired.contains(&peer));
    }

    #[test]
    fn a_phone_that_declines_pairs_nothing() {
        let trust = trust();
        let (phone, _) = Identity::generate().unwrap();
        let (addr, _server) = test_phone::spawn(phone, |mut a| {
            read_message(&mut a.stream).unwrap();
            let no = PairResult { accepted: false, reason: Some("declined".into()) };
            write_json(&mut a.stream, PAIR_RESULT, &no).unwrap();
            let _ = read_message(&mut a.stream);
        });
        let opts = options(trust.clone(), Pairing::Code(Arc::new(CodePrompt::default())));
        let (result, peer) = pair_with(&opts, addr);
        assert_eq!(result.unwrap_err().to_string(), "Pairing was declined on the phone.");
        assert!(!trust.paired.contains(&peer));
    }

    #[test]
    fn an_unpaired_phone_is_not_paired_without_permission() {
        let trust = trust();
        let (phone, _) = Identity::generate().unwrap();
        let (addr, _server) = test_phone::spawn(phone, |mut a| {
            let _ = read_message(&mut a.stream);
        });
        let (result, _) = pair_with(&options(trust, Pairing::None), addr);
        assert!(result.unwrap_err().downcast_ref::<NotPaired>().is_some());
    }

    #[test]
    fn hostname_is_never_empty() {
        assert!(!hostname().is_empty());
    }

    /// Feeds queue lengths to `c`, one frame every 33 ms, with a limit of 8 frames.
    fn fell_behind(c: &mut Catchup, t: &mut Instant, queue: impl IntoIterator<Item = u64>) -> Vec<bool> {
        queue
            .into_iter()
            .map(|queued| {
                *t += Duration::from_millis(33);
                c.fell_behind(queued, 8, *t)
            })
            .collect()
    }

    #[test]
    fn a_decoder_that_keeps_up_is_left_alone() {
        let (mut c, mut t) = (Catchup::default(), Instant::now());
        assert!(!fell_behind(&mut c, &mut t, [0, 1, 3, 8, 0, 2]).contains(&true));
    }

    #[test]
    fn the_burst_at_the_start_of_a_stream_is_not_mistaken_for_a_slow_decoder() {
        let (mut c, mut t) = (Catchup::default(), Instant::now());
        // The decoder warms up, then works through the frames that piled up meanwhile.
        let burst = [3, 9, 14, 20, 20, 17, 12, 7, 2, 0, 1, 0];
        assert!(!fell_behind(&mut c, &mut t, burst).contains(&true));
        // Over the limit again later, but not for long: the clock started over.
        assert!(!fell_behind(&mut c, &mut t, [9; 20]).contains(&true));
    }

    #[test]
    fn a_queue_that_stays_too_long_is_thrown_away_again_and_again() {
        let (mut c, mut t) = (Catchup::default(), Instant::now());
        // Growing by one frame each time: 23 frames at 33 ms are just over BEHIND_FOR.
        let growing = fell_behind(&mut c, &mut t, 9..=32);
        assert!(!growing[..23].contains(&true));
        assert!(growing[23]);
        // The new decoder starts empty. Still too slow: it happens again, so the delay
        // never grows past about a second.
        assert!(!fell_behind(&mut c, &mut t, [0, 1, 2]).contains(&true));
        let again = fell_behind(&mut c, &mut t, 9..=40);
        assert_eq!(again.iter().filter(|b| **b).count(), 1);
    }

    #[test]
    fn a_huge_queue_is_thrown_away_at_once() {
        let (mut c, mut t) = (Catchup::default(), Instant::now());
        assert_eq!(fell_behind(&mut c, &mut t, [63, 64]), [false, true]);
    }
}
