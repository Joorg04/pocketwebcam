//! Everything the desktop window needs, behind one handle: phone discovery, the
//! connection loop, the virtual camera and the live preview.
//!
//! The window only reads [`Engine::snapshot`] and calls the command methods; all the
//! blocking work happens on two background threads (a scanner and a connection manager).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use log::{info, warn};

use crate::audio::{MicStats, MicStatus, VirtualMic};
use crate::capture::{self, Recorder, RecordingStatus};
use crate::discovery::{self, Browser, Phone};
use crate::effects::{Effects, EffectsControl};
use crate::protocol::{self, ConfigUpdate, Hello};
use crate::pairing::{self, Fingerprint, PairedPhone, Secret};
use crate::session::{self, CodePrompt, Current, Info, NotPaired, OnHello, OnPaired, Options, Pairing, PairingFailed, PromptState, Trust};
use crate::settings::Settings;
use crate::video::{Output, Preview, Sink};

/// How long to wait before starting phone discovery again after it failed.
const DISCOVERY_RETRY: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(200);
const RECONNECT_PAUSE: Duration = Duration::from_secs(2);
/// How often a virtual camera held by another program is tried again.
const CAMERA_RETRY: Duration = Duration::from_secs(3);

/// Which phone the user wants to be connected to.
#[derive(Debug, Clone, PartialEq)]
pub enum Wanted {
    /// Stay disconnected.
    Nothing,
    /// Connect to the remembered phone when it shows up (see `auto_target`).
    Auto,
    /// A phone from the list, by name.
    Phone(String),
    /// A phone at a fixed address (when mDNS is blocked).
    Host(SocketAddr),
}

/// Where the video is going.
#[derive(Debug, Clone, PartialEq)]
pub enum OutputStatus {
    Starting,
    /// Frames go to the virtual camera at `device`.
    VirtualCamera { device: String },
    /// No virtual camera; the preview works but other apps can't see the video.
    PreviewOnly { reason: String },
    /// Another program holds the virtual camera, so apps see its video instead of the
    /// phone. Retried every few seconds.
    Busy { reason: String },
    /// The user stopped the virtual camera; the preview keeps working.
    Stopped,
}

#[derive(Debug, Clone, Default)]
struct Shared {
    settings: Settings,
    wanted: Option<Wanted>,
    phones: Vec<Phone>,
    /// Addresses that just failed to connect, for discovery to check on.
    unreachable: Vec<SocketAddr>,
    /// Name of the phone we're currently dialing, until the handshake finishes.
    connecting: Option<String>,
    last_error: Option<String>,
    output: Option<OutputStatus>,
    rebuild_output: bool,
    /// The user pressed "Stop virtual camera". Not saved: the camera starts with the app.
    camera_stopped: bool,
    pairing: Option<PairingSession>,
}

/// A pairing in progress.
#[derive(Clone)]
enum PairingSession {
    /// The QR code is on screen; a phone that scanned it announces `secret.tag()`.
    Qr { secret: Secret, url: String, until: Instant },
    /// Comparing 6 digits with the phone called `phone`.
    Code { phone: String, prompt: Arc<CodePrompt> },
}

impl std::fmt::Debug for PairingSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PairingSession::Qr { until, .. } => write!(f, "Qr(until {until:?})"),
            PairingSession::Code { phone, prompt } => write!(f, "Code({phone}, {:?})", prompt.state()),
        }
    }
}

/// What the window shows while pairing.
#[derive(Debug, Clone, PartialEq)]
pub enum PairingView {
    /// Show this link as a QR code, for `left` more.
    Qr { url: String, left: Duration },
    /// Show `state.code` (once it's there) and ask whether it matches the phone's.
    Code { phone: String, state: PromptState },
}

/// A copy of the state for drawing one frame of the window.
pub struct Snapshot {
    pub settings: Settings,
    pub wanted: Wanted,
    pub phones: Vec<Phone>,
    pub connecting: Option<String>,
    pub last_error: Option<String>,
    pub output: OutputStatus,
    pub mic: MicStatus,
    /// Present while a phone is connected.
    pub phone: Option<PhoneSnapshot>,
    pub pairing: Option<PairingView>,
    pub paired: Vec<PairedPhone>,
}

