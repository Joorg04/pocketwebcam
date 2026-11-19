package com.pocketwebcam.app.camera

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.graphics.Rect
import android.graphics.SurfaceTexture
import android.hardware.camera2.CameraAccessException
import android.hardware.camera2.CameraCaptureSession
import android.hardware.camera2.CameraCharacteristics
import android.hardware.camera2.CameraDevice
import android.hardware.camera2.CameraManager
import android.hardware.camera2.CameraMetadata
import android.hardware.camera2.CaptureRequest
import android.media.MediaCodec
import android.os.Build
import android.os.Handler
import android.os.HandlerThread
import android.util.Log
import android.util.Range
import android.util.Size
import android.view.Surface
import androidx.core.content.ContextCompat
import com.pocketwebcam.app.encode.VideoEncoder
import com.pocketwebcam.app.net.CameraInfo
import com.pocketwebcam.app.net.Capabilities
import com.pocketwebcam.app.net.FormatInfo
import com.pocketwebcam.app.net.StreamConfig
import kotlin.math.abs
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt

/**
 * Owns the Camera2 device and capture session: lens discovery, formats, frame rate,
 * zoom, torch and stabilization. Frames go to an optional preview and an optional
 * encoder Surface.
 *
 * Lenses are "virtual": the phone's main back camera is usually a logical multi-camera
 * that switches between ultra wide / wide / telephoto sensors by zoom ratio, so each
 * lens is that camera plus a base zoom ratio.
 */
class CameraController(private val context: Context) {
    class Lens(
        val info: CameraInfo,
        val cameraId: String,
        /** Zoom ratio that selects this lens (relative to the main wide camera). */
        val baseRatio: Double,
        val front: Boolean,
        val sensorOrientation: Int,
        /** Smallest/largest zoom ratio the camera accepts; null when it has no zoom-ratio control. */
        val ratioRange: ClosedFloatingPointRange<Double>?,
        val maxDigitalZoom: Double,
        val fpsRanges: List<Range<Int>>,
        val torch: Boolean,
        val stabilization: Boolean,
        val opticalStabilization: Boolean,
        val activeArray: Rect?,
        /** Frames are stamped with elapsedRealtimeNanos (boot time) instead of System.nanoTime. */
        val bootTimeClock: Boolean,
    )

    private val manager = context.getSystemService(Context.CAMERA_SERVICE) as CameraManager
    val lenses: List<Lens> = discoverLenses()
    val capabilities = Capabilities(
        lenses.map { it.info }, VideoEncoder.supportedCodecs(),
        audio = context.packageManager.hasSystemFeature(PackageManager.FEATURE_MICROPHONE),
    )

    private val thread = HandlerThread("pw-camera").also { it.start() }
    private val handler = Handler(thread.looper)

    // Everything below is touched only on `handler`.
    private data class Desired(
        val config: StreamConfig,
        val texture: SurfaceTexture?,
        val encoder: Surface?,
        val onApplied: (() -> Unit)?,
    )

    private var desired: Desired? = null
    private var device: CameraDevice? = null
    private var deviceId: String? = null
    private var opening = false
    private var session: CameraCaptureSession? = null
    private var sessionKey: List<Any?>? = null
    private var creatingSession = false
    private var previewSurface: Surface? = null
    private var previewTexture: SurfaceTexture? = null
    private var previewDisabled = false

    // region Capabilities

    fun lensFor(id: String): Lens? = lenses.firstOrNull { it.info.id == id }

    /** The default lens: the main back camera, or whatever exists. */
    fun defaultLens(): Lens? = lenses.firstOrNull { !it.front && abs(it.baseRatio - 1.0) < 0.01 } ?: lenses.firstOrNull()

