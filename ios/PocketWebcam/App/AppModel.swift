import AVFoundation
import SwiftUI
import os

/// Wires camera -> encoder -> network, and is the single source of truth for the UI.
/// Settings changes from the phone UI and from the desktop both go through `apply`.
@MainActor
final class AppModel: ObservableObject {
    @Published private(set) var config: StreamConfig
    @Published private(set) var capabilities: Capabilities
    @Published private(set) var connectedTo: String?
    @Published private(set) var thermal: ProcessInfo.ThermalState = .nominal
    @Published private(set) var streamFPS = 0
    @Published private(set) var streamMbps = 0.0
    @Published private(set) var listeningPort: UInt16?
    /// True while microphone audio is actually being sent.
    @Published private(set) var micOn = false
    /// Loudest recent mic sample, 0 to 1, for the level indicator.
    @Published private(set) var micLevel: Float = 0
    @Published private(set) var micPermission = AVAudioApplication.shared.recordPermission
    @Published var dimmed = false
    /// Clockwise degrees the preview is rotated; 90/270 means the picture shows taller than wide.
    @Published private(set) var previewRotation = 90
    /// Why iOS has stopped the camera for now, as STATE reports it; nil while it runs.
    @Published private(set) var cameraPaused: String?
    /// The pairing in progress, or how the last one ended; drives the pairing sheet.
    @Published var pairing: PairingPrompt?
    @Published private(set) var pairedComputers: [PairedComputer] = []
    /// Set when this phone's key couldn't be made or read; nothing can connect then.
    @Published private(set) var identityError: String?
    @Published var deviceName: String {
        didSet {
            UserDefaults.standard.set(deviceName, forKey: Keys.deviceName)
            server.rename(deviceName)
        }
    }

    let camera = CameraManager()
    /// Random, made once: lets a computer recognise this phone after it's renamed.
    private let deviceID: String
    private let encoder: VideoEncoder
    private let packetizer = AudioPacketizer()
    private let server = StreamServer()
    private let paired = PairedComputers()
    private let streaming = Locked(false)
    /// Read on the audio queue, like `streaming` on the video queue.
    private let audioStreaming = Locked(false)
    /// The last on/off request, so a slow start can tell it was cancelled meanwhile.
    private var micWanted = false
    /// Read on the video queue when a new stream starts, so it can't live on the main actor.
    private let rotation = Locked(0)
    private var statsTimer: Timer?

    private enum Keys {
        static let config = "streamConfig"
        static let deviceName = "deviceName"
        static let deviceID = "deviceID"
    }

    init() {
        var saved = StreamConfig.default
        if let data = UserDefaults.standard.data(forKey: Keys.config),
           let decoded = try? JSONDecoder().decode(StreamConfig.self, from: data) {
            saved = decoded
        }
        config = saved
        capabilities = Capabilities(cameras: [], codecs: [], bitrate: BitrateRange(min: 1_000_000, max: 50_000_000))
        deviceName = UserDefaults.standard.string(forKey: Keys.deviceName) ?? UIDevice.current.name
        if let savedID = UserDefaults.standard.string(forKey: Keys.deviceID), !savedID.isEmpty {
            deviceID = savedID
        } else {
            let made = UUID().uuidString.lowercased()
            UserDefaults.standard.set(made, forKey: Keys.deviceID)
            deviceID = made
        }
        encoder = VideoEncoder(settings: .init(codec: saved.codec, bitrate: saved.bitrate, fps: saved.fps))
        capabilities = camera.capabilities
        pairedComputers = paired.all.map { PairedComputer(fingerprint: $0.fingerprint, name: $0.name) }
        wire()
    }

    // MARK: Lifecycle