impl Snapshot {
    /// The phone has been paired with this computer (going by its TXT record).
    pub fn is_paired(&self, phone: &Phone) -> bool {
        let Some(fp) = phone.fingerprint else { return false };
        let fp = fp.to_string();
        self.paired.iter().any(|p| p.fingerprint == fp)
    }
}

pub struct PhoneSnapshot {
    pub info: Info,
    pub frames: u64,
    pub dropped: u64,
    /// Encoded video bytes received from this phone.
    pub bytes: u64,
    pub audio_chunks: u64,
    /// This computer recently couldn't decode the video as fast as the phone sends it.
    pub decode_overloaded: bool,
}

#[derive(Clone)]
pub struct Engine {
    shared: Arc<Mutex<Shared>>,
    current: Current,
    preview: Arc<Preview>,
    /// Applied to the virtual camera feed; changing it never rebuilds the camera.
    effects: Arc<EffectsControl>,
    /// Writes the phone's stream to a file while recording.
    recorder: Arc<Recorder>,
    /// The output the connection loop is currently using, so the camera can be switched live.
    output: Arc<Mutex<Option<Arc<Output>>>>,
    /// The "Pocket Webcam Microphone" other apps see; there for as long as the engine runs.
    mic: Result<Arc<VirtualMic>, String>,
    /// This computer's key and the paired phones. Without it nothing can connect.
    trust: Result<Arc<Trust>, String>,
}

impl Engine {
    /// Starts discovery and the connection loop. Call after `gst::init()`.
    pub fn start(settings: Settings) -> Engine {
        let wanted = if settings.auto_connect { Wanted::Auto } else { Wanted::Nothing };
        let effects = Arc::new(EffectsControl::new(settings.effects.clone()));
        let mic = VirtualMic::create().map(Arc::new).map_err(|e| {
            warn!("no virtual microphone: {e:#}");
            format!("{e:#}")
        });
        if let Ok(mic) = &mic {
            mic.set_gain_db(settings.mic_gain_db);
        }
        let trust = Trust::load().map(Arc::new).map_err(|e| {
            warn!("no pairing identity: {e:#}");
            format!("Pocket Webcam couldn't set up this computer's encryption key: {e:#}")
        });
        let engine = Engine {
            shared: Arc::new(Mutex::new(Shared {
                settings,
                wanted: Some(wanted),
                ..Shared::default()
            })),
            current: Arc::new(Mutex::new(None)),
            preview: Arc::new(Preview::default()),
            effects,
            recorder: Arc::new(Recorder::default()),
            output: Arc::new(Mutex::new(None)),
            mic,
            trust,
        };
        engine.spawn_scanner();
        engine.spawn_manager();
        engine.spawn_camera_retry();
        engine
    }

    pub fn preview(&self) -> &Arc<Preview> {
        &self.preview
    }

    pub fn recorder(&self) -> &Arc<Recorder> {
        &self.recorder
    }

    pub fn snapshot(&self) -> Snapshot {
        let shared = self.shared.lock().unwrap().clone();
        let remote = self.current.lock().unwrap().clone();
        let phone = remote.map(|r| PhoneSnapshot {
            info: r.info.lock().unwrap().clone(),
            frames: r.frames.load(Ordering::Relaxed),
            dropped: r.dropped.load(Ordering::Relaxed),
            bytes: r.bytes.load(Ordering::Relaxed),
            audio_chunks: r.audio_chunks.load(Ordering::Relaxed),
            decode_overloaded: r.decode_overloaded(),
        });
        Snapshot {
            settings: shared.settings,
            wanted: shared.wanted.unwrap_or(Wanted::Nothing),
            phones: shared.phones,
            connecting: shared.connecting,
            last_error: shared.last_error,
            output: shared.output.unwrap_or(OutputStatus::Starting),
            mic: match &self.mic {
                Ok(_) => MicStatus::Ready,
                Err(reason) => MicStatus::Unavailable { reason: reason.clone() },
            },
            phone,
            pairing: shared.pairing.as_ref().map(|p| match p {
                PairingSession::Qr { url, until, .. } => {
                    PairingView::Qr { url: url.clone(), left: until.saturating_duration_since(Instant::now()) }
                }
                PairingSession::Code { phone, prompt } => PairingView::Code { phone: phone.clone(), state: prompt.state() },
            }),
            paired: self.trust.as_ref().map(|t| t.paired.list()).unwrap_or_default(),
        }
    }

