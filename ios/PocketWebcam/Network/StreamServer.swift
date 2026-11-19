import Foundation
import Network
import os

/// What happens while pairing with a computer, for the UI.
enum PairingEvent: Equatable {
    /// A computer wants to pair by code: show it and ask the user.
    case codeRequested(computer: String, code: String)
    case paired(computer: String)
    case failed(String)
}

/// Listens for one desktop receiver, advertises over Bonjour, and sends video
/// without ever queueing it (see "Latency and backpressure" in PROTOCOL.md).
///
/// Every connection is TLS 1.3. Only paired computers get anything beyond the handshake;
/// others can only pair (PROTOCOL.md, "Pairing").
///
/// All state lives on `queue`; callbacks are invoked on that queue too.
final class StreamServer: @unchecked Sendable {
    var onHello: ((Hello) -> Void)?
    var onPairing: ((PairingEvent) -> Void)?
    var onDisconnected: (() -> Void)?
    var onSetConfig: ((ConfigUpdate) -> Void)?
    var onKeyframeRequest: (() -> Void)?
    /// Fired when the server dropped video and needs the encoder to restart on a keyframe.
    var onNeedKeyframe: (() -> Void)?
    var onListening: ((UInt16) -> Void)?

    private let queue = DispatchQueue(label: "pocketwebcam.net", qos: .userInteractive)
    private let log = Logger(subsystem: "com.pocketwebcam", category: "net")
    private var listener: NWListener?
    private var connection: NWConnection?
    private var handshakeDone = false
    private var parser = FrameParser(maxPayload: StreamServer.maxInboundPayload)
    private var videoInFlight = false
    private var waitingForKeyframe = true
    /// Audio chunks handed to the socket but not yet sent.
    private var audioInFlight = 0
    /// 200 ms of 10 ms chunks: more than this waiting means Wi-Fi stalled.
    private static let maxAudioInFlight = 20
    private var lastReceive = Date()
    private var connectedAt = Date()
    private var watchdog: DispatchSourceTimer?
    private var serviceName = "iPhone"
    /// This phone's own identifier (TXT key `id`); stays the same when it's renamed.
    private var deviceID = ""
    /// Everything a receiver sends is small (see "Framing" in PROTOCOL.md).
    private static let maxInboundPayload = 1024 * 1024
    /// A receiver that hasn't said HELLO after this many seconds is dropped.
    private static let helloDeadline: TimeInterval = 10
    /// While pairing, people compare codes: give them time.
    private static let pairingDeadline: TimeInterval = 100
    /// A scanned QR code can be used for this long.
    private static let scanValidFor: TimeInterval = 5 * 60

    private var identity: PhoneIdentity?
    private var paired: PairedComputers?
    /// The desktop's certificate fingerprint on the current connection.
    private var peer = ""
    private enum Phase {
        /// Waiting for HELLO (paired computer) or PAIR_REQUEST.
        case first
        /// Sent PAIR_PROOF; waiting for the computer's PAIR_RESULT.
        case pairingByScan(computer: String)
        /// Showing the code; `answered` once this phone's user tapped Pair.
        case pairingByCode(computer: String, answered: Bool)
        /// Paired just now; HELLO comes next.
        case paired
        case streaming
    }
    private var phase = Phase.first
    /// The QR code the user scanned last, until it's used or too old.
    private var scan: (code: Pairing.Scan, until: Date)?

    func start(name: String, id: String, identity: PhoneIdentity, paired: PairedComputers) {
        queue.async {
            self.deviceID = id
            self.identity = identity
            self.paired = paired
            self.startListener(name: name, port: Wire.defaultPort)
        }
    }

    /// The user scanned a computer's QR code: announce it, so that computer connects.
    func scanned(_ code: Pairing.Scan) {
        queue.async {
            self.scan = (code, Date().addingTimeInterval(Self.scanValidFor))
            self.listener?.service = self.service()
            let until = self.scan?.until
            self.queue.asyncAfter(deadline: .now() + Self.scanValidFor) {
                // Still the same scan and nobody used it: take it back off the network.
                guard let scan = self.scan, scan.until == until else { return }
                self.scan = nil
                self.listener?.service = self.service()
                self.onPairing?(.failed("The computer didn't connect. Check that it's on the same Wi-Fi, then scan again."))
            }
        }
    }

    /// Stops waiting for the computer whose code was scanned.
    func cancelScan() {
        queue.async {
            guard self.scan != nil else { return }
            self.scan = nil
            self.listener?.service = self.service()
        }
    }