    func start() {
        UIDevice.current.isBatteryMonitoringEnabled = true
        camera.start()
        apply(ConfigUpdate())
        do {
            let identity = try PhoneIdentity.loadOrCreate()
            server.start(name: deviceName, id: deviceID, identity: identity, paired: paired)
        } catch {
            identityError = "Pocket Webcam couldn't create this phone's security key (\(error)). Restart the app; if it keeps happening, reinstall it."
        }
        statsTimer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.updateStats() }
        }
        NotificationCenter.default.addObserver(
            forName: ProcessInfo.thermalStateDidChangeNotification, object: nil, queue: .main
        ) { [weak self] _ in
            Task { @MainActor in
                guard let self else { return }
                self.thermal = ProcessInfo.processInfo.thermalState
                self.sendState()
            }
        }
    }

    func becameActive() {
        camera.start()
        UIApplication.shared.isIdleTimerDisabled = connectedTo != nil
    }

    // MARK: Settings

    /// Merges `update` into the config, lets the camera clamp it, then tells everyone.
    func apply(_ update: ConfigUpdate) {
        if let audio = update.audio { setMicrophone(audio) }
        var requested = config.merging(update)
        requested.bitrate = min(max(requested.bitrate, capabilities.bitrate.min), capabilities.bitrate.max)
        if !capabilities.codecs.contains(requested.codec) { requested.codec = "h264" }

        camera.apply(requested) { applied in
            Task { @MainActor in self.didApply(applied) }
        }
    }

    private func didApply(_ applied: StreamConfig) {
        config = applied
        if let data = try? JSONEncoder().encode(applied) {
            UserDefaults.standard.set(data, forKey: Keys.config)
        }
        // Size and lens changes reach the encoder through the frames themselves;
        // codec, bitrate and fps are pushed explicitly.
        encoder.update(.init(codec: applied.codec, bitrate: applied.bitrate, fps: applied.fps))
        sendState()
    }

    func selectCamera(_ id: String) { apply(ConfigUpdate(camera: id, zoom: 1.0)) }
    func setResolution(_ f: FormatInfo) { apply(ConfigUpdate(width: f.width, height: f.height)) }
    func setFPS(_ fps: Int) { apply(ConfigUpdate(fps: fps)) }
    func setQuality(_ q: Quality) { apply(ConfigUpdate(bitrate: q.bitrate(for: config))) }
    func setCodec(_ codec: String) { apply(ConfigUpdate(codec: codec)) }
    func setTorch(_ on: Bool) { apply(ConfigUpdate(torch: on)) }
    func setMirror(_ on: Bool) { apply(ConfigUpdate(mirror: on)) }
    func setStabilization(_ on: Bool) { apply(ConfigUpdate(stabilization: on)) }
    func setZoom(_ zoom: Double) { apply(ConfigUpdate(zoom: zoom)) }

    // MARK: Microphone

    /// Turns the mic on or off for this connection (from the computer or the phone's own
    /// button). Asks for permission the first time; a denial is reported in STATE.
    func setMicrophone(_ on: Bool) {
        guard on, connectedTo != nil else {
            stopMicrophone()
            return
        }
        switch AVAudioApplication.shared.recordPermission {
        case .granted:
            startMicrophone()
        case .undetermined:
            AVAudioApplication.requestRecordPermission { [weak self] granted in
                Task { @MainActor in
                    guard let self else { return }
                    self.micPermission = AVAudioApplication.shared.recordPermission
                    if granted && self.connectedTo != nil {
                        self.startMicrophone()
                    } else {
                        self.sendState()
                    }
                }
            }
        default:
            micPermission = .denied
            sendState()
        }
    }

    private func startMicrophone() {
        micPermission = .granted
        micWanted = true
        guard !micOn else { return }
        packetizer.reset()
        // Queued before any AUDIO_FRAME, so the receiver knows the format first.
        server.send(.audioFormat, json: AudioFormat())
        camera.setMicrophone(true) { [weak self] on in
            Task { @MainActor in
                guard let self else { return }
                // Turned off, or the computer went away, while the mic was starting.
                guard self.micWanted, self.connectedTo != nil else {
                    self.stopMicrophone()
                    return
                }
                self.audioStreaming.value = on
                self.micOn = on
                self.sendState()
            }
        }
    }

    private func stopMicrophone() {
        micWanted = false
        audioStreaming.value = false
        micLevel = 0
        camera.setMicrophone(false) { _ in }
        guard micOn else { return }
        micOn = false
        sendState()
    }

    // MARK: Pairing

    /// A scanned QR code or an opened `pocketwebcam://pair` link. False if it isn't one.
    @discardableResult
    func handlePairingLink(_ link: String) -> Bool {
        guard let scan = Pairing.parse(link) else {
            if link.hasPrefix("pocketwebcam://") {
                pairing = .failed("This code is from a different version of Pocket Webcam. Update the app on this phone and on the computer.")
            }
            return false
        }
        server.scanned(scan)
        pairing = .waiting(computer: scan.name)
        return true
    }

    /// The user's answer to "does the computer show this code?".
    func answerCode(_ yes: Bool) {
        server.answerCode(yes)
        if yes, case .code(let computer, let code) = pairing {
            pairing = .confirmed(computer: computer, code: code)
        } else if !yes {
            pairing = nil
        }
    }

    func cancelPairing() {
        switch pairing {
        case .code, .confirmed: server.answerCode(false)
        default: break
        }
        server.cancelScan()
        pairing = nil
    }

    func forgetComputer(_ fingerprint: String) {
        paired.forget(fingerprint)
        pairedComputers = paired.all.map { PairedComputer(fingerprint: $0.fingerprint, name: $0.name) }
    }

    private func pairingEvent(_ event: PairingEvent) {
        switch event {
        case .codeRequested(let computer, let code):
            pairing = .code(computer: computer, code: code)
        case .paired(let computer):
            pairedComputers = paired.all.map { PairedComputer(fingerprint: $0.fingerprint, name: $0.name) }
            pairing = .paired(computer: computer)
        case .failed(let why):
            // Only worth showing while the user is in the middle of pairing.
            if pairing != nil, !(pairing?.isFinished ?? false) { pairing = .failed(why) }
        }
    }

    var currentCamera: CameraInfo? { capabilities.cameras.first { $0.id == config.camera } }

    /// Label for the lens buttons: "0.5×", "1×", "3×" or "Front".
    func lensLabel(_ cam: CameraInfo) -> String {
        guard cam.position == "back", let device = camera.devices.first(where: { $0.uniqueID == cam.id }) else {
            return "Front"
        }
        let z = camera.relativeZoom(device)
        return z < 0.95 ? "0.5×" : "\(Int(z.rounded()))×"
    }

    var quality: Quality? { Quality.allCases.first { $0.bitrate(for: config) == config.bitrate } }

    /// Width / height of the picture as the preview shows it, so the frame can hug it.
    var previewAspect: CGFloat {
        let w = CGFloat(max(config.width, 1)), h = CGFloat(max(config.height, 1))
        return previewRotation % 180 == 0 ? w / h : h / w
    }

    // MARK: Wiring

    private func wire() {
        let encoder = self.encoder
        let server = self.server
        let streaming = self.streaming

        let rotation = self.rotation

        camera.onSampleBuffer = { sample in
            if streaming.value { encoder.encode(sample) }
        }
        let packetizer = self.packetizer
        let audioStreaming = self.audioStreaming
        camera.onAudioSampleBuffer = { sample in
            if audioStreaming.value { packetizer.process(sample) }
        }
        var chunks = 0
        packetizer.onChunk = { [weak self] pts, data in
            server.sendAudio(ptsMicros: pts, samples: data)
            // The level indicator only needs ~10 updates a second.
            chunks += 1
            if chunks % 10 == 0 {
                let level = AudioPacketizer.peak(data)
                Task { @MainActor in self?.micLevel = level }
            }
        }
        camera.onCaptureRotation = { angle in
            guard rotation.value != angle else { return }
            rotation.value = angle
            encoder.restart()  // new STREAM_FORMAT carrying the new rotation
        }
        camera.onPreviewRotation = { [weak self] angle in
            Task { @MainActor in self?.previewRotation = angle }
        }
        camera.onInterruption = { [weak self] reason in
            Task { @MainActor in
                guard let self, self.cameraPaused != reason else { return }
                self.cameraPaused = reason
                self.sendState()
            }
        }
        // Called on the video queue right before the stream's first frame is encoded.
        // Must enqueue synchronously so STREAM_FORMAT reaches the socket before that frame.
        encoder.onFormat = { w, h in
            let s = encoder.currentSettings
            server.sendStreamFormat(StreamFormat(codec: s.codec, width: w, height: h, fps: s.fps, rotation: rotation.value))
        }
        encoder.onFrame = { key, pts, data in
            server.sendVideo(keyframe: key, ptsMicros: pts, annexB: data)
        }

        server.onNeedKeyframe = { encoder.requestKeyframe() }
        server.onKeyframeRequest = { encoder.requestKeyframe() }
        server.onListening = { [weak self] port in
            Task { @MainActor in self?.listeningPort = port }
        }
        server.onPairing = { [weak self] event in
            Task { @MainActor in self?.pairingEvent(event) }
        }
        server.onHello = { [weak self] hello in
            Task { @MainActor in self?.receiverConnected(hello) }
        }
        server.onDisconnected = { [weak self] in
            Task { @MainActor in self?.receiverDisconnected() }
        }
        server.onSetConfig = { [weak self] update in
            Task { @MainActor in self?.apply(update) }
        }
    }

    private func receiverConnected(_ hello: Hello) {
        connectedTo = hello.name
        server.send(.hello, json: Hello(
            protocol: Wire.protocolVersion, name: deviceName, app: "Pocket Webcam",
            appVersion: Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String, platform: "ios",
            id: deviceID
        ))
        server.send(.capabilities, json: capabilities)
        sendState()
        encoder.restart()  // the next frame starts a fresh stream with a STREAM_FORMAT
        streaming.value = true
        UIApplication.shared.isIdleTimerDisabled = true
    }

    private func receiverDisconnected() {
        stopMicrophone()
        streaming.value = false
        connectedTo = nil
        dimmed = false
        UIApplication.shared.isIdleTimerDisabled = false
    }

    private func sendState() {
        guard connectedTo != nil else { return }
        let battery = UIDevice.current.batteryLevel
        let charging = [.charging, .full].contains(UIDevice.current.batteryState)
        server.send(.state, json: StateMessage(
            config: config, thermal: thermal.wireName,
            battery: battery < 0 ? nil : Double(battery), charging: charging,
            audio: micOn, micPermission: micPermission.wireName, cameraPaused: cameraPaused
        ))
    }

    private func updateStats() {
        let (frames, bytes) = encoder.takeStats()
        streamFPS = frames
        streamMbps = Double(bytes) * 8 / 1_000_000
    }
}