    // ---- pairing ---------------------------------------------------------------------

    /// Shows a new QR code for a phone to scan. The phone that scans it connects and is
    /// paired; the code works for `pairing::QR_VALID_FOR`.
    pub fn start_qr_pairing(&self) {
        let Ok(trust) = &self.trust else { return };
        let secret = Secret::random();
        let url = pairing::qr_url(&trust.me, &secret, &session::hostname());
        self.shared.lock().unwrap().pairing =
            Some(PairingSession::Qr { secret, url, until: Instant::now() + pairing::QR_VALID_FOR });
    }

    /// Answers the 6-digit code question: `true` when the codes match.
    pub fn answer_code(&self, matches: bool) {
        if let Some(PairingSession::Code { prompt, .. }) = &self.shared.lock().unwrap().pairing {
            prompt.answer(matches);
        }
    }

    /// Closes the QR code or stops comparing codes.
    pub fn cancel_pairing(&self) {
        let session = self.shared.lock().unwrap().pairing.take();
        if let Some(PairingSession::Code { prompt, .. }) = session {
            prompt.answer(false);
        }
    }

    /// Forgets a paired phone; it has to be paired again to connect.
    pub fn forget_phone(&self, fingerprint: &str) {
        let Ok(trust) = &self.trust else { return };
        let Ok(fp) = Fingerprint::parse(fingerprint) else { return };
        if trust.paired.forget(&fp) {
            info!("forgot phone {fp}");
        }
        let connected_to_it = self.shared.lock().unwrap().phones.iter().any(|p| p.fingerprint == Some(fp))
            && self.current.lock().unwrap().is_some();
        if connected_to_it {
            self.set_wanted(Wanted::Auto);
        }
    }

    // ---- commands ------------------------------------------------------------------

    /// Connects to this phone (dropping the current connection first, if any).
    pub fn connect_phone(&self, name: &str) {
        self.set_wanted(Wanted::Phone(name.to_string()));
    }

    /// Connects to `ip` or `ip:port`.
    pub fn connect_host(&self, host: &str) -> Result<()> {
        let addr = discovery::parse_host(host.trim(), protocol::DEFAULT_PORT)?;
        self.set_wanted(Wanted::Host(addr));
        Ok(())
    }

    pub fn connect_auto(&self) {
        self.set_wanted(Wanted::Auto);
    }

    pub fn disconnect(&self) {
        self.set_wanted(Wanted::Nothing);
    }

    fn set_wanted(&self, wanted: Wanted) {
        {
            let mut s = self.shared.lock().unwrap();
            s.wanted = Some(wanted);
            s.last_error = None;
        }
        self.close_current();
    }

    fn close_current(&self) {
        if let Some(remote) = self.current.lock().unwrap().clone() {
            remote.close();
        }
    }

    /// Saves the settings; rebuilds the virtual camera if it is affected.
    pub fn apply_settings(&self, settings: Settings) {
        settings.save();
        self.effects.set(settings.effects.clone());
        if let Ok(mic) = &self.mic {
            mic.set_gain_db(settings.mic_gain_db);
        }
        let rebuild = {
            let mut s = self.shared.lock().unwrap();
            // Only a change to the virtual camera itself needs it rebuilt (and the phone reconnected).
            let o = &s.settings;
            let rebuild = (o.width, o.height, o.fps, o.hw_decode, &o.device)
                != (settings.width, settings.height, settings.fps, settings.hw_decode, &settings.device);
            s.settings = settings;
            s.rebuild_output |= rebuild;
            rebuild
        };
        if rebuild {
            self.close_current();
        }
    }

