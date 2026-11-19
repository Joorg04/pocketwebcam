package com.pocketwebcam.app

import android.Manifest
import android.app.Application
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.graphics.SurfaceTexture
import android.os.BatteryManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.PowerManager
import android.provider.Settings
import android.view.OrientationEventListener
import android.view.Surface
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableDoubleStateOf
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.core.content.ContextCompat
import androidx.core.content.edit
import androidx.lifecycle.AndroidViewModel
import com.pocketwebcam.app.audio.Microphone
import com.pocketwebcam.app.camera.CameraController
import com.pocketwebcam.app.encode.VideoEncoder
import com.pocketwebcam.app.net.AudioFormat
import com.pocketwebcam.app.net.CameraInfo
import com.pocketwebcam.app.net.Msg
import com.pocketwebcam.app.net.PairedComputers
import com.pocketwebcam.app.net.Pairing
import com.pocketwebcam.app.net.PairingEvent
import com.pocketwebcam.app.net.PhoneIdentity
import com.pocketwebcam.app.net.StreamConfig
import com.pocketwebcam.app.net.StreamFormat
import com.pocketwebcam.app.net.StreamServer
import com.pocketwebcam.app.net.Wire
import org.json.JSONObject
import java.net.Inet4Address
import java.net.NetworkInterface

/** What the pairing dialog shows. */
sealed class PairingPrompt(val finished: Boolean = false) {
    /** Scanned a computer's code; waiting for it to connect. */
    data class Waiting(val computer: String) : PairingPrompt()
    /** A computer asks to pair by comparing this code. */
    data class Code(val computer: String, val code: String) : PairingPrompt()
    /** Tapped Pair here; the computer's user still has to confirm. */
    data class Confirmed(val computer: String, val code: String) : PairingPrompt()
    data class Paired(val computer: String) : PairingPrompt(finished = true)
    data class Failed(val why: String) : PairingPrompt(finished = true)
}

/**
 * Wires camera -> encoder -> network and is the single source of truth for the UI.
 * Settings changes from the phone UI and from the desktop both go through [apply].
 */
class AppModel(private val app: Application) : AndroidViewModel(app) {
    val camera = CameraController(app)
    private val server = StreamServer(app)
    private val encoder = VideoEncoder(
        onFrame = { key, pts, data -> if (streaming) server.sendVideo(key, pts, data) },
        onFailed = { main.post { encoderFailed() } },
    )
    private val mic = Microphone(
        onChunk = { pts, pcm -> server.sendAudio(pts, pcm) },
        onLevel = { level -> main.post { micLevel = level } },
        onFailed = { main.post { micFailed() } },
    )
    private val prefs = app.getSharedPreferences("pocketwebcam", Context.MODE_PRIVATE)
    private val main = Handler(Looper.getMainLooper())

    /** What is actually streamed: [wanted], stepped down while the phone is hot. */
    var config by mutableStateOf(StreamConfig())
        private set
    var thermalLimit by mutableStateOf(ThermalLimit.None)
        private set
    var connectedTo by mutableStateOf<String?>(null)
        private set
    var listeningPort by mutableIntStateOf(0)
        private set
    var ipAddress by mutableStateOf<String?>(null)
        private set
    var thermal by mutableStateOf("nominal")
        private set
    var streamFps by mutableIntStateOf(0)
        private set
    var streamMbps by mutableDoubleStateOf(0.0)
        private set
    var hasPermission by mutableStateOf(false)
        private set
    var dimmed by mutableStateOf(false)
    /** How the phone is physically held (0, 90, 180, 270), for the UI. */
    var physicalOrientation by mutableIntStateOf(0)
        private set
    var deviceName by mutableStateOf("")
        private set
    /** True while microphone audio is actually being sent. */
    var micOn by mutableStateOf(false)
        private set
    /** Loudest recent mic sample, 0 to 1, for the level indicator. */
    var micLevel by mutableFloatStateOf(0f)
        private set
    /** `granted`, `denied` or `undetermined`, as STATE reports it. */
    var micPermission by mutableStateOf("undetermined")
        private set
    /** Set while the activity should show the microphone permission dialog. */
    var micPermissionNeeded by mutableStateOf(false)
        private set
    /** The pairing in progress, or how the last one ended; drives the pairing dialog. */
    var pairing by mutableStateOf<PairingPrompt?>(null)
    /** Paired computers as (fingerprint, name). */
    var pairedComputers by mutableStateOf<List<Pair<String, String>>>(emptyList())
        private set
    /** Set when this phone's key couldn't be made or read; nothing can connect then. */
    var identityError by mutableStateOf<String?>(null)
        private set
    private val paired = PairedComputers(app)