    /** Clamps `requested` to what this phone can do. Pure: touches no hardware. */
    fun resolve(requested: StreamConfig): StreamConfig {
        val lens = lensFor(requested.camera) ?: defaultLens() ?: return requested
        var c = requested.copy(camera = lens.info.id)

        val formats = lens.info.formats
        if (formats.isNotEmpty()) {
            val exact = formats.firstOrNull { it.width == c.width && it.height == c.height }
            // Never round up: a bigger size than asked for only costs battery and heat.
            val chosen = exact
                ?: formats.filter { it.width * it.height <= c.width * c.height }.maxByOrNull { it.width * it.height }
                ?: formats.minByOrNull { it.width * it.height }!!
            val fps = if (c.fps in chosen.fps) c.fps
            else chosen.fps.filter { it <= c.fps }.maxOrNull() ?: chosen.fps.minOrNull() ?: 30
            c = c.copy(width = chosen.width, height = chosen.height, fps = fps)
        }
        c = c.copy(zoom = c.zoom.coerceIn(lens.info.zoomMin, lens.info.zoomMax))
        if (!lens.torch) c = c.copy(torch = false)
        if (!lens.stabilization) c = c.copy(stabilization = false)
        if (c.codec !in capabilities.codecs) c = c.copy(codec = "h264")
        return c
    }

    private fun discoverLenses(): List<Lens> {
        val result = mutableListOf<Lens>()
        val ids = try { manager.cameraIdList.toList() } catch (_: CameraAccessException) { emptyList() }
        fun chars(id: String) = manager.getCameraCharacteristics(id)
        fun physicalIds(id: String): Set<String> =
            if (Build.VERSION.SDK_INT >= 28) chars(id).physicalCameraIds else emptySet()

        val back = ids.filter { chars(it).get(CameraCharacteristics.LENS_FACING) == CameraCharacteristics.LENS_FACING_BACK }
        val front = ids.filter { chars(it).get(CameraCharacteristics.LENS_FACING) == CameraCharacteristics.LENS_FACING_FRONT }

        back.maxByOrNull { physicalIds(it).size }?.let { id ->
            val ch = chars(id)
            val range = zoomRatioRange(ch)
            val ratios = lensRatios(id, physicalIds(id), range)
            for (ratio in ratios) result += buildLens(id, ch, ratio, front = false)
        }
        front.firstOrNull()?.let { id -> result += buildLens(id, chars(id), 1.0, front = true) }
        return result
    }

    private fun zoomRatioRange(ch: CameraCharacteristics): ClosedFloatingPointRange<Double>? {
        if (Build.VERSION.SDK_INT < 30) return null
        val r = ch.get(CameraCharacteristics.CONTROL_ZOOM_RATIO_RANGE) ?: return null
        return r.lower.toDouble()..r.upper.toDouble()
    }

    /** Ratios (1.0 = main camera) of the lenses reachable by zoom on this logical camera. */
    private fun lensRatios(id: String, physical: Set<String>, range: ClosedFloatingPointRange<Double>?): List<Double> {
        if (range == null || physical.size < 2) return listOf(1.0)
        data class Phys(val equiv: Double)
        val phys = physical.mapNotNull { pid ->
            val pc = manager.getCameraCharacteristics(pid)
            val focal = pc.get(CameraCharacteristics.LENS_INFO_AVAILABLE_FOCAL_LENGTHS)?.firstOrNull() ?: return@mapNotNull null
            val width = pc.get(CameraCharacteristics.SENSOR_INFO_PHYSICAL_SIZE)?.width ?: return@mapNotNull null
            val equiv = focal * 36.0 / width
            if (equiv in 10.0..250.0) Phys(equiv) else null
        }
        if (phys.isEmpty()) return listOf(1.0)
        val main = phys.minByOrNull { abs(it.equiv - 26.0) }!!.equiv
        val snapped = phys.map { snapRatio(it.equiv / main) }.toSortedSet()
        snapped += 1.0
        return snapped.filter { it >= range.start - 0.05 && it <= range.endInclusive }
            .map { max(it, range.start) }
            .ifEmpty { listOf(1.0) }
    }

    private fun snapRatio(r: Double): Double = when {
        r < 0.9 -> (r * 10).roundToInt() / 10.0
        r < 1.25 -> 1.0
        else -> (r * 2).roundToInt() / 2.0
    }