    /// Starts or stops the virtual camera. The preview and the phone connection keep running.
    pub fn set_virtual_camera(&self, on: bool) {
        let (settings, output) = {
            let mut s = self.shared.lock().unwrap();
            // "Start" also retries a camera that couldn't open (e.g. right after the setup script ran).
            let already = if on { matches!(s.output, Some(OutputStatus::VirtualCamera { .. })) } else { s.camera_stopped };
            if already {
                return;
            }
            s.camera_stopped = !on;
            (s.settings.clone(), self.output.lock().unwrap().clone())
        };
        let Some(output) = output else {
            // Not built yet: the connection loop builds it in the right state.
            return;
        };
        let (sink, status) = if on { camera_sink(&settings) } else { (Sink::Null, OutputStatus::Stopped) };
        let status = match output.set_sink(&sink) {
            Ok(()) => status,
            Err(e) => camera_failed(&sink, &e),
        };
        self.shared.lock().unwrap().output = Some(status);
    }

    /// Changes the look and adjustments live; `persist` also saves them to the settings file.
    /// Never rebuilds the virtual camera or drops the phone.
    pub fn set_effects(&self, effects: Effects, persist: bool) {
        self.effects.set(effects);
        let settings = {
            let mut s = self.shared.lock().unwrap();
            s.settings.effects = self.effects.get();
            s.settings.clone()
        };
        if persist {
            settings.save();
        }
    }

    /// Changes and saves settings that don't affect the virtual camera (scenes, capture
    /// folder, …). Never rebuilds the camera or drops the phone.
    pub fn update_settings(&self, change: impl FnOnce(&mut Settings)) {
        let settings = {
            let mut s = self.shared.lock().unwrap();
            change(&mut s.settings);
            s.settings.clone()
        };
        settings.save();
    }

    /// Starts recording the phone's stream into the videos folder. Returns the file's path.
    pub fn start_recording(&self) -> Result<PathBuf> {
        let settings = self.shared.lock().unwrap().settings.clone();
        let remote = self.current.lock().unwrap().clone().ok_or_else(|| anyhow!("no phone is connected"))?;
        let format = remote.info.lock().unwrap().format.clone().ok_or_else(|| anyhow!("the video hasn't started yet"))?;
        let dir = capture::videos_dir(&settings);
        let path = capture::new_capture_path(&dir, settings.capture_format.extension());
        let audio = remote.info.lock().unwrap().caps.audio.is_some();
        self.recorder.start(path.clone(), settings.capture_format, &format, audio)?;
        // The file has to start on a keyframe; don't wait up to a few seconds for the next one.
        self.request_keyframe();
        Ok(path)
    }

    pub fn stop_recording(&self) {
        self.recorder.stop();
    }

    pub fn recording(&self) -> Option<RecordingStatus> {
        self.recorder.status()
    }

    /// Remembers the phone called `name` as the one that connects by itself, and saves it.
    pub fn remember_phone(&self, name: &str) {
        let id = self.shared.lock().unwrap().phones.iter().find(|p| p.name == name).and_then(|p| p.id.clone());
        self.remember(name, id.as_deref());
    }

    fn remember(&self, name: &str, id: Option<&str>) {
        let settings = {
            let mut s = self.shared.lock().unwrap();
            if s.settings.last_phone.as_deref() == Some(name) && s.settings.last_phone_id.as_deref() == id {
                return;
            }
            s.settings.last_phone = Some(name.to_string());
            s.settings.last_phone_id = id.map(str::to_string);
            s.settings.clone()
        };
        settings.save();
    }

    /// Sends a change to the phone (lens, resolution, zoom, …) without blocking the window.
    pub fn send_config(&self, update: ConfigUpdate) {
        let Some(remote) = self.current.lock().unwrap().clone() else { return };
        thread::spawn(move || {
            if let Err(e) = remote.set_config(&update) {
                warn!("could not send settings to the phone: {e}");
            }
        });
    }