    /// The user's answer to "does the computer show the same code?".
    func answerCode(_ yes: Bool) {
        queue.async {
            guard case .pairingByCode(let computer, let answered) = self.phase else { return }
            if yes {
                guard !answered else { return }
                self.phase = .pairingByCode(computer: computer, answered: true)
                self.sendControl(Wire.frame(.pairResult, json: PairResult(accepted: true)))
            } else {
                // Cancelling, before or after tapping Pair: the computer is told and dropped.
                self.phase = .first
                self.sendThenClose(Wire.frame(.pairResult, json: PairResult(accepted: false, reason: "declined")))
            }
        }
    }

    /// Re-advertises under a new Bonjour name.
    func rename(_ name: String) {
        queue.async {
            self.serviceName = name
            self.listener?.service = self.service()
        }
    }

    func stop() {
        queue.async {
            self.listener?.cancel()
            self.listener = nil
            self.closeConnection()
        }
    }

    // MARK: Sending

    func send<T: Encodable>(_ type: MessageType, json value: T) {
        queue.async { self.sendControl(Wire.frame(type, json: value)) }
    }

    /// Announces a new stream shape. Video is held back until the next keyframe.
    func sendStreamFormat(_ format: StreamFormat) {
        queue.async {
            self.waitingForKeyframe = true
            self.sendControl(Wire.frame(.streamFormat, json: format))
        }
    }

    func sendVideo(keyframe: Bool, ptsMicros: UInt64, annexB: Data) {
        queue.async {
            guard let connection = self.connection, self.handshakeDone else { return }
            if self.waitingForKeyframe && !keyframe { return }
            if self.videoInFlight {
                // The socket is still busy with the previous frame: drop, then resync on a keyframe.
                self.waitingForKeyframe = true
                self.onNeedKeyframe?()
                return
            }
            self.waitingForKeyframe = false
            self.videoInFlight = true
            let data = Wire.videoFrame(keyframe: keyframe, ptsMicros: ptsMicros, annexB: annexB)
            connection.send(content: data, completion: .contentProcessed { [weak self] error in
                guard let self else { return }
                self.queue.async {
                    self.videoInFlight = false
                    if let error { self.log.error("video send failed: \(error.localizedDescription)") }
                }
            })
        }
    }

    /// Unlike video, audio is sent even while a video frame is in flight (a gap in sound
    /// is audible, a skipped frame isn't). It's only dropped once 200 ms is backed up.
    func sendAudio(ptsMicros: UInt64, samples: Data) {
        queue.async {
            guard let connection = self.connection, self.handshakeDone else { return }
            guard self.audioInFlight < Self.maxAudioInFlight else { return }
            self.audioInFlight += 1
            let data = Wire.audioFrame(ptsMicros: ptsMicros, samples: samples)
            connection.send(content: data, completion: .contentProcessed { [weak self, weak connection] _ in
                guard let self else { return }
                self.queue.async {
                    // A late completion from a previous connection must not touch the new count.
                    guard connection != nil, connection === self.connection else { return }
                    self.audioInFlight = max(0, self.audioInFlight - 1)
                }
            })
        }
    }

    // MARK: Listener

    private func service() -> NWListener.Service {
        var txt = NWTXTRecord()
        txt["v"] = String(Wire.protocolVersion)
        txt["model"] = Self.modelIdentifier
        if !deviceID.isEmpty { txt["id"] = deviceID }
        if let identity { txt["fp"] = identity.fingerprint }
        if let scan, scan.until > Date() { txt["pair"] = Pairing.tag(secret: scan.code.secret) }
        return NWListener.Service(name: serviceName, type: Wire.serviceType, domain: nil, txtRecord: txt)
    }

    private func startListener(name: String, port: UInt16) {
        serviceName = name
        let tcp = NWProtocolTCP.Options()
        tcp.noDelay = true
        guard let identity, let secIdentity = sec_identity_create(identity.identity) else {
            log.error("no identity; can't listen")
            return
        }
        let tls = NWProtocolTLS.Options()
        let sec = tls.securityProtocolOptions
        sec_protocol_options_set_min_tls_protocol_version(sec, .TLSv13)
        sec_protocol_options_set_local_identity(sec, secIdentity)
        // The computer must show a certificate. Any certificate passes the handshake (TLS
        // still checks the computer holds its key); whether it's a paired computer is
        // decided right after, before anything is sent to it.
        sec_protocol_options_set_peer_authentication_required(sec, true)
        sec_protocol_options_set_verify_block(sec, { _, _, complete in complete(true) }, queue)
        let params = NWParameters(tls: tls, tcp: tcp)
        params.includePeerToPeer = false
        // The camera is for computers on the same local network, never for the internet:
        // don't accept connections that come in over mobile data.
        params.prohibitedInterfaceTypes = [.cellular]

        let listener: NWListener
        do {
            listener = try NWListener(using: params, on: NWEndpoint.Port(rawValue: port)!)
        } catch {
            // Port taken: let the system pick one. Receivers read the port from Bonjour.
            guard let any = try? NWListener(using: params) else {
                log.error("cannot create listener: \(error.localizedDescription)")
                return
            }
            listener = any
        }
        listener.service = service()
        listener.newConnectionHandler = { [weak self] conn in self?.accept(conn) }
        listener.stateUpdateHandler = { [weak self] state in
            guard let self else { return }
            switch state {
            case .ready:
                if let port = listener.port?.rawValue { self.onListening?(port) }
            case .failed(let error):
                self.log.error("listener failed: \(error.localizedDescription); restarting")
                listener.cancel()
                self.queue.asyncAfter(deadline: .now() + 1) {
                    self.startListener(name: self.serviceName, port: port)
                }
            default:
                break
            }
        }
        listener.start(queue: queue)
        self.listener = listener
    }

