import AVFoundation
import os

/// Owns the capture session: lens discovery, formats, frame rate, zoom, torch,
/// stabilization, mirroring and the microphone. Frames go to `onSampleBuffer` and
/// audio to `onAudioSampleBuffer`, each on its own private queue.
final class CameraManager: NSObject, @unchecked Sendable {
    let session = AVCaptureSession()
    let previewLayer = AVCaptureVideoPreviewLayer()

    var onSampleBuffer: ((CMSampleBuffer) -> Void)?
    var onAudioSampleBuffer: ((CMSampleBuffer) -> Void)?
    /// Clockwise degrees the receiver must rotate captured frames to be upright.
    var onCaptureRotation: ((Int) -> Void)?
    /// Clockwise degrees the on-screen preview is rotated by. Called on the main thread.
    var onPreviewRotation: ((Int) -> Void)?
    /// Why iOS has stopped the camera for now (`background`, `inUse`, `call`, `overheated`,
    /// `unknown`), or nil once it runs again. Called on the main thread.
    var onInterruption: ((String?) -> Void)?

    private let sessionQueue = DispatchQueue(label: "pocketwebcam.camera")
    private let videoQueue = DispatchQueue(label: "pocketwebcam.video", qos: .userInteractive)
    private let output = AVCaptureVideoDataOutput()
    private let audioQueue = DispatchQueue(label: "pocketwebcam.audio", qos: .userInteractive)
    private let audioOutput = AVCaptureAudioDataOutput()
    /// Present only while the mic is on, so iOS shows its "mic in use" dot only then.
    private var audioInput: AVCaptureDeviceInput?
    private let log = Logger(subsystem: "com.pocketwebcam", category: "camera")
    private var input: AVCaptureDeviceInput?
    private var rotationCoordinator: AVCaptureDevice.RotationCoordinator?
    private var rotationObservations: [NSKeyValueObservation] = []
    private var sessionObservers: [NSObjectProtocol] = []

    /// Physical cameras, in display order: back ultra wide, wide, telephoto, then front.
    let devices: [AVCaptureDevice] = {
        let discovery = AVCaptureDevice.DiscoverySession(
            deviceTypes: [.builtInUltraWideCamera, .builtInWideAngleCamera, .builtInTelephotoCamera],
            mediaType: .video, position: .unspecified
        )
        let order: [AVCaptureDevice.DeviceType] = [.builtInUltraWideCamera, .builtInWideAngleCamera, .builtInTelephotoCamera]
        return discovery.devices.sorted {
            if $0.position != $1.position { return $0.position == .back }
            return (order.firstIndex(of: $0.deviceType) ?? 9) < (order.firstIndex(of: $1.deviceType) ?? 9)
        }
    }()

    static let frameRates = [24, 25, 30, 48, 50, 60, 100, 120, 240]
    private static let pixelFormat = kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange

    override init() {
        super.init()
        previewLayer.session = session
        previewLayer.videoGravity = .resizeAspect
        observeSession()
        sessionQueue.async { self.configureSession() }
    }

    // MARK: Interruptions

    /// iOS takes the camera away at times (the app leaves the screen, another app or a
    /// call needs it, the phone is too hot) and gives it back later. Say so, so the
    /// computer can show why the picture stopped instead of just going blank.
    private func observeSession() {
        let center = NotificationCenter.default
        sessionObservers = [
            center.addObserver(forName: AVCaptureSession.wasInterruptedNotification, object: session, queue: .main) { [weak self] note in
                let raw = (note.userInfo?[AVCaptureSessionInterruptionReasonKey] as? NSNumber)?.intValue
                let reason = raw.flatMap { AVCaptureSession.InterruptionReason(rawValue: $0) }
                self?.onInterruption?(CameraManager.pauseReason(reason))
            },
            center.addObserver(forName: AVCaptureSession.interruptionEndedNotification, object: session, queue: .main) { [weak self] _ in
                self?.onInterruption?(nil)
            },
            center.addObserver(forName: AVCaptureSession.runtimeErrorNotification, object: session, queue: .main) { [weak self] note in
                guard let self else { return }
                let error = note.userInfo?[AVCaptureSessionErrorKey] as? AVError
                self.log.error("capture session error: \(error?.localizedDescription ?? "unknown")")
                // iOS restarted its media services under us: the session has to be started again.
                if error?.code == .mediaServicesWereReset { self.start() }
            },
        ]
    }