    /// Turns the phone's microphone on or off for this connection.
    pub fn set_phone_mic(&self, on: bool) {
        self.send_config(ConfigUpdate { audio: Some(on), ..Default::default() });
    }

    /// Level and buffer numbers for the meter; `None` without a virtual microphone.
    pub fn mic_stats(&self) -> Option<MicStats> {
        self.mic.as_ref().ok().map(|m| m.stats())
    }

    /// Asks the phone for a fresh keyframe (fixes a smeared picture after packet loss).
    pub fn request_keyframe(&self) {
        let Some(remote) = self.current.lock().unwrap().clone() else { return };
        thread::spawn(move || {
            let _ = remote.request_keyframe();
        });
    }

    // ---- background threads ----------------------------------------------------------

    /// Keeps the list of phones current with one long-running search.
    fn spawn_scanner(&self) {
        let engine = self.clone();
        thread::spawn(move || loop {
            match Browser::start() {
                Ok(mut browser) => loop {
                    if let Err(e) = browser.poll(POLL) {
                        warn!("{e:#}; starting it again");
                        break;
                    }
                    let failed = std::mem::take(&mut engine.shared.lock().unwrap().unreachable);
                    for addr in failed {
                        browser.connection_failed(addr);
                    }
                    let phones = browser.phones();
                    let mut s = engine.shared.lock().unwrap();
                    if s.phones != phones {
                        s.phones = phones;
                    }
                    // A phone just scanned the QR code: drop the current phone so the
                    // connection manager can pair it straight away.
                    let scanned = qr_phone(&s).map(|p| p.addr);
                    drop(s);
                    if let Some(addr) = scanned {
                        let busy_elsewhere = engine.current.lock().unwrap().as_ref().is_some_and(|r| r.info.lock().unwrap().addr != Some(addr));
                        if busy_elsewhere {
                            engine.close_current();
                        }
                    }
                },
                Err(e) => warn!("phone discovery failed: {e:#}"),
            }
            engine.shared.lock().unwrap().phones.clear();
            thread::sleep(DISCOVERY_RETRY);
        });
    }

    /// Takes the virtual camera back once the program that held it lets go.
    fn spawn_camera_retry(&self) {
        let engine = self.clone();
        thread::spawn(move || loop {
            thread::sleep(CAMERA_RETRY);
            let busy = {
                let s = engine.shared.lock().unwrap();
                matches!(s.output, Some(OutputStatus::Busy { .. })) && !s.camera_stopped
            };
            if busy {
                engine.set_virtual_camera(true);
            }
        });
    }

    fn spawn_manager(&self) {
        let engine = self.clone();
        thread::spawn(move || engine.manage());
    }

