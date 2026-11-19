import AVFoundation
import os

/// Turns microphone sample buffers (whatever rate and format the hardware uses) into
/// 10 ms chunks of 48 kHz mono signed 16-bit little-endian PCM, the AUDIO_FRAME format.
///
/// Call everything on one queue (the camera's audio queue).
final class AudioPacketizer: @unchecked Sendable {
    static let sampleRate = 48_000.0
    static let chunkFrames = 480  // 10 ms
    private static let chunkBytes = chunkFrames * 2

    /// (pts in µs, PCM bytes) for every full chunk.
    var onChunk: ((UInt64, Data) -> Void)?

    private let log = Logger(subsystem: "com.pocketwebcam", category: "audio")
    private let outputFormat = AVAudioFormat(
        commonFormat: .pcmFormatInt16, sampleRate: sampleRate, channels: 1, interleaved: true
    )!
    private var inputFormat: AVAudioFormat?
    private var converter: AVAudioConverter?
    private var pending = Data()
    /// Capture time of the first sample in `pending`.
    private var pendingPTS: UInt64 = 0

    /// Forgets partial audio, e.g. when the mic is turned off and on again.
    func reset() {
        pending.removeAll()
        converter?.reset()
    }

    func process(_ sample: CMSampleBuffer) {
        guard let description = CMSampleBufferGetFormatDescription(sample) else { return }
        let format = AVAudioFormat(cmAudioFormatDescription: description)
        if format != inputFormat {
            inputFormat = format
            converter = AVAudioConverter(from: format, to: outputFormat)
            pending.removeAll()
            if converter == nil { log.error("no converter from \(format.description)") }
        }
        guard let converter else { return }

        let frames = AVAudioFrameCount(CMSampleBufferGetNumSamples(sample))
        guard frames > 0, let input = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: frames) else { return }
        input.frameLength = frames
        let copied = CMSampleBufferCopyPCMDataIntoAudioBufferList(
            sample, at: 0, frameCount: Int32(frames), into: input.mutableAudioBufferList
        )
        guard copied == noErr else { return }

        let capacity = AVAudioFrameCount(Double(frames) * outputFormat.sampleRate / format.sampleRate) + 64
        guard let output = AVAudioPCMBuffer(pcmFormat: outputFormat, frameCapacity: capacity) else { return }
        var fed = false
        var error: NSError?
        // `.noDataNow` (not end of stream) keeps the resampler's state for the next buffer.
        converter.convert(to: output, error: &error) { _, status in
            if fed {
                status.pointee = .noDataNow
                return nil
            }
            fed = true
            status.pointee = .haveData
            return input
        }
        if let error {
            log.error("audio conversion failed: \(error.localizedDescription)")
            return
        }
        guard output.frameLength > 0, let samples = output.int16ChannelData?[0] else { return }

        if pending.isEmpty {
            let pts = CMSampleBufferGetPresentationTimeStamp(sample)
            pendingPTS = UInt64(max(0, CMTimeGetSeconds(pts)) * 1_000_000)
        }
        pending.append(UnsafeBufferPointer(start: samples, count: Int(output.frameLength)))
        while pending.count >= Self.chunkBytes {
            onChunk?(pendingPTS, Data(pending.prefix(Self.chunkBytes)))
            pending.removeFirst(Self.chunkBytes)
            pendingPTS += UInt64(Self.chunkFrames) * 1_000_000 / UInt64(Self.sampleRate)
        }
    }

    /// Loudest sample of a chunk, 0 to 1, for the level meter.
    static func peak(_ chunk: Data) -> Float {
        chunk.withUnsafeBytes { raw in
            let samples = raw.bindMemory(to: Int16.self)
            let loudest = samples.reduce(0) { max($0, abs(Int32(Int16(littleEndian: $1)))) }
            return Float(loudest) / 32768
        }
    }
}