    /** What the phone's own controls and the desktop asked for. This is what's saved. */
    private var wanted = StreamConfig()
    /** The last mic on/off request for this connection. The mic starts off on every connection. */
    private var micWanted = false
    /** Restores the full stream once the phone has cooled down. */
    private val coolDown = Runnable { stepTo(ThermalLimit.forThermal(thermal)) }

    /** Random, made once: lets a computer recognise this phone after it's renamed. */
    private val deviceId: String = prefs.getString("deviceId", null)
        ?: java.util.UUID.randomUUID().toString().also { prefs.edit().putString("deviceId", it).apply() }

    @Volatile private var streaming = false
    @Volatile private var deviceOrientation = 0
    private var encoderRestarts = 0
    private var previewTexture: SurfaceTexture? = null
    private var encoderSurface: Surface? = null
    private var encoderSettings: List<Any>? = null
    private var visible = false

    private val orientation = object : OrientationEventListener(app) {
        override fun onOrientationChanged(degrees: Int) {
            if (degrees == ORIENTATION_UNKNOWN) return
            val snapped = ((degrees + 45) / 90 * 90) % 360
            if (snapped != deviceOrientation) {
                deviceOrientation = snapped
                main.post {
                    physicalOrientation = snapped
                    rotationChanged()
                }
            }
        }
    }

    init {
        deviceName = prefs.getString("deviceName", null)
            ?: Settings.Global.getString(app.contentResolver, "device_name")
            ?: Build.MODEL
        val saved = StreamConfig.fromJson(prefs.getString("config", null)) ?: StreamConfig()
        wanted = camera.resolve(saved)
        config = wanted
        refreshMicPermission()
        wire()
        pairedComputers = paired.all()
        try {
            server.start(deviceName, deviceId, PhoneIdentity.loadOrCreate(), paired)
        } catch (e: Exception) {
            identityError = "Pocket Webcam couldn't create this phone's security key (${e.message}). Restart the app; if it keeps happening, reinstall it."
        }
        orientation.enable()
        ipAddress = localAddress()
        if (Build.VERSION.SDK_INT >= 29) {
            val pm = app.getSystemService(Context.POWER_SERVICE) as PowerManager
            thermalChanged(pm.currentThermalStatus)
            pm.addThermalStatusListener { status -> main.post { thermalChanged(status) } }
        }
        main.post(object : Runnable {
            override fun run() {
                val (frames, bytes) = encoder.takeStats()
                streamFps = frames.toInt()
                streamMbps = bytes * 8 / 1_000_000.0
                ipAddress = localAddress()
                main.postDelayed(this, 1000)
            }
        })
    }

    // region Lifecycle

    fun onPermission(granted: Boolean) {
        hasPermission = granted
        if (granted && visible) pushCamera()
        updateMicrophone()
    }

    fun onVisible(isVisible: Boolean) {
        visible = isVisible
        // Android silences the mic in the background anyway; stop it so the privacy dot goes too.
        updateMicrophone()
        if (isVisible) {
            if (hasPermission) {
                if (streaming) restartPipeline() else pushCamera()
            }
        } else {
            camera.stop()
            encoder.stop()
            encoderSurface = null
            encoderSettings = null
        }
    }

    fun setPreviewTexture(texture: SurfaceTexture?) {
        previewTexture = texture
        if (visible && hasPermission) {
            if (streaming && encoderSurface == null) restartPipeline() else pushCamera()
        }
    }

    private fun pushCamera() {
        camera.start(config, previewTexture, encoderSurface)
    }

    // endregion

    // region Settings

    fun rename(name: String) {
        val clean = name.trim().ifEmpty { Build.MODEL }
        deviceName = clean
        prefs.edit { putString("deviceName", clean) }
        server.rename(clean)
    }

    /** Merges a change into the config, lets the camera clamp it, then tells everyone. */
    fun apply(change: (StreamConfig) -> StreamConfig) {
        var requested = change(wanted)
        requested = requested.copy(
            bitrate = requested.bitrate.coerceIn(camera.capabilities.bitrateMin, camera.capabilities.bitrateMax),
        )
        wanted = camera.resolve(requested)
        prefs.edit { putString("config", wanted.toJson().toString()) }
        reconfigure()
    }

