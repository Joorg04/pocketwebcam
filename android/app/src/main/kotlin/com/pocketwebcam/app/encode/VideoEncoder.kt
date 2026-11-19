package com.pocketwebcam.app.encode

import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaCodecList
import android.media.MediaFormat
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.HandlerThread
import android.util.Log
import android.view.Surface
import java.util.concurrent.atomic.AtomicLong

/**
 * Hardware H.264/HEVC encoder fed by a camera Surface. Emits Annex-B access units with
 * parameter sets in-band on every keyframe, as PROTOCOL.md requires.
 */
class VideoEncoder(
    private val onFrame: (keyframe: Boolean, ptsMicros: Long, annexB: ByteArray) -> Unit,
    /** The running encoder failed; it has been stopped and needs a restart. */
    private val onFailed: () -> Unit = {},
) {
    private val thread = HandlerThread("pw-encoder").also { it.start() }
    private val handler = Handler(thread.looper)
    private var codec: MediaCodec? = null
    private var parameterSets: ByteArray? = null
    private val frames = AtomicLong()
    private val bytes = AtomicLong()

    val isRunning: Boolean get() = codec != null

    /** Starts encoding and returns the Surface the camera must render into. */
    @Synchronized
    fun start(mime: String, width: Int, height: Int, fps: Int, bitrate: Int): Surface {
        stop()
        val c = MediaCodec.createEncoderByType(mime)
        // Not every hardware encoder does constant bitrate; asking for it anyway makes configure() fail.
        val cbr = runCatching {
            c.codecInfo.getCapabilitiesForType(mime).encoderCapabilities
                ?.isBitrateModeSupported(MediaCodecInfo.EncoderCapabilities.BITRATE_MODE_CBR) == true
        }.getOrDefault(false)
        val format = MediaFormat.createVideoFormat(mime, width, height).apply {
            setInteger(MediaFormat.KEY_COLOR_FORMAT, MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface)
            setInteger(MediaFormat.KEY_BIT_RATE, bitrate)
            setInteger(MediaFormat.KEY_FRAME_RATE, fps)
            setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 1)
            setInteger(
                MediaFormat.KEY_BITRATE_MODE,
                if (cbr) MediaCodecInfo.EncoderCapabilities.BITRATE_MODE_CBR else MediaCodecInfo.EncoderCapabilities.BITRATE_MODE_VBR,
            )
            setInteger(MediaFormat.KEY_PRIORITY, 0) // real time
            if (Build.VERSION.SDK_INT >= 30) setInteger(MediaFormat.KEY_LATENCY, 1)
            if (Build.VERSION.SDK_INT >= 29) setInteger(MediaFormat.KEY_MAX_B_FRAMES, 0)
        }
        Log.i(TAG, "encoder ${c.name}: $mime ${width}x$height@$fps ${bitrate / 1_000_000} Mbps ${if (cbr) "CBR" else "VBR"}")
        parameterSets = null
        c.setCallback(object : MediaCodec.Callback() {
            override fun onInputBufferAvailable(codec: MediaCodec, index: Int) {}

            override fun onOutputBufferAvailable(codec: MediaCodec, index: Int, info: MediaCodec.BufferInfo) {
                try {
                    val buffer = codec.getOutputBuffer(index)
                    if (buffer != null && info.size > 0) {
                        buffer.position(info.offset)
                        buffer.limit(info.offset + info.size)
                        val data = ByteArray(info.size)
                        buffer.get(data)
                        handleOutput(data, info)
                    }
                } finally {
                    try { codec.releaseOutputBuffer(index, false) } catch (_: IllegalStateException) {}
                }
            }

            override fun onError(codec: MediaCodec, e: MediaCodec.CodecException) {
                Log.e(TAG, "encoder error (transient=${e.isTransient})", e)
                if (e.isTransient) return
                val current = synchronized(this@VideoEncoder) { this@VideoEncoder.codec === codec }
                if (current) {
                    stop()
                    onFailed()
                }
            }

            override fun onOutputFormatChanged(codec: MediaCodec, format: MediaFormat) {}
        }, handler)
        val surface = try {
            c.configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
            c.createInputSurface().also { c.start() }
        } catch (e: Exception) {
            c.release()
            throw e
        }
        codec = c
        return surface
    }

    private fun handleOutput(data: ByteArray, info: MediaCodec.BufferInfo) {
        if (info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG != 0) {
            parameterSets = data
            return
        }
        val key = info.flags and MediaCodec.BUFFER_FLAG_KEY_FRAME != 0
        val out = if (key && parameterSets != null) parameterSets!! + data else data
        frames.incrementAndGet()
        bytes.addAndGet(out.size.toLong())
        onFrame(key, info.presentationTimeUs, out)
    }

    @Synchronized
    fun stop() {
        val c = codec ?: return
        codec = null
        try { c.stop() } catch (_: Exception) {}
        try { c.release() } catch (_: Exception) {}
    }

    @Synchronized
    fun setBitrate(bps: Int) {
        codec?.setParameters(Bundle().apply { putInt(MediaCodec.PARAMETER_KEY_VIDEO_BITRATE, bps) })
    }

    @Synchronized
    fun requestKeyframe() {
        codec?.setParameters(Bundle().apply { putInt(MediaCodec.PARAMETER_KEY_REQUEST_SYNC_FRAME, 0) })
    }

    /** Frames and bytes since the last call. */
    fun takeStats(): Pair<Long, Long> = frames.getAndSet(0) to bytes.getAndSet(0)

    companion object {
        private const val TAG = "PocketWebcamEnc"

        fun mime(codec: String) = if (codec == "hevc") "video/hevc" else "video/avc"

        private fun hasEncoder(mime: String) = MediaCodecList(MediaCodecList.REGULAR_CODECS).codecInfos
            .any { it.isEncoder && it.supportedTypes.any { t -> t.equals(mime, ignoreCase = true) } }

        /** Codecs this phone can encode, most compatible first. */
        fun supportedCodecs(): List<String> = buildList {
            add("h264")
            if (hasEncoder("video/hevc")) add("hevc")
        }

        /** Whether the H.264 encoder accepts this size and rate. */
        fun supports(width: Int, height: Int, fps: Int): Boolean {
            val caps = MediaCodecList(MediaCodecList.REGULAR_CODECS).codecInfos
                .filter { it.isEncoder && it.supportedTypes.any { t -> t.equals("video/avc", true) } }
                .mapNotNull { runCatching { it.getCapabilitiesForType("video/avc").videoCapabilities }.getOrNull() }
            return caps.any { it.areSizeAndRateSupported(width, height, fps.toDouble()) }
        }
    }
}