    fn manage(&self) {
        let mut output: Option<Arc<Output>> = None;
        loop {
            let (settings, wanted, phones, rebuild, stopped) = {
                let mut s = self.shared.lock().unwrap();
                let rebuild = std::mem::take(&mut s.rebuild_output);
                (s.settings.clone(), s.wanted.clone().unwrap_or(Wanted::Nothing), s.phones.clone(), rebuild, s.camera_stopped)
            };

            if output.is_none() || rebuild {
                output = None; // release the old virtual camera first
                *self.output.lock().unwrap() = None;
                match self.build_output(&settings, stopped) {
                    Ok((out, status)) => {
                        *self.output.lock().unwrap() = Some(out.clone());
                        output = Some(out);
                        self.shared.lock().unwrap().output = Some(status);
                    }
                    Err(e) => {
                        self.shared.lock().unwrap().last_error = Some(format!("{e:#}"));
                        thread::sleep(RECONNECT_PAUSE);
                        continue;
                    }
                }
            }
            let Some(out) = output.clone() else { continue };

            let trust = match &self.trust {
                Ok(trust) => trust.clone(),
                Err(e) => {
                    self.shared.lock().unwrap().last_error = Some(e.clone());
                    thread::sleep(RECONNECT_PAUSE);
                    continue;
                }
            };
            let Some(Target { name, id, addr, pairing }) = self.target(&wanted, &phones, &settings, &trust) else {
                thread::sleep(POLL);
                continue;
            };

            info!("connecting to {name} at {addr}");
            {
                let mut s = self.shared.lock().unwrap();
                s.connecting = Some(name.clone());
                s.last_error = None;
            }
            // A phone from the list that answers becomes the remembered one. (A phone
            // reached by address isn't in the list, so it can't connect by itself later.)
            let on_hello: Option<OnHello> = match &wanted {
                Wanted::Host(_) => None,
                _ => {
                    let (engine, name) = (self.clone(), name.clone());
                    let listed_id = phones.iter().find(|p| p.name == name).and_then(|p| p.id.clone());
                    Some(Box::new(move |hello: &Hello| {
                        engine.remember(&name, hello.id.as_deref().or(listed_id.as_deref()));
                    }))
                }
            };
            let on_paired: OnPaired = {
                let engine = self.clone();
                // The QR code or code question closes; the phone keeps streaming, and its
                // HELLO makes it the remembered phone.
                Box::new(move |_: &Fingerprint| engine.shared.lock().unwrap().pairing = None)
            };
            let opts = Options {
                hw_decode: settings.hw_decode,
                recorder: Some(self.recorder.clone()),
                mic: self.mic.as_ref().ok().cloned(),
                mic_auto: settings.mic_auto,
                on_hello,
                trust,
                pairing,
                phone_name: name.clone(),
                phone_id: id,
                on_paired: Some(on_paired),
            };
            let result = session::run(addr, &out, &opts, &self.current);
            {
                let mut s = self.shared.lock().unwrap();
                s.connecting = None;
                // A code comparison belongs to one connection attempt.
                if let (Pairing::Code(_), Some(PairingSession::Code { .. })) = (&opts.pairing, &s.pairing) {
                    s.pairing = None;
                }
                match result {
                    Ok(()) => info!("disconnected from {name}"),
                    Err(e) if e.downcast_ref::<PairingFailed>().is_some() => {
                        warn!("pairing with {name} failed: {e}");
                        s.pairing = None;
                        s.last_error = Some(e.to_string());
                        s.wanted = Some(Wanted::Auto);
                    }
                    Err(e) if e.downcast_ref::<NotPaired>().is_some() => {
                        warn!("{name}: {e}");
                        s.last_error = Some(format!(
                            "{name} isn't paired with this computer any more. Pick it in the phone list to pair it again."
                        ));
                        s.wanted = Some(Wanted::Auto);
                    }
                    Err(e) => {
                        warn!("{e:#}");
                        // A close requested by the window shows up as an error; don't show it.
                        let requested = s.wanted.as_ref() != Some(&wanted) || s.rebuild_output;
                        // The phone may have gone away without saying so: take it off the
                        // list while discovery checks, so the list doesn't offer a dead phone.
                        if !requested {
                            s.phones.retain(|p| p.addr != addr);
                            s.unreachable.push(addr);
                        }
                        // Searching automatically is silent; a phone the user picked deserves a reason.
                        if !requested && wanted != Wanted::Auto {
                            s.last_error = Some(format!(
                                "Couldn't reach {name}. Make sure Pocket Webcam is open on the phone and both are on the same Wi-Fi."
                            ));
                        }
                    }
                }
            }
            thread::sleep(RECONNECT_PAUSE.min(Duration::from_millis(800)));
        }
    }