    /** Runs [wanted], stepped down while the phone is hot, and tells everyone. */
    private fun reconfigure() {
        val before = config
        val capped = camera.resolve(thermalLimit.cap(wanted))
        val applied = capped.copy(
            bitrate = ThermalLimit.bitrate(wanted, capped).coerceIn(camera.capabilities.bitrateMin, wanted.bitrate),
        )
        config = applied

        val settings = encodeSettings(applied)
        when {
            !visible || !hasPermission -> {}
            streaming && settings != encoderSettings -> restartPipeline()
            else -> {
                if (streaming && applied.bitrate != before.bitrate) encoder.setBitrate(applied.bitrate)
                if (streaming && applied.mirror != before.mirror) {
                    server.sendStreamFormat(streamFormat())
                    encoder.requestKeyframe()
                }
                pushCamera()
            }
        }
        sendState()
    }

    fun applyRemote(update: JSONObject) = main.post {
        if (update.has("audio")) setMicrophone(update.optBoolean("audio"))
        apply { it.merged(update) }
    }

    fun selectLens(info: CameraInfo) = apply { it.copy(camera = info.id, zoom = 1.0) }
    fun setResolution(w: Int, h: Int) = apply { it.copy(width = w, height = h) }
    fun setFps(fps: Int) = apply { it.copy(fps = fps) }
    fun setQuality(q: Quality) = apply { it.copy(bitrate = q.bitrate(it)) }
    fun setCodec(codec: String) = apply { it.copy(codec = codec) }
    fun setTorch(on: Boolean) = apply { it.copy(torch = on) }
    fun setMirror(on: Boolean) = apply { it.copy(mirror = on) }
    fun setStabilization(on: Boolean) = apply { it.copy(stabilization = on) }
    fun setZoom(zoom: Double) = apply { it.copy(zoom = zoom) }

    val currentLens: CameraController.Lens? get() = camera.lensFor(config.camera)

    val quality: Quality? get() = Quality.entries.firstOrNull { it.bitrate(config) == config.bitrate }

    /** Label for lens buttons: "0.5×", "1×", "3×" or "Front". */
    fun lensLabel(lens: CameraController.Lens): String = when {
        lens.front -> "Front"
        lens.baseRatio < 1 -> "%.1f×".format(java.util.Locale.ROOT, lens.baseRatio).replace("0.", ".")
        else -> "${lens.baseRatio.toInt()}×".let { if (lens.baseRatio % 1.0 != 0.0) "%.1f×".format(java.util.Locale.ROOT, lens.baseRatio) else it }
    }

    // endregion

    // region Microphone

    /**
     * Turns the mic on or off for this connection (from the computer or the phone's own
     * button). Asks for permission the first time; a denial is reported in STATE.
     */
    fun setMicrophone(on: Boolean) {
        micWanted = on && connectedTo != null
        updateMicrophone()
    }

    /** The permission dialog closed, whatever the answer. */
    fun onMicPermissionResult() {
        micPermissionNeeded = false
        prefs.edit { putBoolean("micAsked", true) }
        updateMicrophone()
    }