    private fun buildLens(id: String, ch: CameraCharacteristics, ratio: Double, front: Boolean): Lens {
        val range = zoomRatioRange(ch)
        val digital = (ch.get(CameraCharacteristics.SCALER_AVAILABLE_MAX_DIGITAL_ZOOM) ?: 1f).toDouble()
        val zoomTop = range?.endInclusive ?: digital
        val sensor = ch.get(CameraCharacteristics.SENSOR_ORIENTATION) ?: 90
        val ranges = ch.get(CameraCharacteristics.CONTROL_AE_AVAILABLE_TARGET_FPS_RANGES)?.toList() ?: emptyList()
        val torch = ch.get(CameraCharacteristics.FLASH_INFO_AVAILABLE) == true
        val stabModes = ch.get(CameraCharacteristics.CONTROL_AVAILABLE_VIDEO_STABILIZATION_MODES)?.toList() ?: emptyList()
        val eis = CameraMetadata.CONTROL_VIDEO_STABILIZATION_MODE_ON in stabModes
        val oisModes = ch.get(CameraCharacteristics.LENS_INFO_AVAILABLE_OPTICAL_STABILIZATION)?.toList() ?: emptyList()
        val ois = CameraMetadata.LENS_OPTICAL_STABILIZATION_MODE_ON in oisModes

        val features = buildList {
            if (eis || ois) add("stabilization")
            if (torch) add("torch")
        }
        val (name, lensType) = when {
            front -> "Front" to "wide"
            ratio < 0.8 -> "Ultra Wide" to "ultraWide"
            ratio < 1.5 -> "Wide" to "wide"
            else -> "Telephoto" to "telephoto"
        }
        val formats = formatsFor(ch, ranges)
        val info = CameraInfo(
            id = "$id:${"%.1f".format(java.util.Locale.ROOT, ratio)}",
            name = name,
            position = if (front) "front" else "back",
            lens = lensType,
            zoomMin = 1.0,
            zoomMax = max(1.0, min(10.0, zoomTop / ratio)),
            formats = formats,
            features = features,
        )
        return Lens(
            info = info, cameraId = id, baseRatio = ratio, front = front, sensorOrientation = sensor,
            ratioRange = range, maxDigitalZoom = digital, fpsRanges = ranges, torch = torch,
            stabilization = eis || ois, opticalStabilization = ois,
            activeArray = ch.get(CameraCharacteristics.SENSOR_INFO_ACTIVE_ARRAY_SIZE),
            bootTimeClock = ch.get(CameraCharacteristics.SENSOR_INFO_TIMESTAMP_SOURCE) ==
                CameraMetadata.SENSOR_INFO_TIMESTAMP_SOURCE_REALTIME,
        )
    }

