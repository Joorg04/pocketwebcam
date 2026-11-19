package com.pocketwebcam.app.net

import org.json.JSONArray
import org.json.JSONObject
import java.nio.ByteBuffer

/** Message types from PROTOCOL.md. */
object Msg {
    const val HELLO = 0x01
    const val CAPABILITIES = 0x02
    const val SET_CONFIG = 0x03
    const val STATE = 0x04
    const val STREAM_FORMAT = 0x10
    const val VIDEO_FRAME = 0x11
    const val KEYFRAME_REQ = 0x12
    const val AUDIO_FORMAT = 0x13
    const val AUDIO_FRAME = 0x14
    const val PAIR_REQUEST = 0x30
    const val PAIR_PROOF = 0x31
    const val PAIR_RESULT = 0x32
    const val PING = 0x20
    const val PONG = 0x21
    const val ERROR = 0x7F
}

object Wire {
    /** v2: TLS 1.3 between paired devices only (PROTOCOL.md, "Encryption"). */
    const val PROTOCOL = 2
    const val SERVICE_TYPE = "_pocketwebcam._tcp"
    const val DEFAULT_PORT = 7310
    const val MAX_PAYLOAD = 16 * 1024 * 1024

    fun frame(type: Int, payload: ByteArray = ByteArray(0)): ByteArray {
        val out = ByteArray(5 + payload.size)
        out[0] = type.toByte()
        ByteBuffer.wrap(out).putInt(1, payload.size)
        System.arraycopy(payload, 0, out, 5, payload.size)
        return out
    }

    fun frame(type: Int, json: JSONObject): ByteArray = frame(type, json.toString().toByteArray(Charsets.UTF_8))

    /** VIDEO_FRAME payload: flags, 8-byte big-endian pts in microseconds, Annex-B data. */
    fun videoFrame(keyframe: Boolean, ptsMicros: Long, annexB: ByteArray): ByteArray {
        val out = ByteArray(5 + 9 + annexB.size)
        val buf = ByteBuffer.wrap(out)
        buf.put(0, Msg.VIDEO_FRAME.toByte())
        buf.putInt(1, 9 + annexB.size)
        buf.put(5, if (keyframe) 1 else 0)
        buf.putLong(6, ptsMicros)
        System.arraycopy(annexB, 0, out, 14, annexB.size)
        return out
    }

    /** AUDIO_FRAME payload: 8-byte big-endian pts in microseconds, then little-endian samples. */
    fun audioFrame(ptsMicros: Long, samples: ByteArray): ByteArray {
        val out = ByteArray(5 + 8 + samples.size)
        val buf = ByteBuffer.wrap(out)
        buf.put(0, Msg.AUDIO_FRAME.toByte())
        buf.putInt(1, 8 + samples.size)
        buf.putLong(5, ptsMicros)
        System.arraycopy(samples, 0, out, 13, samples.size)
        return out
    }
}

data class FormatInfo(val width: Int, val height: Int, val fps: List<Int>)

data class CameraInfo(
    val id: String,
    val name: String,
    val position: String,
    val lens: String,
    val zoomMin: Double,
    val zoomMax: Double,
    val formats: List<FormatInfo>,
    val features: List<String>,
) {
    fun toJson() = JSONObject()
        .put("id", id).put("name", name).put("position", position).put("lens", lens)
        .put("zoom", JSONObject().put("min", zoomMin).put("max", zoomMax))
        .put("formats", JSONArray(formats.map {
            JSONObject().put("width", it.width).put("height", it.height).put("fps", JSONArray(it.fps))
        }))
        .put("features", JSONArray(features))
}

data class Capabilities(
    val cameras: List<CameraInfo>,
    val codecs: List<String>,
    val bitrateMin: Int = 1_000_000,
    val bitrateMax: Int = 50_000_000,
    /** Whether the phone can send its microphone. Without one, `audio` is left out. */
    val audio: Boolean = false,
) {
    fun toJson(): JSONObject {
        val json = JSONObject()
            .put("cameras", JSONArray(cameras.map { it.toJson() }))
            .put("codecs", JSONArray(codecs))
            .put("bitrate", JSONObject().put("min", bitrateMin).put("max", bitrateMax))
        if (audio) json.put("audio", AudioFormat.CAPABILITIES)
        return json
    }
}

/** The phone's full stream configuration. Persisted between launches. */
data class StreamConfig(
    val camera: String = "",
    val width: Int = 1920,
    val height: Int = 1080,
    val fps: Int = 30,
    val codec: String = "h264",
    val bitrate: Int = 10_000_000,
    val zoom: Double = 1.0,
    val torch: Boolean = false,
    val stabilization: Boolean = false,
    val mirror: Boolean = false,
) {
    fun toJson() = JSONObject()
        .put("camera", camera).put("width", width).put("height", height).put("fps", fps)
        .put("codec", codec).put("bitrate", bitrate).put("zoom", zoom)
        .put("torch", torch).put("stabilization", stabilization).put("mirror", mirror)

    /** SET_CONFIG is a partial update: only keys that are present change. */
    fun merged(update: JSONObject) = copy(
        camera = if (update.has("camera")) update.optString("camera", camera) else camera,
        width = if (update.has("width")) update.optInt("width", width) else width,
        height = if (update.has("height")) update.optInt("height", height) else height,
        fps = if (update.has("fps")) update.optInt("fps", fps) else fps,
        codec = if (update.has("codec")) update.optString("codec", codec) else codec,
        bitrate = if (update.has("bitrate")) update.optInt("bitrate", bitrate) else bitrate,
        zoom = if (update.has("zoom")) update.optDouble("zoom", zoom) else zoom,
        torch = if (update.has("torch")) update.optBoolean("torch", torch) else torch,
        stabilization = if (update.has("stabilization")) update.optBoolean("stabilization", stabilization) else stabilization,
        mirror = if (update.has("mirror")) update.optBoolean("mirror", mirror) else mirror,
    )

    companion object {
        fun fromJson(s: String?): StreamConfig? = try {
            if (s == null) null else StreamConfig().merged(JSONObject(s))
        } catch (_: Exception) {
            null
        }
    }
}

/** STREAM_FORMAT. `mirror` is an additive field: the receiver flips the upright image horizontally. */
data class StreamFormat(
    val codec: String,
    val width: Int,
    val height: Int,
    val fps: Int,
    val rotation: Int,
    val mirror: Boolean,
) {
    fun toJson() = JSONObject()
        .put("codec", codec).put("width", width).put("height", height).put("fps", fps)
        .put("rotation", rotation).put("mirror", mirror)
}

/** AUDIO_FORMAT: shape of the AUDIO_FRAME samples that follow. v1 only has this one. */
object AudioFormat {
    const val SAMPLE_RATE = 48_000
    const val CHANNELS = 1
    /** 10 ms per AUDIO_FRAME. */
    const val CHUNK_SAMPLES = SAMPLE_RATE / 100

    fun toJson(): JSONObject = JSONObject().put("codec", "pcm_s16le").put("sampleRate", SAMPLE_RATE).put("channels", CHANNELS)

    val CAPABILITIES: JSONObject
        get() = JSONObject()
            .put("codecs", JSONArray(listOf("pcm_s16le")))
            .put("sampleRates", JSONArray(listOf(SAMPLE_RATE)))
            .put("channels", JSONArray(listOf(CHANNELS)))
}