    static func pauseReason(_ reason: AVCaptureSession.InterruptionReason?) -> String {
        switch reason {
        case .videoDeviceNotAvailableInBackground: return "background"
        case .videoDeviceInUseByAnotherClient, .videoDeviceNotAvailableWithMultipleForegroundApps: return "inUse"
        case .audioDeviceInUseByAnotherClient: return "call"
        case .videoDeviceNotAvailableDueToSystemPressure: return "overheated"
        default: return "unknown"
        }
    }

    // MARK: Capabilities

    var capabilities: Capabilities {
        Capabilities(
            cameras: devices.map(cameraInfo),
            codecs: ["h264", "hevc"],
            bitrate: BitrateRange(min: 1_000_000, max: 50_000_000),
            audio: AudioCapabilities(codecs: ["pcm_s16le"], sampleRates: [48_000], channels: [1])
        )
    }

    private func cameraInfo(_ device: AVCaptureDevice) -> CameraInfo {
        var features = ["stabilization"]
        if device.hasTorch { features.append("torch") }
        if device.isFocusModeSupported(.locked) { features.append("focusLock") }
        if device.isExposureModeSupported(.locked) { features.append("exposureLock") }
        return CameraInfo(
            id: device.uniqueID,
            name: Self.lensName(device),
            position: device.position == .front ? "front" : "back",
            lens: Self.lensType(device),
            zoom: ZoomRange(min: Double(device.minAvailableVideoZoomFactor),
                            max: Double(min(device.maxAvailableVideoZoomFactor, 10))),
            formats: Self.formats(for: device),
            features: features
        )
    }

    /// 16:9 resolutions of 720p and up, each with every standard frame rate it supports.
    static func formats(for device: AVCaptureDevice) -> [FormatInfo] {
        var byResolution: [String: FormatInfo] = [:]
        for format in device.formats where CMFormatDescriptionGetMediaSubType(format.formatDescription) == pixelFormat {
            let d = CMVideoFormatDescriptionGetDimensions(format.formatDescription)
            let w = Int(d.width), h = Int(d.height)
            guard w >= 1280, abs(w * 9 - h * 16) <= 16 else { continue }
            let rates = frameRates.filter { fps in
                format.videoSupportedFrameRateRanges.contains { Double(fps) >= $0.minFrameRate && Double(fps) <= $0.maxFrameRate }
            }
            let key = "\(w)x\(h)"
            var entry = byResolution[key] ?? FormatInfo(width: w, height: h, fps: [])
            entry.fps = Array(Set(entry.fps).union(rates)).sorted()
            byResolution[key] = entry
        }
        return byResolution.values.sorted { $0.width > $1.width }
    }

    static func lensName(_ device: AVCaptureDevice) -> String {
        if device.position == .front { return "Front" }
        switch device.deviceType {
        case .builtInUltraWideCamera: return "Ultra Wide"
        case .builtInTelephotoCamera: return "Telephoto"
        default: return "Wide"
        }
    }

    static func lensType(_ device: AVCaptureDevice) -> String {
        if device.position == .front { return "trueDepth" }
        switch device.deviceType {
        case .builtInUltraWideCamera: return "ultraWide"
        case .builtInTelephotoCamera: return "telephoto"
        default: return "wide"
        }
    }

    /// Zoom relative to the main wide camera, from field of view: ~0.5 for ultra wide, ~3 or ~5 for tele.
    func relativeZoom(_ device: AVCaptureDevice) -> Double {
        guard let wide = devices.first(where: { $0.position == .back && $0.deviceType == .builtInWideAngleCamera }) else { return 1 }
        func halfTan(_ d: AVCaptureDevice) -> Double { tan(Double(d.activeFormat.videoFieldOfView) * .pi / 360) }
        return halfTan(wide) / halfTan(device)
    }

