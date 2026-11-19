import CoreMedia
import Foundation
import VideoToolbox
import os

/// Hardware H.264/HEVC encoder producing Annex-B access units.
///
/// The session is created lazily from the first frame's size, so any change in
/// camera format (resolution, lens, rotation request) simply produces a new
/// session and a new `onFormat` callback, which becomes a STREAM_FORMAT message.
final class VideoEncoder: @unchecked Sendable {
    struct Settings: Equatable {
        var codec: String   // "h264" | "hevc"
        var bitrate: Int
        var fps: Int
    }

    /// (width, height) of a freshly created session. Always followed by a keyframe.
    var onFormat: ((Int, Int) -> Void)?
    /// (keyframe, pts in µs, Annex-B data)
    var onFrame: ((Bool, UInt64, Data) -> Void)?

    private let lock = NSLock()
    private let log = Logger(subsystem: "com.pocketwebcam", category: "encoder")
    private var session: VTCompressionSession?
    private var width = 0
    private var height = 0
    private var settings: Settings
    private var forceKeyframe = true

    // Stats, read by the UI once a second. Separate lock: the output callback runs
    // while `lock` is held during session teardown (CompleteFrames flushes synchronously).
    private let statsLock = NSLock()
    private var frameCount = 0
    private var byteCount = 0

    init(settings: Settings) {
        self.settings = settings
    }

    deinit {
        invalidateSession()
    }

    /// Bitrate and fps change live; a codec change rebuilds the session.
    func update(_ new: Settings) {
        lock.lock(); defer { lock.unlock() }
        let old = settings
        settings = new
        guard let session else { return }
        if new.codec != old.codec {
            invalidateSession()
            return
        }
        if new.bitrate != old.bitrate { applyBitrate(session) }
        if new.fps != old.fps {
            VTSessionSetProperty(session, key: kVTCompressionPropertyKey_ExpectedFrameRate, value: new.fps as CFNumber)
            VTSessionSetProperty(session, key: kVTCompressionPropertyKey_MaxKeyFrameInterval, value: new.fps * 2 as CFNumber)
        }
    }

    func requestKeyframe() {
        lock.lock(); forceKeyframe = true; lock.unlock()
    }

    /// Drops the session so the next frame starts a fresh stream (new STREAM_FORMAT + keyframe).
    func restart() {
        lock.lock(); invalidateSession(); lock.unlock()
    }

    var currentSettings: Settings {
        lock.lock(); defer { lock.unlock() }
        return settings
    }

    /// Returns and resets (frames, bytes) since the last call.
    func takeStats() -> (frames: Int, bytes: Int) {
        statsLock.lock(); defer { statsLock.unlock() }
        let stats = (frameCount, byteCount)
        frameCount = 0
        byteCount = 0
        return stats
    }

    func encode(_ sampleBuffer: CMSampleBuffer) {
        guard let pixelBuffer = CMSampleBufferGetImageBuffer(sampleBuffer) else { return }
        let w = CVPixelBufferGetWidth(pixelBuffer)
        let h = CVPixelBufferGetHeight(pixelBuffer)

        lock.lock()
        var created = false
        if session == nil || w != width || h != height {
            invalidateSession()
            guard createSession(width: w, height: h) else { lock.unlock(); return }
            created = true
        }
        let session = self.session!
        let props: CFDictionary? = forceKeyframe
            ? [kVTEncodeFrameOptionKey_ForceKeyFrame: kCFBooleanTrue] as CFDictionary
            : nil
        forceKeyframe = false
        lock.unlock()

        if created { onFormat?(w, h) }

        let pts = CMSampleBufferGetPresentationTimeStamp(sampleBuffer)
        VTCompressionSessionEncodeFrame(
            session, imageBuffer: pixelBuffer, presentationTimeStamp: pts,
            duration: .invalid, frameProperties: props, infoFlagsOut: nil
        ) { [weak self] status, _, encoded in
            guard let self, status == noErr, let encoded else { return }
            self.emit(encoded)
        }
    }

    // MARK: Session

    /// Caller holds `lock`.
    private func createSession(width: Int, height: Int) -> Bool {
        let hevc = settings.codec == "hevc"
        // Low-latency rate control (H.264 only) keeps frames small and evenly sized.
        let spec: CFDictionary? = hevc ? nil
            : [kVTVideoEncoderSpecification_EnableLowLatencyRateControl: kCFBooleanTrue] as CFDictionary

        var session: VTCompressionSession?
        let status = VTCompressionSessionCreate(
            allocator: nil, width: Int32(width), height: Int32(height),
            codecType: hevc ? kCMVideoCodecType_HEVC : kCMVideoCodecType_H264,
            encoderSpecification: spec, imageBufferAttributes: nil, compressedDataAllocator: nil,
            outputCallback: nil, refcon: nil, compressionSessionOut: &session
        )
        guard status == noErr, let session else {
            log.error("VTCompressionSessionCreate failed: \(status)")
            return false
        }

        let fps = settings.fps
        let profile = hevc ? kVTProfileLevel_HEVC_Main_AutoLevel : kVTProfileLevel_H264_High_AutoLevel
        VTSessionSetProperty(session, key: kVTCompressionPropertyKey_RealTime, value: kCFBooleanTrue)
        VTSessionSetProperty(session, key: kVTCompressionPropertyKey_ProfileLevel, value: profile)
        VTSessionSetProperty(session, key: kVTCompressionPropertyKey_AllowFrameReordering, value: kCFBooleanFalse)
        VTSessionSetProperty(session, key: kVTCompressionPropertyKey_ExpectedFrameRate, value: fps as CFNumber)
        VTSessionSetProperty(session, key: kVTCompressionPropertyKey_MaxKeyFrameInterval, value: fps * 2 as CFNumber)
        VTSessionSetProperty(session, key: kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration, value: 2 as CFNumber)
        applyBitrate(session)
        VTCompressionSessionPrepareToEncodeFrames(session)

        self.session = session
        self.width = width
        self.height = height
        forceKeyframe = true
        log.info("encoder: \(self.settings.codec) \(width)x\(height) @ \(fps) fps")
        return true
    }

