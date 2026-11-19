import Foundation

/// Message types and payloads from PROTOCOL.md.
enum MessageType: UInt8 {
    case hello = 0x01
    case capabilities = 0x02
    case setConfig = 0x03
    case state = 0x04
    case streamFormat = 0x10
    case videoFrame = 0x11
    case keyframeRequest = 0x12
    case audioFormat = 0x13
    case audioFrame = 0x14
    case pairRequest = 0x30
    case pairProof = 0x31
    case pairResult = 0x32
    case ping = 0x20
    case pong = 0x21
    case error = 0x7F
}

enum Wire {
    /// v2: TLS 1.3 between paired devices only (PROTOCOL.md, "Encryption").
    static let protocolVersion = 2
    static let serviceType = "_pocketwebcam._tcp"
    static let defaultPort: UInt16 = 7310
    static let maxPayload = 16 * 1024 * 1024

    static func frame(_ type: MessageType, _ payload: Data = Data()) -> Data {
        var data = Data(capacity: 5 + payload.count)
        data.append(type.rawValue)
        withUnsafeBytes(of: UInt32(payload.count).bigEndian) { data.append(contentsOf: $0) }
        data.append(payload)
        return data
    }

    static func frame<T: Encodable>(_ type: MessageType, json value: T) -> Data {
        frame(type, (try? JSONEncoder().encode(value)) ?? Data("{}".utf8))
    }

    /// VIDEO_FRAME payload: flags, 8-byte big-endian pts in microseconds, Annex-B data.
    static func videoFrame(keyframe: Bool, ptsMicros: UInt64, annexB: Data) -> Data {
        var payload = Data(capacity: 9 + annexB.count)
        payload.append(keyframe ? 1 : 0)
        withUnsafeBytes(of: ptsMicros.bigEndian) { payload.append(contentsOf: $0) }
        payload.append(annexB)
        return frame(.videoFrame, payload)
    }

    /// AUDIO_FRAME payload: 8-byte big-endian pts in microseconds, then little-endian samples.
    static func audioFrame(ptsMicros: UInt64, samples: Data) -> Data {
        var payload = Data(capacity: 8 + samples.count)
        withUnsafeBytes(of: ptsMicros.bigEndian) { payload.append(contentsOf: $0) }
        payload.append(samples)
        return frame(.audioFrame, payload)
    }
}

/// Splits the incoming TCP byte stream into messages.
struct FrameParser {
    private var buffer = Data()
    private let maxPayload: Int

    /// A message with a bigger payload than `maxPayload` makes `append` throw.
    init(maxPayload: Int = Wire.maxPayload) {
        self.maxPayload = maxPayload
    }

    struct Message {
        let type: UInt8
        let payload: Data
    }

    enum ParseError: Error { case payloadTooLarge }

    mutating func append(_ data: Data) throws -> [Message] {
        buffer.append(data)
        var messages: [Message] = []
        while buffer.count >= 5 {
            let start = buffer.startIndex
            let length = buffer[start + 1 ..< start + 5].reduce(0) { $0 << 8 | Int($1) }
            guard length <= maxPayload else { throw ParseError.payloadTooLarge }
            guard buffer.count >= 5 + length else { break }
            let payload = Data(buffer[start + 5 ..< start + 5 + length])
            messages.append(Message(type: buffer[start], payload: payload))
            buffer = Data(buffer[(start + 5 + length)...])
        }
        return messages
    }
}

struct Hello: Codable {
    var `protocol`: Int
    var name: String
    var app: String?
    var appVersion: String?
    var platform: String?
    /// The phone's own identifier (see `AppModel.deviceID`). Computers don't send one.
    var id: String? = nil
}

struct ZoomRange: Codable, Hashable {
    var min: Double
    var max: Double
}

struct FormatInfo: Codable, Hashable {
    var width: Int
    var height: Int
    var fps: [Int]
}

struct CameraInfo: Codable, Hashable, Identifiable {
    var id: String
    var name: String
    var position: String
    var lens: String
    var zoom: ZoomRange
    var formats: [FormatInfo]
    var features: [String]
}