    // MARK: Connection

    private func accept(_ conn: NWConnection) {
        if connection != nil {
            // One receiver at a time: tell the newcomer and hang up.
            conn.start(queue: queue)
            let busy = ErrorMessage(code: "busy", message: "Another computer is already connected.")
            conn.send(content: Wire.frame(.error, json: busy), completion: .contentProcessed { _ in conn.cancel() })
            return
        }
        connection = conn
        handshakeDone = false
        phase = .first
        peer = ""
        parser = FrameParser(maxPayload: StreamServer.maxInboundPayload)
        videoInFlight = false
        waitingForKeyframe = true
        audioInFlight = 0
        lastReceive = Date()
        connectedAt = Date()

        conn.stateUpdateHandler = { [weak self, weak conn] state in
            guard let self, let conn, conn === self.connection else { return }
            switch state {
            case .ready:
                guard let fingerprint = Self.peerFingerprint(conn) else {
                    self.log.error("computer sent no certificate")
                    self.closeConnection()
                    return
                }
                self.peer = fingerprint
                self.receive(on: conn)
                self.startWatchdog()
            case .failed, .cancelled:
                self.closeConnection()
            default:
                break
            }
        }
        conn.start(queue: queue)
    }

    private func receive(on conn: NWConnection) {
        conn.receive(minimumIncompleteLength: 1, maximumLength: 64 * 1024) { [weak self] data, _, isComplete, error in
            guard let self, conn === self.connection else { return }
            if let data, !data.isEmpty {
                self.lastReceive = Date()
                do {
                    for message in try self.parser.append(data) { self.handle(message) }
                } catch {
                    self.log.error("bad data from receiver; disconnecting")
                    self.closeConnection()
                    return
                }
            }
            if isComplete || error != nil {
                self.closeConnection()
            } else {
                self.receive(on: conn)
            }
        }
    }

    private func handle(_ message: FrameParser.Message) {
        switch MessageType(rawValue: message.type) {
        case .pairRequest:
            guard case .first = phase,
                  let request = try? JSONDecoder().decode(PairRequest.self, from: message.payload) else { return }
            startPairing(request)
        case .pairResult:
            guard let result = try? JSONDecoder().decode(PairResult.self, from: message.payload) else { return }
            finishPairing(result)
        case .hello:
            switch phase {
            case .first:
                guard paired?.contains(peer) == true else {
                    // Not paired: this computer gets nothing, not even our HELLO.
                    log.info("refused a computer that isn't paired")
                    sendThenClose(Wire.frame(.error, json: ErrorMessage(
                        code: "not_paired", message: "This phone isn't paired with this computer.")))
                    return
                }
            case .paired:
                break
            default:
                return
            }
            guard let hello = try? JSONDecoder().decode(Hello.self, from: message.payload) else { return }
            guard hello.protocol == Wire.protocolVersion else {
                sendControl(Wire.frame(.error, json: ErrorMessage(code: "version", message: "Update Pocket Webcam on this computer.")))
                closeConnection()
                return
            }
            handshakeDone = true
            phase = .streaming
            onHello?(hello)
        // Nothing may change the camera before the receiver has said a valid HELLO.
        case .setConfig:
            guard handshakeDone else { return }
            if let update = try? JSONDecoder().decode(ConfigUpdate.self, from: message.payload) {
                onSetConfig?(update)
            }
        case .keyframeRequest:
            guard handshakeDone else { return }
            onKeyframeRequest?()
        case .ping:
            sendControl(Wire.frame(.pong, message.payload))
        case .error:
            log.error("receiver error: \(String(decoding: message.payload, as: UTF8.self))")
        default:
            break // pong and unknown types are ignored
        }
    }

    private func sendControl(_ data: Data) {
        connection?.send(content: data, completion: .contentProcessed { _ in })
    }

    /// Sends a last message, then hangs up.
    private func sendThenClose(_ data: Data) {
        guard let conn = connection else { return }
        conn.send(content: data, completion: .contentProcessed { [weak self] _ in
            self?.queue.async {
                guard conn === self?.connection else { return }
                self?.closeConnection()
            }
        })
    }

    // MARK: Pairing