    /**
     * Standard 16:9 resolutions (720p, 1080p, 1440p, 4K) the camera and encoder both handle, with usable
     * frame rates. Odd sizes some phones offer (e.g. 3264x1836) would only confuse people.
     */
    private fun formatsFor(ch: CameraCharacteristics, ranges: List<Range<Int>>): List<FormatInfo> {
        val map = ch.get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP) ?: return emptyList()
        val sizes = map.getOutputSizes(MediaCodec::class.java)?.toList() ?: emptyList()
        return sizes
            .filter { it.height in STANDARD_HEIGHTS && it.width <= 3840 && abs(it.width * 9 - it.height * 16) <= 16 }
            .sortedByDescending { it.width }
            .mapNotNull { size ->
                val minDuration = map.getOutputMinFrameDuration(MediaCodec::class.java, size)
                val cameraMax = if (minDuration > 0) 1_000_000_000.0 / minDuration else 30.0
                val fps = FRAME_RATES.filter { f ->
                    f <= cameraMax + 0.5 &&
                        ranges.any { f in it.lower..it.upper } &&
                        VideoEncoder.supports(size.width, size.height, f)
                }
                if (fps.isEmpty()) null else FormatInfo(size.width, size.height, fps)
            }
    }

    // endregion

    // region Running

    /**
     * Makes the camera match `config`, showing a preview on `texture` and feeding `encoder`.
     * Opens the camera or rebuilds the capture session only when needed.
     * `onApplied` runs once the new configuration is streaming.
     */
    fun start(config: StreamConfig, texture: SurfaceTexture?, encoder: Surface?, onApplied: (() -> Unit)? = null) {
        handler.post {
            desired = Desired(config, texture, encoder, onApplied)
            reconfigure()
        }
    }

    fun stop() {
        handler.post {
            desired = null
            closeSession()
            closeDevice()
            releasePreview()
        }
    }

    private fun reconfigure() {
        val d = desired ?: return
        if (opening || creatingSession) return
        val lens = lensFor(d.config.camera) ?: return

        if (device != null && deviceId != lens.cameraId) {
            closeSession()
            closeDevice()
        }
        val cam = device
        if (cam == null) {
            openDevice(lens.cameraId)
            return
        }

        val preview = if (previewDisabled) null else previewFor(d.texture)
        if (preview == null && d.encoder == null) {
            // Nothing to render to: idle the session and report in.
            closeSession()
            d.onApplied?.let { cb ->
                desired = d.copy(onApplied = null)
                cb()
            }
            return
        }
        val key = listOf(lens.cameraId, d.config.width, d.config.height, preview, d.encoder)
        if (session == null || key != sessionKey) {
            createSession(cam, d, preview, key)
        } else {
            updateRequest(d, lens, preview)
        }
    }

    private fun previewFor(texture: SurfaceTexture?): Surface? {
        if (texture == null) {
            releasePreview()
            return null
        }
        if (previewTexture !== texture) {
            releasePreview()
            texture.setDefaultBufferSize(PREVIEW_SIZE.width, PREVIEW_SIZE.height)
            previewTexture = texture
            previewSurface = Surface(texture)
        }
        return previewSurface
    }

    private fun releasePreview() {
        previewSurface?.release()
        previewSurface = null
        previewTexture = null
    }

    private fun openDevice(id: String) {
        if (ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) != PackageManager.PERMISSION_GRANTED) {
            Log.w(TAG, "camera permission missing")
            return
        }
        opening = true
        try {
            manager.openCamera(id, object : CameraDevice.StateCallback() {
                override fun onOpened(camera: CameraDevice) {
                    handler.post {
                        opening = false
                        device = camera
                        deviceId = id
                        reconfigure()
                    }
                }

                override fun onDisconnected(camera: CameraDevice) {
                    handler.post {
                        camera.close()
                        if (device === camera) resetDeviceState()
                        opening = false
                    }
                }

                override fun onError(camera: CameraDevice, error: Int) {
                    Log.e(TAG, "camera error $error")
                    handler.post {
                        camera.close()
                        if (device === camera) resetDeviceState()
                        opening = false
                    }
                }
            }, handler)
        } catch (e: Exception) {
            Log.e(TAG, "openCamera failed", e)
            opening = false
        }
    }

    private fun resetDeviceState() {
        device = null
        deviceId = null
        session = null
        sessionKey = null
        creatingSession = false
    }

    private fun closeSession() {
        try { session?.close() } catch (_: Exception) {}
        session = null
        sessionKey = null
    }

    private fun closeDevice() {
        try { device?.close() } catch (_: Exception) {}
        resetDeviceState()
    }

    private fun createSession(cam: CameraDevice, d: Desired, preview: Surface?, key: List<Any?>) {
        closeSession()
        val surfaces = listOfNotNull(preview, d.encoder)
        if (surfaces.isEmpty()) return
        creatingSession = true
        try {
            @Suppress("DEPRECATION")
            cam.createCaptureSession(surfaces, object : CameraCaptureSession.StateCallback() {
                override fun onConfigured(s: CameraCaptureSession) {
                    handler.post {
                        creatingSession = false
                        if (device !== cam) {
                            s.close()
                            return@post
                        }
                        session = s
                        sessionKey = key
                        val current = desired
                        val lens = current?.let { lensFor(it.config.camera) }
                        if (current != null && lens != null) updateRequest(current, lens, preview)
                    }
                }

                override fun onConfigureFailed(s: CameraCaptureSession) {
                    handler.post {
                        creatingSession = false
                        Log.e(TAG, "session configuration failed (preview=${preview != null})")
                        if (preview != null && d.encoder != null) {
                            // Some phones can't preview and record 4K together: stream without the preview.
                            previewDisabled = true
                            reconfigure()
                        }
                    }
                }
            }, handler)
        } catch (e: Exception) {
            creatingSession = false
            Log.e(TAG, "createCaptureSession failed", e)
        }
    }

    private fun updateRequest(d: Desired, lens: Lens, preview: Surface?) {
        val cam = device ?: return
        val s = session ?: return
        val c = d.config
        try {
            val b = cam.createCaptureRequest(CameraDevice.TEMPLATE_RECORD)
            preview?.let { b.addTarget(it) }
            d.encoder?.let { b.addTarget(it) }
            b.set(CaptureRequest.CONTROL_AE_TARGET_FPS_RANGE, pickRange(lens.fpsRanges, c.fps))
            b.set(CaptureRequest.CONTROL_AF_MODE, CaptureRequest.CONTROL_AF_MODE_CONTINUOUS_VIDEO)
            if (lens.stabilization) {
                // Electronic stabilization adds a few frames of latency, so it's opt-in.
                b.set(
                    CaptureRequest.CONTROL_VIDEO_STABILIZATION_MODE,
                    if (c.stabilization) CameraMetadata.CONTROL_VIDEO_STABILIZATION_MODE_ON
                    else CameraMetadata.CONTROL_VIDEO_STABILIZATION_MODE_OFF,
                )
                if (lens.opticalStabilization) {
                    b.set(
                        CaptureRequest.LENS_OPTICAL_STABILIZATION_MODE,
                        if (c.stabilization) CameraMetadata.LENS_OPTICAL_STABILIZATION_MODE_ON
                        else CameraMetadata.LENS_OPTICAL_STABILIZATION_MODE_OFF,
                    )
                }
            }
            b.set(CaptureRequest.FLASH_MODE, if (c.torch && lens.torch) CaptureRequest.FLASH_MODE_TORCH else CaptureRequest.FLASH_MODE_OFF)
            applyZoom(b, lens, lens.baseRatio * c.zoom)
            s.setRepeatingRequest(b.build(), null, handler)
            d.onApplied?.let { cb ->
                desired = d.copy(onApplied = null)
                cb()
            }
        } catch (e: Exception) {
            Log.e(TAG, "setRepeatingRequest failed", e)
        }
    }

    private fun applyZoom(b: CaptureRequest.Builder, lens: Lens, ratio: Double) {
        val range = lens.ratioRange
        if (range != null && Build.VERSION.SDK_INT >= 30) {
            b.set(CaptureRequest.CONTROL_ZOOM_RATIO, ratio.coerceIn(range.start, range.endInclusive).toFloat())
            return
        }
        val active = lens.activeArray ?: return
        val z = ratio.coerceIn(1.0, lens.maxDigitalZoom)
        if (z <= 1.0) return
        val w = (active.width() / z).toInt()
        val h = (active.height() / z).toInt()
        val left = active.centerX() - w / 2
        val top = active.centerY() - h / 2
        b.set(CaptureRequest.SCALER_CROP_REGION, Rect(left, top, left + w, top + h))
    }

    /** Prefers an exact `fps..fps` range so the frame rate doesn't float with the light. */
    private fun pickRange(ranges: List<Range<Int>>, fps: Int): Range<Int> =
        ranges.firstOrNull { it.lower == fps && it.upper == fps }
            ?: ranges.filter { fps in it.lower..it.upper }.maxByOrNull { it.lower }
            ?: Range(fps, fps)

    // endregion

    companion object {
        private const val TAG = "PocketWebcamCam"
        val FRAME_RATES = listOf(24, 25, 30, 48, 50, 60)
        val STANDARD_HEIGHTS = setOf(720, 1080, 1440, 2160)
        val PREVIEW_SIZE = Size(1280, 720)
    }
}