    // MARK: Session

    private func configureSession() {
        session.beginConfiguration()
        session.sessionPreset = .inputPriority  // let activeFormat decide resolution
        output.videoSettings = [kCVPixelBufferPixelFormatTypeKey as String: Self.pixelFormat]
        output.alwaysDiscardsLateVideoFrames = true
        output.setSampleBufferDelegate(self, queue: videoQueue)
        if session.canAddOutput(output) { session.addOutput(output) }
        audioOutput.setSampleBufferDelegate(self, queue: audioQueue)
        if session.canAddOutput(audioOutput) { session.addOutput(audioOutput) }
        session.commitConfiguration()
    }

    /// Adds or removes the microphone. The completion gets whether it's on now.
    func setMicrophone(_ on: Bool, completion: @escaping (Bool) -> Void) {
        sessionQueue.async { completion(self.setMicrophoneNow(on)) }
    }

    private func setMicrophoneNow(_ on: Bool) -> Bool {
        session.beginConfiguration()
        defer { session.commitConfiguration() }
        if !on {
            if let audioInput { session.removeInput(audioInput) }
            audioInput = nil
            return false
        }
        if audioInput != nil { return true }
        guard let mic = AVCaptureDevice.default(for: .audio),
              let input = try? AVCaptureDeviceInput(device: mic),
              session.canAddInput(input)
        else {
            log.error("microphone unavailable")
            return false
        }
        session.addInput(input)
        audioInput = input
        return true
    }

    func start() {
        sessionQueue.async { if !self.session.isRunning { self.session.startRunning() } }
    }

    func stop() {
        sessionQueue.async { if self.session.isRunning { self.session.stopRunning() } }
    }

    /// Applies `requested`, clamping to what the hardware supports. The completion
    /// receives the configuration actually in effect.
    func apply(_ requested: StreamConfig, completion: @escaping (StreamConfig) -> Void) {
        sessionQueue.async {
            let applied = self.applyNow(requested)
            completion(applied)
        }
    }

    private func applyNow(_ requested: StreamConfig) -> StreamConfig {
        var config = requested
        guard let device = devices.first(where: { $0.uniqueID == requested.camera })
                ?? devices.first(where: { $0.position == .back && $0.deviceType == .builtInWideAngleCamera })
                ?? devices.first
        else {
            log.error("no cameras available")
            return requested
        }
        config.camera = device.uniqueID

        session.beginConfiguration()
        if input?.device != device {
            if let input { session.removeInput(input) }
            if let newInput = try? AVCaptureDeviceInput(device: device), session.canAddInput(newInput) {
                session.addInput(newInput)
                input = newInput
            }
        }

        // Format: exact match if possible, otherwise the closest size that supports the fps.
        let (format, fps) = Self.bestFormat(device, width: config.width, height: config.height, fps: config.fps)
        do {
            try device.lockForConfiguration()
            if let format {
                device.activeFormat = format
                let d = CMVideoFormatDescriptionGetDimensions(format.formatDescription)
                config.width = Int(d.width)
                config.height = Int(d.height)
            }
            config.fps = fps
            let duration = CMTime(value: 1, timescale: CMTimeScale(fps))
            device.activeVideoMinFrameDuration = duration
            device.activeVideoMaxFrameDuration = duration

            let maxZoom = min(device.maxAvailableVideoZoomFactor, device.activeFormat.videoMaxZoomFactor, 10)
            let zoom = min(max(CGFloat(config.zoom), device.minAvailableVideoZoomFactor), maxZoom)
            device.videoZoomFactor = zoom
            config.zoom = Double(zoom)

            if device.hasTorch, device.isTorchModeSupported(.on) {
                device.torchMode = config.torch ? .on : .off
            } else {
                config.torch = false
            }
            if device.isFocusModeSupported(.continuousAutoFocus) { device.focusMode = .continuousAutoFocus }
            device.unlockForConfiguration()
        } catch {
            log.error("lockForConfiguration failed: \(error.localizedDescription)")
        }

        if let connection = output.connection(with: .video) {
            if connection.isVideoStabilizationSupported {
                // Stabilization adds a few frames of latency, so it's opt-in.
                connection.preferredVideoStabilizationMode = config.stabilization ? .standard : .off
            } else {
                config.stabilization = false
            }
            // Mirroring here would flip the still-sideways frame, which comes out upside
            // down once the computer rotates it. The computer mirrors instead (STREAM_FORMAT).
            if connection.isVideoMirroringSupported {
                connection.automaticallyAdjustsVideoMirroring = false
                connection.isVideoMirrored = false
            }
        }
        session.commitConfiguration()

        observeRotation(of: device)
        return config
    }