struct BitrateRange: Codable, Hashable {
    var min: Int
    var max: Int
}

struct AudioCapabilities: Codable {
    var codecs: [String]
    var sampleRates: [Int]
    var channels: [Int]
}

struct Capabilities: Codable {
    var cameras: [CameraInfo]
    var codecs: [String]
    var bitrate: BitrateRange
    /// Left out (nil) until the app can send audio.
    var audio: AudioCapabilities? = nil
}

/// The phone's full stream configuration. Persisted between launches.
struct StreamConfig: Codable, Equatable {
    var camera: String
    var width: Int
    var height: Int
    var fps: Int
    var codec: String
    var bitrate: Int
    var zoom: Double
    var torch: Bool
    var stabilization: Bool
    var mirror: Bool

    static let `default` = StreamConfig(
        camera: "", width: 1920, height: 1080, fps: 30, codec: "h264",
        bitrate: 10_000_000, zoom: 1.0, torch: false, stabilization: false, mirror: false
    )

    func merging(_ u: ConfigUpdate) -> StreamConfig {
        var c = self
        if let v = u.camera { c.camera = v }
        if let v = u.width { c.width = v }
        if let v = u.height { c.height = v }
        if let v = u.fps { c.fps = v }
        if let v = u.codec { c.codec = v }
        if let v = u.bitrate { c.bitrate = v }
        if let v = u.zoom { c.zoom = v }
        if let v = u.torch { c.torch = v }
        if let v = u.stabilization { c.stabilization = v }
        if let v = u.mirror { c.mirror = v }
        return c
    }
}

/// SET_CONFIG: only present keys change.
struct ConfigUpdate: Codable {
    var camera: String?
    var width: Int?
    var height: Int?
    var fps: Int?
    var codec: String?
    var bitrate: Int?
    var zoom: Double?
    var torch: Bool?
    var stabilization: Bool?
    var mirror: Bool?
    /// Microphone on or off. Not part of `StreamConfig`, so it isn't persisted.
    var audio: Bool?
}

/// STATE: the full config plus device health.
struct StateMessage: Encodable {
    var config: StreamConfig
    var thermal: String
    var battery: Double?
    var charging: Bool
    var audio = false
    /// `granted`, `denied` or `undetermined`.
    var micPermission = "undetermined"
    /// Why the camera has stopped for now (`background`, `inUse`, `call`, `overheated`,
    /// `unknown`); left out while it runs.
    var cameraPaused: String? = nil

    private enum Keys: String, CodingKey { case thermal, battery, charging, audio, micPermission, cameraPaused }

    func encode(to encoder: Encoder) throws {
        try config.encode(to: encoder)
        var c = encoder.container(keyedBy: Keys.self)
        try c.encode(thermal, forKey: .thermal)
        try c.encodeIfPresent(battery, forKey: .battery)
        try c.encode(charging, forKey: .charging)
        try c.encode(audio, forKey: .audio)
        try c.encode(micPermission, forKey: .micPermission)
        try c.encodeIfPresent(cameraPaused, forKey: .cameraPaused)
    }
}

struct StreamFormat: Codable, Equatable {
    var codec: String
    var width: Int
    var height: Int
    var fps: Int
    var rotation: Int
}

/// AUDIO_FORMAT: shape of the AUDIO_FRAME samples that follow.
struct AudioFormat: Codable, Equatable {
    var codec = "pcm_s16le"
    var sampleRate = 48_000
    var channels = 1
}

/// PAIR_REQUEST (computer to phone): the first message from a computer that isn't paired yet.
struct PairRequest: Codable {
    /// `qr` (this phone scanned the computer's code) or `code` (compare 6 digits).
    var mode: String
    var name: String
}

/// PAIR_PROOF (phone to computer): HMAC showing this phone scanned the computer's code.
struct PairProof: Codable {
    var proof: String
}

/// PAIR_RESULT (both ways).
struct PairResult: Codable {
    var accepted: Bool
    var reason: String? = nil
}

struct ErrorMessage: Codable {
    var code: String
    var message: String
}