    private func startPairing(_ request: PairRequest) {
        guard let identity else { return }
        let computer = request.name.isEmpty ? "Computer" : request.name
        switch request.mode {
        case "qr":
            guard let scan, scan.until > Date() else {
                sendThenClose(Wire.frame(.pairResult, json: PairResult(accepted: false, reason: "no_code")))
                return
            }
            guard scan.code.desktop == peer else {
                // Another computer, or someone in between who can't have the scanned
                // computer's key: not the one whose code was scanned.
                log.info("pairing refused: not the computer whose code was scanned")
                sendThenClose(Wire.frame(.pairResult, json: PairResult(accepted: false, reason: "wrong_computer")))
                return
            }
            guard let proof = Pairing.proof(secret: scan.code.secret, phone: identity.fingerprint, desktop: peer) else { return }
            phase = .pairingByScan(computer: scan.code.name.isEmpty ? computer : scan.code.name)
            sendControl(Wire.frame(.pairProof, json: PairProof(proof: proof)))
        case "code":
            guard let code = Pairing.code(desktop: peer, phone: identity.fingerprint) else { return }
            phase = .pairingByCode(computer: computer, answered: false)
            onPairing?(.codeRequested(computer: computer, code: code))
        default:
            sendThenClose(Wire.frame(.pairResult, json: PairResult(accepted: false, reason: "unsupported")))
        }
    }

    private func finishPairing(_ result: PairResult) {
        let computer: String
        switch phase {
        case .pairingByScan(let name):
            computer = name
        case .pairingByCode(let name, let answered):
            guard answered else {
                // The computer gave up before this phone's user answered.
                onPairing?(.failed("Pairing was cancelled on the computer."))
                closeConnection()
                return
            }
            computer = name
        default:
            return
        }
        guard result.accepted else {
            onPairing?(.failed(result.reason == "timeout" ? "Pairing timed out. Try again." : "The computer didn't accept the pairing."))
            closeConnection()
            return
        }
        paired?.add(peer, name: computer)
        if case .pairingByScan = phase {
            scan = nil
            listener?.service = service()
        }
        phase = .paired
        log.info("paired with \(computer, privacy: .public)")
        onPairing?(.paired(computer: computer))
    }

    /// SHA-256 fingerprint of the computer's certificate on a TLS connection.
    private static func peerFingerprint(_ conn: NWConnection) -> String? {
        guard let meta = conn.metadata(definition: NWProtocolTLS.definition) as? NWProtocolTLS.Metadata else { return nil }
        var der: Data?
        _ = sec_protocol_metadata_access_peer_certificate_chain(meta.securityProtocolMetadata) { cert in
            if der == nil { der = SecCertificateCopyData(sec_certificate_copy_ref(cert).takeRetainedValue()) as Data }
        }
        return der.map(Pairing.fingerprint(of:))
    }

    /// The receiver pings every 2 s; silence for 10 s means it's gone.
    private func startWatchdog() {
        watchdog?.cancel()
        let timer = DispatchSource.makeTimerSource(queue: queue)
        timer.schedule(deadline: .now() + 2, repeating: 2)
        timer.setEventHandler { [weak self] in
            guard let self, self.connection != nil else { return }
            // Nobody pings while people compare codes; the pairing deadline covers that.
            if !self.isPairing, Date().timeIntervalSince(self.lastReceive) > 10 {
                self.log.info("receiver timed out")
                self.closeConnection()
            } else if !self.handshakeDone, Date().timeIntervalSince(self.connectedAt) > self.firstDeadline {
                // Whoever connects has to introduce itself; otherwise it would keep real
                // computers out (one receiver at a time) just by sending pings.
                self.log.info("receiver never said hello")
                self.closeConnection()
            }
        }
        timer.resume()
        watchdog = timer
    }

    private var isPairing: Bool {
        switch phase {
        case .pairingByCode, .pairingByScan: true
        default: false
        }
    }

    /// How long a computer may take before HELLO: long while people compare codes.
    private var firstDeadline: TimeInterval {
        switch phase {
        case .pairingByCode, .pairingByScan: Self.pairingDeadline
        default: Self.helloDeadline
        }
    }

    private func closeConnection() {
        watchdog?.cancel()
        watchdog = nil
        guard let conn = connection else { return }
        connection = nil
        switch phase {
        case .pairingByCode, .pairingByScan:
            onPairing?(.failed("The computer stopped pairing."))
        default:
            break
        }
        phase = .first
        let wasConnected = handshakeDone
        handshakeDone = false
        conn.cancel()
        if wasConnected { onDisconnected?() }
    }

    private static let modelIdentifier: String = {
        var info = utsname()
        uname(&info)
        return withUnsafeBytes(of: &info.machine) { raw in
            String(decoding: raw.prefix { $0 != 0 }, as: UTF8.self)
        }
    }()
}