    /// Who to connect to next, and whether that connection may pair.
    fn target(&self, wanted: &Wanted, phones: &[Phone], settings: &Settings, trust: &Trust) -> Option<Target> {
        let paired = |p: &Phone| p.fingerprint.is_some_and(|fp| trust.paired.contains(&fp));
        let listed = |p: &Phone, pairing: Pairing| Target { name: p.name.clone(), id: p.id.clone(), addr: p.addr, pairing };
        let mut s = self.shared.lock().unwrap();
        if matches!(&s.pairing, Some(PairingSession::Qr { until, .. }) if *until <= Instant::now()) {
            s.pairing = None;
        }
        // A phone that scanned the QR code comes first.
        if let Some(PairingSession::Qr { secret, .. }) = &s.pairing {
            let secret = *secret;
            if let Some(p) = qr_phone(&s) {
                return Some(listed(p, Pairing::Qr(secret)));
            }
        }
        match wanted {
            Wanted::Nothing => None,
            Wanted::Host(addr) => {
                // No TXT record to go by: pair by code if it turns out to be a new phone.
                let prompt = Arc::new(CodePrompt::default());
                s.pairing = Some(PairingSession::Code { phone: addr.to_string(), prompt: prompt.clone() });
                Some(Target { name: addr.to_string(), id: None, addr: *addr, pairing: Pairing::Code(prompt) })
            }
            Wanted::Phone(name) => {
                let p = phones.iter().find(|p| &p.name == name)?;
                if !p.compatible() {
                    s.last_error = Some(format!(
                        "{} has an older Pocket Webcam app that can't connect to this computer. Update the app on the phone.",
                        p.name
                    ));
                    s.wanted = Some(Wanted::Nothing);
                    return None;
                }
                if paired(p) {
                    return Some(listed(p, Pairing::None));
                }
                // Picking a phone that isn't paired yet pairs it by code.
                let prompt = Arc::new(CodePrompt::default());
                s.pairing = Some(PairingSession::Code { phone: p.name.clone(), prompt: prompt.clone() });
                Some(listed(p, Pairing::Code(prompt)))
            }
            Wanted::Auto => {
                let candidates: Vec<Phone> = phones.iter().filter(|p| p.compatible() && paired(p)).cloned().collect();
                let p = auto_target(&candidates, settings.last_phone_id.as_deref(), settings.last_phone.as_deref())?;
                Some(listed(p, Pairing::None))
            }
        }
    }

    fn build_output(&self, settings: &Settings, stopped: bool) -> Result<(Arc<Output>, OutputStatus)> {
        let (sink, status) = if stopped { (Sink::Null, OutputStatus::Stopped) } else { camera_sink(settings) };
        let new = |sink: &Sink| {
            Output::new(sink, settings.width, settings.height, settings.fps, Some(self.preview.clone()), self.effects.clone())
        };
        match new(&sink) {
            Ok(output) => Ok((output, status)),
            // Keep the preview (and the phone) working without the camera.
            Err(e) if matches!(sink, Sink::V4l2(_)) => {
                let output = new(&Sink::Null).map_err(|e| anyhow!("could not start the preview: {e:#}"))?;
                Ok((output, camera_failed(&sink, &e)))
            }
            Err(e) => Err(anyhow!("could not start the virtual camera: {e:#}")),
        }
    }
}

/// A connection the manager is about to make.
struct Target {
    name: String,
    id: Option<String>,
    addr: std::net::SocketAddr,
    pairing: Pairing,
}

/// The phone that has scanned the QR code on screen, if any.
fn qr_phone(s: &Shared) -> Option<&Phone> {
    let Some(PairingSession::Qr { secret, until, .. }) = &s.pairing else { return None };
    if *until <= Instant::now() {
        return None;
    }
    let tag = secret.tag();
    s.phones.iter().find(|p| p.compatible() && p.pair_tag.as_deref() == Some(tag.as_str()))
}

/// The phone to connect to without being asked, if it's on the network.
///
/// Once a phone is remembered, only that phone connects by itself: on a shared network
/// (an office, a dorm) the app must never pick up somebody else's phone just because the
/// user's own isn't there. It's recognised by its identifier, or by its name if its app
/// is from before identifiers. Before any phone is remembered (the very first start), a
/// phone that is alone on the network is taken to be the user's; with several to choose
/// from, the user picks.
fn auto_target<'a>(phones: &'a [Phone], remembered_id: Option<&str>, remembered_name: Option<&str>) -> Option<&'a Phone> {
    match (remembered_id, remembered_name) {
        (Some(id), _) => phones.iter().find(|p| p.id.as_deref() == Some(id)),
        (None, Some(name)) => phones.iter().find(|p| p.name == name),
        (None, None) => match phones {
            [only] => Some(only),
            _ => None,
        },
    }
}