    private fun refreshMicPermission() {
        micPermission = when {
            ContextCompat.checkSelfPermission(app, Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED -> "granted"
            prefs.getBoolean("micAsked", false) -> "denied"
            else -> "undetermined"
        }
    }

    /** Records exactly while the mic is wanted, allowed, a computer is connected and the app is on screen. */
    private fun updateMicrophone() {
        val before = micOn to micPermission
        refreshMicPermission()
        val active = micWanted && connectedTo != null && visible && camera.capabilities.audio
        // Ask on first use, over the camera rather than over the camera permission screen.
        if (active && hasPermission && micPermission == "undetermined") micPermissionNeeded = true
        val run = active && micPermission == "granted"
        if (run && !micOn) startMicrophone() else if (!run && micOn) stopMicrophone()
        if ((micOn to micPermission) != before) sendState()
    }

    private fun startMicrophone() {
        if (ContextCompat.checkSelfPermission(app, Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) return
        // Queued before any AUDIO_FRAME, so the receiver knows the format first.
        server.send(Msg.AUDIO_FORMAT, AudioFormat.toJson())
        micOn = try {
            mic.start(bootTime = currentLens?.bootTimeClock == true)
        } catch (e: SecurityException) {
            android.util.Log.e("PocketWebcam", "microphone not allowed", e)
            false
        }
        if (!micOn) android.util.Log.e("PocketWebcam", "microphone unavailable")
    }

    private fun stopMicrophone() {
        mic.stop()
        micOn = false
        micLevel = 0f
    }

    /** The mic stopped by itself (taken by another app or failed): report it as off. */
    private fun micFailed() {
        micWanted = false
        updateMicrophone()
    }

    // endregion

    // region Thermal

    /**
     * Steps the stream down as soon as the phone gets hot and back up once it has stayed
     * cooler for a minute, so a phone hovering at a threshold doesn't restart the encoder
     * again and again.
     */
    private fun thermalChanged(status: Int) {
        thermal = thermalName(status)
        val limit = ThermalLimit.forThermal(thermal)
        main.removeCallbacks(coolDown)
        if (limit > thermalLimit) {
            stepTo(limit)
            return
        }
        if (limit < thermalLimit) main.postDelayed(coolDown, COOL_DOWN_MS)
        sendState()
    }

    private fun stepTo(limit: ThermalLimit) {
        if (limit == thermalLimit) return
        android.util.Log.i("PocketWebcam", "thermal $thermal: stream limit $limit")
        thermalLimit = limit
        reconfigure()
    }

    // endregion

    // region Streaming

    private fun wire() {
        server.onNeedKeyframe = { encoder.requestKeyframe() }
        server.onKeyframeRequest = { encoder.requestKeyframe() }
        server.onListening = { port -> main.post { listeningPort = port } }
        server.onHello = { hello -> main.post { receiverConnected(hello) } }
        server.onPairing = { event -> main.post { pairingEvent(event) } }
        server.onDisconnected = { main.post { receiverDisconnected() } }
        server.onSetConfig = { update -> applyRemote(update) }
    }

    // region Pairing

    /** A scanned QR code or an opened `pocketwebcam://pair` link. False if it isn't one. */
    fun handlePairingLink(link: String): Boolean {
        val scan = Pairing.parse(link)
        if (scan == null) {
            if (link.startsWith("pocketwebcam://")) {
                pairing = PairingPrompt.Failed("This code is from a different version of Pocket Webcam. Update the app on this phone and on the computer.")
            }
            return false
        }
        server.scanned(scan)
        pairing = PairingPrompt.Waiting(scan.name)
        return true
    }

    /** The user's answer to "does the computer show this code?". */
    fun answerCode(yes: Boolean) {
        server.answerCode(yes)
        val p = pairing
        pairing = if (yes && p is PairingPrompt.Code) PairingPrompt.Confirmed(p.computer, p.code) else if (!yes) null else p
    }

    fun cancelPairing() {
        if (pairing is PairingPrompt.Code || pairing is PairingPrompt.Confirmed) server.answerCode(false)
        server.cancelScan()
        pairing = null
    }

    fun forgetComputer(fingerprint: String) {
        paired.forget(fingerprint)
        pairedComputers = paired.all()
    }

    private fun pairingEvent(event: PairingEvent) {
        when (event) {
            is PairingEvent.CodeRequested -> pairing = PairingPrompt.Code(event.computer, event.code)
            is PairingEvent.Paired -> {
                pairedComputers = paired.all()
                pairing = PairingPrompt.Paired(event.computer)
            }
            // Only worth showing while the user is in the middle of pairing.
            is PairingEvent.Failed -> if (pairing != null && pairing?.finished != true) pairing = PairingPrompt.Failed(event.why)
        }
    }

    // endregion

    private fun receiverConnected(hello: JSONObject) {
        connectedTo = hello.optString("name", "computer")
        server.send(
            Msg.HELLO,
            JSONObject()
                .put("protocol", Wire.PROTOCOL).put("name", deviceName).put("app", "Pocket Webcam")
                .put("appVersion", BuildConfig.VERSION_NAME).put("platform", "android")
                .put("id", deviceId),
        )
        server.send(Msg.CAPABILITIES, camera.capabilities.toJson())
        micWanted = false
        sendState()
        streaming = true
        encoderRestarts = 0
        if (visible && hasPermission) restartPipeline()
    }

    /** The hardware encoder died mid-stream: fall back to H.264, then retry a few times. */
    private fun encoderFailed() {
        if (!streaming || !visible || !hasPermission) return
        encoderSurface = null
        encoderSettings = null
        if (++encoderRestarts > MAX_ENCODER_RESTARTS) {
            android.util.Log.e("PocketWebcam", "encoder keeps failing, giving up until the receiver reconnects")
            return
        }
        if (config.codec != "h264") apply { it.copy(codec = "h264") } else restartPipeline()
    }

    private fun receiverDisconnected() {
        micWanted = false
        streaming = false
        connectedTo = null
        updateMicrophone()
        dimmed = false
        if (visible && hasPermission) {
            // Drop the encoder only after the camera stopped writing to it.
            encoderSurface = null
            camera.start(config, previewTexture, null) {
                encoder.stop()
                encoderSettings = null
            }
        } else {
            encoder.stop()
            encoderSurface = null
            encoderSettings = null
        }
    }

    /** Rebuilds the encoder (new size, rate or codec) and points the camera at it. */
    private fun restartPipeline() {
        val cfg = config
        encoderSurface = null
        camera.start(cfg, previewTexture, null) {
            val surface = try {
                encoder.start(VideoEncoder.mime(cfg.codec), cfg.width, cfg.height, cfg.fps, cfg.bitrate)
            } catch (e: Exception) {
                android.util.Log.e("PocketWebcam", "encoder start failed", e)
                if (cfg.codec != "h264") main.post { apply { it.copy(codec = "h264") } }
                return@start
            }
            encoderSurface = surface
            encoderSettings = encodeSettings(cfg)
            // STREAM_FORMAT must reach the socket before this stream's first frame.
            server.sendStreamFormat(streamFormat())
            camera.start(cfg, previewTexture, surface) { encoder.requestKeyframe() }
        }
    }

    private fun encodeSettings(c: StreamConfig): List<Any> = listOf(c.codec, c.width, c.height, c.fps)

    /** Clockwise degrees the receiver must rotate frames to be upright, from gravity. */
    private fun streamRotation(): Int {
        val lens = camera.lensFor(config.camera) ?: return 0
        val o = deviceOrientation
        return if (lens.front) (lens.sensorOrientation - o + 360) % 360 else (lens.sensorOrientation + o) % 360
    }

    private fun streamFormat() = StreamFormat(config.codec, config.width, config.height, config.fps, streamRotation(), config.mirror)

    private fun rotationChanged() {
        if (streaming && encoderSurface != null) {
            server.sendStreamFormat(streamFormat())
            encoder.requestKeyframe()
        }
    }

    private fun sendState() {
        if (connectedTo == null) return
        val battery = app.registerReceiver(null, IntentFilter(Intent.ACTION_BATTERY_CHANGED))
        val level = battery?.getIntExtra(BatteryManager.EXTRA_LEVEL, -1) ?: -1
        val scale = battery?.getIntExtra(BatteryManager.EXTRA_SCALE, -1) ?: -1
        val status = battery?.getIntExtra(BatteryManager.EXTRA_STATUS, -1) ?: -1
        val json = config.toJson().put("thermal", thermal)
        if (level >= 0 && scale > 0) json.put("battery", level.toDouble() / scale)
        json.put("charging", status == BatteryManager.BATTERY_STATUS_CHARGING || status == BatteryManager.BATTERY_STATUS_FULL)
        json.put("audio", micOn).put("micPermission", micPermission)
        server.send(Msg.STATE, json)
    }

    // endregion

    override fun onCleared() {
        main.removeCallbacks(coolDown)
        mic.stop()
        orientation.disable()
        server.stop()
        encoder.stop()
        camera.stop()
    }

    private fun thermalName(status: Int) = when (status) {
        PowerManager.THERMAL_STATUS_NONE, PowerManager.THERMAL_STATUS_LIGHT -> "nominal"
        PowerManager.THERMAL_STATUS_MODERATE -> "fair"
        PowerManager.THERMAL_STATUS_SEVERE -> "serious"
        else -> "critical"
    }

    private fun localAddress(): String? = try {
        NetworkInterface.getNetworkInterfaces().toList()
            .filter { it.isUp && !it.isLoopback }
            .flatMap { it.inetAddresses.toList() }
            .filterIsInstance<Inet4Address>()
            .firstOrNull { it.isSiteLocalAddress }
            ?.hostAddress
    } catch (_: Exception) {
        null
    }

    private companion object {
        const val MAX_ENCODER_RESTARTS = 3
        const val COOL_DOWN_MS = 60_000L
    }
}

/** Bitrate presets shown in the UI, scaled by resolution, frame rate and codec. */
enum class Quality(val label: String, private val bitsPerPixel: Double) {
    Low("Low", 0.05), Standard("Standard", 0.1), High("High", 0.16), Max("Max", 0.25);

    fun bitrate(c: StreamConfig): Int {
        val codecFactor = if (c.codec == "hevc") 0.7 else 1.0
        // Frame rate above 30 costs less than linearly: motion between frames is smaller.
        val fpsFactor = if (c.fps <= 30) c.fps.toDouble() else 30 + (c.fps - 30) * 0.5
        val mbps = Math.round(c.width.toDouble() * c.height * fpsFactor * bitsPerPixel * codecFactor / 1_000_000)
        return mbps.coerceIn(1, 50).toInt() * 1_000_000
    }
}