    static func bestFormat(_ device: AVCaptureDevice, width: Int, height: Int, fps: Int) -> (AVCaptureDevice.Format?, Int) {
        let candidates = device.formats.filter { CMFormatDescriptionGetMediaSubType($0.formatDescription) == pixelFormat }
        func dims(_ f: AVCaptureDevice.Format) -> (Int, Int) {
            let d = CMVideoFormatDescriptionGetDimensions(f.formatDescription)
            return (Int(d.width), Int(d.height))
        }
        func maxRate(_ f: AVCaptureDevice.Format) -> Double {
            f.videoSupportedFrameRateRanges.map(\.maxFrameRate).max() ?? 30
        }
        func supports(_ f: AVCaptureDevice.Format, _ fps: Int) -> Bool {
            f.videoSupportedFrameRateRanges.contains { Double(fps) >= $0.minFrameRate && Double(fps) <= $0.maxFrameRate }
        }

        // Exact size + fps. Prefer the format with the lowest max rate: high-fps
        // formats are often binned and look worse at normal frame rates.
        let exact = candidates.filter { dims($0) == (width, height) && supports($0, fps) }
        if let best = exact.min(by: { maxRate($0) < maxRate($1) }) { return (best, fps) }

        // Same size, clamp fps down to what it can do.
        if let sameSize = candidates.filter({ dims($0) == (width, height) }).max(by: { maxRate($0) < maxRate($1) }) {
            let clamped = frameRates.filter { supports(sameSize, $0) }.last ?? 30
            return (sameSize, clamped)
        }

        // Closest size by pixel count that supports the fps.
        let target = width * height
        let bySize = candidates.filter { supports($0, fps) }.min {
            abs(dims($0).0 * dims($0).1 - target) < abs(dims($1).0 * dims($1).1 - target)
        }
        return (bySize, bySize == nil ? 30 : fps)
    }

    // MARK: Rotation

    private func observeRotation(of device: AVCaptureDevice) {
        DispatchQueue.main.async {
            let coordinator = AVCaptureDevice.RotationCoordinator(device: device, previewLayer: self.previewLayer)
            self.rotationCoordinator = coordinator
            self.rotationObservations = [
                coordinator.observe(\.videoRotationAngleForHorizonLevelPreview, options: [.initial, .new]) { [weak self] c, _ in
                    self?.previewLayer.connection?.videoRotationAngle = c.videoRotationAngleForHorizonLevelPreview
                    self?.onPreviewRotation?(Int(c.videoRotationAngleForHorizonLevelPreview.rounded()) % 360)
                },
                coordinator.observe(\.videoRotationAngleForHorizonLevelCapture, options: [.initial, .new]) { [weak self] c, _ in
                    self?.onCaptureRotation?(Int(c.videoRotationAngleForHorizonLevelCapture.rounded()) % 360)
                },
            ]
        }
    }
}

extension CameraManager: AVCaptureVideoDataOutputSampleBufferDelegate, AVCaptureAudioDataOutputSampleBufferDelegate {
    func captureOutput(_ output: AVCaptureOutput, didOutput sampleBuffer: CMSampleBuffer, from connection: AVCaptureConnection) {
        if output === audioOutput {
            onAudioSampleBuffer?(sampleBuffer)
        } else {
            onSampleBuffer?(sampleBuffer)
        }
    }
}