    private func applyBitrate(_ session: VTCompressionSession) {
        let bitrate = settings.bitrate
        VTSessionSetProperty(session, key: kVTCompressionPropertyKey_AverageBitRate, value: bitrate as CFNumber)
        // Hard cap at 1.5x average over one second, in bytes.
        let limits = [Double(bitrate) * 1.5 / 8, 1.0] as CFArray
        VTSessionSetProperty(session, key: kVTCompressionPropertyKey_DataRateLimits, value: limits)
    }

    /// Caller holds `lock`.
    private func invalidateSession() {
        guard let session else { return }
        VTCompressionSessionCompleteFrames(session, untilPresentationTimeStamp: .invalid)
        VTCompressionSessionInvalidate(session)
        self.session = nil
    }

    // MARK: Output

    private func emit(_ sample: CMSampleBuffer) {
        guard let block = CMSampleBufferGetDataBuffer(sample) else { return }

        let attachments = CMSampleBufferGetSampleAttachmentsArray(sample, createIfNecessary: false) as? [[CFString: Any]]
        let notSync = attachments?.first?[kCMSampleAttachmentKey_NotSync] as? Bool ?? false
        let keyframe = !notSync

        var out = Data()
        let startCode: [UInt8] = [0, 0, 0, 1]
        var nalLengthSize = 4

        if keyframe, let format = CMSampleBufferGetFormatDescription(sample) {
            for set in parameterSets(format, nalLengthSize: &nalLengthSize) {
                out.append(contentsOf: startCode)
                out.append(set)
            }
        }

        // Convert length-prefixed (AVCC/HVCC) NAL units to start-code-prefixed (Annex-B).
        let total = CMBlockBufferGetDataLength(block)
        var avcc = Data(count: total)
        let copied = avcc.withUnsafeMutableBytes {
            CMBlockBufferCopyDataBytes(block, atOffset: 0, dataLength: total, destination: $0.baseAddress!)
        }
        guard copied == kCMBlockBufferNoErr else { return }

        var offset = 0
        while offset + nalLengthSize <= total {
            var length = 0
            for i in 0..<nalLengthSize { length = length << 8 | Int(avcc[offset + i]) }
            offset += nalLengthSize
            guard length > 0, offset + length <= total else { break }
            out.append(contentsOf: startCode)
            out.append(avcc[offset ..< offset + length])
            offset += length
        }

        let pts = CMSampleBufferGetPresentationTimeStamp(sample)
        let micros = UInt64(max(0, CMTimeGetSeconds(pts)) * 1_000_000)

        statsLock.lock()
        frameCount += 1
        byteCount += out.count
        statsLock.unlock()

        onFrame?(keyframe, micros, out)
    }

    /// SPS/PPS (H.264) or VPS/SPS/PPS (HEVC) from the format description.
    private func parameterSets(_ format: CMFormatDescription, nalLengthSize: inout Int) -> [Data] {
        let isHEVC = CMFormatDescriptionGetMediaSubType(format) == kCMVideoCodecType_HEVC
        var count = 0
        var headerLength: Int32 = 4
        let status = isHEVC
            ? CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(format, parameterSetIndex: 0, parameterSetPointerOut: nil, parameterSetSizeOut: nil, parameterSetCountOut: &count, nalUnitHeaderLengthOut: &headerLength)
            : CMVideoFormatDescriptionGetH264ParameterSetAtIndex(format, parameterSetIndex: 0, parameterSetPointerOut: nil, parameterSetSizeOut: nil, parameterSetCountOut: &count, nalUnitHeaderLengthOut: &headerLength)
        guard status == noErr else { return [] }
        nalLengthSize = Int(headerLength)

        var sets: [Data] = []
        for i in 0..<count {
            var pointer: UnsafePointer<UInt8>?
            var size = 0
            let s = isHEVC
                ? CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(format, parameterSetIndex: i, parameterSetPointerOut: &pointer, parameterSetSizeOut: &size, parameterSetCountOut: nil, nalUnitHeaderLengthOut: nil)
                : CMVideoFormatDescriptionGetH264ParameterSetAtIndex(format, parameterSetIndex: i, parameterSetPointerOut: &pointer, parameterSetSizeOut: &size, parameterSetCountOut: nil, nalUnitHeaderLengthOut: nil)
            if s == noErr, let pointer { sets.append(Data(bytes: pointer, count: size)) }
        }
        return sets
    }
}