/// What the pairing sheet shows.
enum PairingPrompt: Equatable, Identifiable {
    /// Scanned a computer's code; waiting for it to connect.
    case waiting(computer: String)
    /// A computer asks to pair by comparing this code.
    case code(computer: String, code: String)
    /// Tapped Pair here; the computer's user still has to confirm.
    case confirmed(computer: String, code: String)
    case paired(computer: String)
    case failed(String)

    var id: String {
        switch self {
        case .waiting: "waiting"
        // Same id for both code steps, so the sheet stays up between them.
        case .code(_, let code), .confirmed(_, let code): "code-\(code)"
        case .paired: "paired"
        case .failed: "failed"
        }
    }

    var isFinished: Bool {
        switch self {
        case .paired, .failed: true
        default: false
        }
    }
}

struct PairedComputer: Identifiable, Equatable {
    var fingerprint: String
    var name: String
    var id: String { fingerprint }
}

/// Bitrate presets shown in the UI, scaled by resolution, frame rate and codec.
enum Quality: String, CaseIterable, Identifiable {
    case low = "Low", standard = "Standard", high = "High", max = "Max"
    var id: String { rawValue }

    func bitrate(for c: StreamConfig) -> Int {
        // ~0.1 bits per pixel per frame is "standard" H.264; HEVC needs ~30% less.
        let bitsPerPixel: Double = switch self {
        case .low: 0.05
        case .standard: 0.1
        case .high: 0.16
        case .max: 0.25
        }
        let codecFactor = c.codec == "hevc" ? 0.7 : 1.0
        // Frame rate above 30 costs less than linearly: motion between frames is smaller.
        let fpsFactor = Double(c.fps) <= 30 ? Double(c.fps) : 30 + (Double(c.fps) - 30) * 0.5
        let raw = Double(c.width * c.height) * fpsFactor * bitsPerPixel * codecFactor
        let mbps = (raw / 1_000_000).rounded()
        return Int(Swift.min(Swift.max(mbps, 1), 50)) * 1_000_000
    }
}

/// A lock-protected value shared between the main actor and the video queue.
final class Locked<Value>: @unchecked Sendable {
    private let lock = NSLock()
    private var _value: Value
    init(_ value: Value) { _value = value }
    var value: Value {
        get { lock.lock(); defer { lock.unlock() }; return _value }
        set { lock.lock(); _value = newValue; lock.unlock() }
    }
}

extension AVAudioApplication.recordPermission {
    var wireName: String {
        switch self {
        case .granted: "granted"
        case .denied: "denied"
        default: "undetermined"
        }
    }
}

extension ProcessInfo.ThermalState {
    var wireName: String {
        switch self {
        case .nominal: "nominal"
        case .fair: "fair"
        case .serious: "serious"
        case .critical: "critical"
        @unknown default: "nominal"
        }
    }
}