/// The status after the virtual camera at `sink` failed to open with `err`.
fn camera_failed(sink: &Sink, err: &anyhow::Error) -> OutputStatus {
    #[cfg(target_os = "linux")]
    if let Sink::V4l2(device) = sink {
        let others = crate::loopback::other_users(device);
        if !others.is_empty() {
            warn!("{device} is held by {}: {err:#}", others.join(", "));
            return OutputStatus::Busy {
                reason: format!(
                    "Another program is sending its own video to the Pocket Webcam camera, so apps \
                     that pick it see that video instead of your phone. Programs using the camera: {}. \
                     Turn off the other program's virtual camera (for example in OBS) or close it; \
                     Pocket Webcam takes the camera back automatically.",
                    others.join(", ")
                ),
            };
        }
    }
    let _ = sink;
    OutputStatus::PreviewOnly { reason: format!("could not start the virtual camera: {err:#}") }
}

/// Where the virtual camera's frames go on this system, and the status that means.
#[cfg(windows)]
fn camera_sink(_settings: &Settings) -> (Sink, OutputStatus) {
    match crate::wincam::register_camera() {
        Ok(()) => (Sink::WinCamera, OutputStatus::VirtualCamera { device: "Pocket Webcam".into() }),
        Err(e) => (Sink::Null, OutputStatus::PreviewOnly { reason: format!("{e:#}") }),
    }
}

/// Where the virtual camera's frames go on this system, and the status that means.
#[cfg(not(windows))]
fn camera_sink(settings: &Settings) -> (Sink, OutputStatus) {
    if !cfg!(target_os = "linux") {
        return (Sink::Null, OutputStatus::PreviewOnly { reason: "virtual camera is not available on this system yet".into() });
    }
    if std::path::Path::new(&settings.device).exists() {
        (Sink::V4l2(settings.device.clone()), OutputStatus::VirtualCamera { device: settings.device.clone() })
    } else {
        (
            Sink::Null,
            OutputStatus::PreviewOnly {
                reason: format!("{} does not exist; the virtual camera isn't loaded", settings.device),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn phone(name: &str, id: Option<&str>) -> Phone {
        Phone {
            name: name.into(),
            addr: "10.0.0.2:7310".parse().unwrap(),
            id: id.map(str::to_string),
            version: 2,
            fingerprint: None,
            pair_tag: None,
        }
    }

    #[test]
    fn a_remembered_phone_is_the_only_one_that_connects_by_itself() {
        let mine = phone("iPhone", Some("mine"));
        let theirs = phone("iPhone (2)", Some("theirs"));
        let both = [theirs.clone(), mine.clone()];
        assert_eq!(auto_target(&both, Some("mine"), Some("iPhone")), Some(&mine));
        // Mine isn't there: somebody else's phone is left alone, even with my phone's old name.
        let renamed = phone("iPhone", Some("theirs"));
        assert_eq!(auto_target(&[theirs], Some("mine"), Some("iPhone")), None);
        assert_eq!(auto_target(&[renamed], Some("mine"), Some("iPhone")), None);
        // Renaming my phone doesn't lose it.
        let new_name = phone("Nic's iPhone", Some("mine"));
        assert_eq!(auto_target(std::slice::from_ref(&new_name), Some("mine"), Some("iPhone")), Some(&new_name));
    }

    #[test]
    fn a_phone_from_before_identifiers_is_recognised_by_name() {
        let old = phone("Pixel 8", None);
        let other = phone("Galaxy", None);
        assert_eq!(auto_target(&[other.clone(), old.clone()], None, Some("Pixel 8")), Some(&old));
        assert_eq!(auto_target(&[other], None, Some("Pixel 8")), None);
    }

    #[test]
    fn the_first_start_takes_a_lone_phone_but_never_chooses_between_several() {
        let a = phone("A", Some("a"));
        let b = phone("B", None);
        assert_eq!(auto_target(&[], None, None), None);
        assert_eq!(auto_target(std::slice::from_ref(&a), None, None), Some(&a));
        assert_eq!(auto_target(&[a, b], None, None), None);
    }
}
