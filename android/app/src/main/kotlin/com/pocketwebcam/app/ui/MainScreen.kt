package com.pocketwebcam.app.ui

import android.graphics.Matrix
import android.graphics.SurfaceTexture
import android.view.TextureView
import android.view.View
import android.view.Window
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.displayCutoutPadding
import androidx.compose.foundation.layout.systemBarsPadding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material3.Icon
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import com.pocketwebcam.app.PairingPrompt
import com.pocketwebcam.app.net.Pairing
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Slider
import androidx.compose.material3.Surface
import androidx.compose.material3.SliderDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import com.pocketwebcam.app.AppModel
import com.pocketwebcam.app.BuildConfig
import com.pocketwebcam.app.Quality
import com.pocketwebcam.app.ThermalLimit

private val Yellow = Color(0xFFFFD60A)
private val Ink = Color(0xFF1A1500)
private val Bg = Color(0xFF0B0B0C)
private val Card = Color(0xFF18181B)
private val Line = Color(0xFF2A2A2E)
private val Muted = Color(0xFFA1A1A8)
private val Green = Color(0xFF30D158)
private val Orange = Color(0xFFFF9F0A)
private val Red = Color(0xFFFF453A)

@Composable
fun PocketTheme(content: @Composable () -> Unit) {
    MaterialTheme(
        colorScheme = darkColorScheme(
            primary = Yellow, onPrimary = Ink, background = Bg, surface = Card, onSurface = Color(0xFFF5F5F7),
        ),
        content = content,
    )
}

@Composable
fun MainScreen(model: AppModel, window: Window, requestCamera: () -> Unit, requestMic: () -> Unit) {
    val connected = model.connectedTo != null

    // Keep the screen awake while streaming; dimmed mode drops brightness to save battery.
    DisposableEffect(connected) {
        if (connected) window.addFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        onDispose { window.clearFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON) }
    }
    DisposableEffect(model.dimmed) {
        val attrs = window.attributes
        attrs.screenBrightness = if (model.dimmed) 0.01f else android.view.WindowManager.LayoutParams.BRIGHTNESS_OVERRIDE_NONE
        window.attributes = attrs
        onDispose {
            val a = window.attributes
            a.screenBrightness = android.view.WindowManager.LayoutParams.BRIGHTNESS_OVERRIDE_NONE
            window.attributes = a
        }
    }

    Surface(Modifier.fillMaxSize(), color = Bg, contentColor = Color(0xFFF5F5F7)) {
    Box(Modifier.fillMaxSize()) {
        if (!model.hasPermission) {
            Onboarding(requestCamera)
        } else {
            var menuOpen by remember { mutableStateOf(false) }
            // The video fills the screen and stays centered; the top bar and the camera panel float over it.
            Box(Modifier.fillMaxSize().systemBarsPadding(), contentAlignment = Alignment.Center) {
                Preview(model)
            }
            androidx.compose.animation.AnimatedVisibility(menuOpen, enter = fadeIn(), exit = fadeOut()) {
                Box(
                    Modifier
                        .fillMaxSize()
                        .background(Color.Black.copy(alpha = 0.45f))
                        .clickable(remember { MutableInteractionSource() }, indication = null) { menuOpen = false },
                )
            }
            Column(
                Modifier
                    .align(Alignment.TopCenter)
                    .fillMaxWidth()
                    .background(Brush.verticalGradient(listOf(Color.Black.copy(alpha = 0.7f), Color.Transparent)))
                    .statusBarsPadding()
                    .displayCutoutPadding(),
                horizontalAlignment = Alignment.End,
            ) {
                TopBar(model, menuOpen, requestMic) { menuOpen = !menuOpen }
                AnimatedVisibility(
                    menuOpen,
                    enter = slideInVertically { -it / 3 } + fadeIn(),
                    exit = slideOutVertically { -it / 3 } + fadeOut(),
                ) {
                    Controls(model)
                }
            }
            if (model.connectedTo == null && model.pairedComputers.isEmpty() && model.pairing == null) {
                PairHint(Modifier.align(Alignment.BottomCenter))
            }
            if (model.dimmed) {
                Box(
                    Modifier.fillMaxSize().background(Color.Black).clickable { model.dimmed = false },
                    contentAlignment = Alignment.Center,
                ) {
                    Text("Streaming · tap to wake", color = Muted, fontSize = 16.sp)
                }
            }
        }
        model.pairing?.let { PairingDialog(model, it) }
    }
    }
}

/** How to pair, while no computer is paired yet. */
@Composable
private fun PairHint(modifier: Modifier) {
    Column(
        modifier
            .navigationBarsPadding()
            .padding(16.dp)
            .fillMaxWidth()
            .clip(RoundedCornerShape(20.dp))
            .background(Card)
            .padding(18.dp),
    ) {
        Text("Pair with a computer", fontWeight = FontWeight.Bold, fontSize = 17.sp)
        Spacer(Modifier.size(6.dp))
        Text(
            "Open Pocket Webcam on your computer and choose Pair a phone. Scan the code it shows with your " +
                "phone's camera app or Google Lens, then tap the link. Or pick this phone on the computer and compare codes.",
            color = Muted, fontSize = 14.sp,
        )
    }
}

/** Pairing as it happens: waiting for the computer, the code to compare, the outcome. */
@Composable
private fun PairingDialog(model: AppModel, prompt: PairingPrompt) {
    val dismissable = prompt.finished
    AlertDialog(
        onDismissRequest = { if (dismissable) model.pairing = null },
        title = {
            Text(
                when (prompt) {
                    is PairingPrompt.Waiting -> "Pairing with ${prompt.computer}…"
                    is PairingPrompt.Code -> "Pair with ${prompt.computer}?"
                    is PairingPrompt.Confirmed -> "Pair with ${prompt.computer}?"
                    is PairingPrompt.Paired -> "Paired with ${prompt.computer}"
                    is PairingPrompt.Failed -> "Not paired"
                },
            )
        },
        text = {
            Column {
                when (prompt) {
                    is PairingPrompt.Waiting -> Text("Keep Pocket Webcam open on the computer. Both need to be on the same Wi-Fi.", color = Muted)
                    is PairingPrompt.Code, is PairingPrompt.Confirmed -> {
                        val code = if (prompt is PairingPrompt.Code) prompt.code else (prompt as PairingPrompt.Confirmed).code
                        Text("Check that the computer shows the same code:", color = Muted)
                        Spacer(Modifier.size(12.dp))
                        Text(
                            Pairing.spaced(code), color = Yellow, fontSize = 40.sp, fontWeight = FontWeight.Bold,
                            fontFamily = androidx.compose.ui.text.font.FontFamily.Monospace,
                            modifier = Modifier.fillMaxWidth(), textAlign = androidx.compose.ui.text.style.TextAlign.Center,
                        )
                        Spacer(Modifier.size(12.dp))
                        Text(
                            if (prompt is PairingPrompt.Confirmed) "Now confirm on the computer."
                            else "If the codes are different, don't pair: something else is answering.",
                            color = Muted, fontSize = 13.sp,
                        )
                    }
                    is PairingPrompt.Paired -> Text("From now on they connect by themselves whenever the app is open.", color = Muted)
                    is PairingPrompt.Failed -> Text(prompt.why, color = Muted)
                }
            }
        },
        confirmButton = {
            when (prompt) {
                is PairingPrompt.Code -> TextButton(onClick = { model.answerCode(true) }) { Text("Pair", color = Yellow) }
                is PairingPrompt.Paired, is PairingPrompt.Failed -> TextButton(onClick = { model.pairing = null }) { Text("OK", color = Yellow) }
                else -> {}
            }
        },
        dismissButton = {
            when (prompt) {
                is PairingPrompt.Code -> TextButton(onClick = { model.answerCode(false) }) { Text("Cancel", color = Muted) }
                is PairingPrompt.Waiting, is PairingPrompt.Confirmed -> TextButton(onClick = { model.cancelPairing() }) { Text("Cancel", color = Muted) }
                else -> {}
            }
        },
    )
}

@Composable
private fun Onboarding(requestCamera: () -> Unit) {
    Column(
        Modifier.fillMaxSize().padding(32.dp),
        verticalArrangement = Arrangement.Center,
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Text("Pocket Webcam", fontSize = 32.sp, fontWeight = FontWeight.Black, color = Yellow)
        Spacer(Modifier.size(16.dp))
        Text(
            "Turn this phone into a wireless webcam for your computer. Pocket Webcam needs the camera to do that. " +
                "Video goes straight to your computer over your own Wi-Fi and never to our servers.",
            color = Muted, fontSize = 16.sp,
        )
        Spacer(Modifier.size(28.dp))
        Button(onClick = requestCamera, colors = ButtonDefaults.buttonColors(containerColor = Yellow, contentColor = Ink)) {
            Text("Allow camera", fontWeight = FontWeight.Bold)
        }
    }
}

@Composable
private fun TopBar(model: AppModel, menuOpen: Boolean, requestMic: () -> Unit, onToggleMenu: () -> Unit) {
    var showSettings by remember { mutableStateOf(false) }
    Row(
        Modifier.fillMaxWidth().padding(start = 16.dp, end = 10.dp, top = 8.dp, bottom = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.size(10.dp).clip(CircleShape).background(if (model.connectedTo != null) Green else Yellow))
        Spacer(Modifier.size(10.dp))
        Column(Modifier.weight(1f)) {
            val who = model.connectedTo
            Text(
                if (who != null) "Streaming to $who" else "Waiting for computer",
                fontWeight = FontWeight.Bold, fontSize = 16.sp, maxLines = 1, overflow = TextOverflow.Ellipsis,
            )
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    if (who != null) "${model.streamFps} fps · ${"%.1f".format(model.streamMbps)} Mbps"
                    else model.deviceName + (model.ipAddress?.let { " · $it:${model.listeningPort}" } ?: ""),
                    color = Muted, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis,
                )
                if (who != null && model.micOn) {
                    Spacer(Modifier.size(6.dp))
                    MicLevel(model.micLevel)
                }
            }
        }
        ThermalBadge(model.thermal)
        if (model.connectedTo != null && model.camera.capabilities.audio) {
            val blocked = model.micPermission == "denied"
            RoundButton(
                selected = model.micOn,
                description = if (blocked) "Microphone not allowed. Open settings" else "Microphone",
                onClick = { if (blocked) requestMic() else model.setMicrophone(!model.micOn) },
            ) { tint -> MicIcon(tint, crossed = !model.micOn) }
            Spacer(Modifier.size(8.dp))
        }
        RoundButton(selected = menuOpen, description = "Camera controls", onClick = onToggleMenu) { tint ->
            SlidersIcon(tint)
        }
        Spacer(Modifier.size(8.dp))
        RoundButton(selected = false, description = "Settings", onClick = { showSettings = true }) { tint ->
            Icon(Icons.Filled.Settings, contentDescription = null, tint = tint, modifier = Modifier.size(22.dp))
        }
    }
    if (showSettings) SettingsDialog(model) { showSettings = false }
}

/** A 42 dp round, translucent icon button; yellow while its panel is open. */
@Composable
private fun RoundButton(selected: Boolean, description: String, onClick: () -> Unit, icon: @Composable (Color) -> Unit) {
    Box(
        Modifier
            .size(42.dp)
            .clip(CircleShape)
            .background(if (selected) Yellow else Color.White.copy(alpha = 0.14f))
            .clickable(onClick = onClick)
            .semantics { contentDescription = description },
        contentAlignment = Alignment.Center,
    ) { icon(if (selected) Ink else Color.White) }
}

/** "Hot" / "Too hot" warning; empty while the phone is cool. */
@Composable
private fun ThermalBadge(thermal: String) {
    if (thermal != "serious" && thermal != "critical") return
    val critical = thermal == "critical"
    Text(
        if (critical) "Too hot" else "Hot",
        color = if (critical) Red else Orange, fontWeight = FontWeight.Bold, fontSize = 12.sp,
        modifier = Modifier
            .padding(end = 8.dp)
            .clip(RoundedCornerShape(50))
            .background(Color.White.copy(alpha = 0.14f))
            .padding(horizontal = 10.dp, vertical = 6.dp),
    )
}

/** A microphone, crossed out when off. */
@Composable
private fun MicIcon(tint: Color, crossed: Boolean, iconSize: androidx.compose.ui.unit.Dp = 20.dp) {
    Canvas(Modifier.size(iconSize)) {
        val w = size.width
        val stroke = w * 0.1f
        // Capsule head, cradle and stem.
        drawRoundRect(
            tint, topLeft = Offset(w * 0.35f, w * 0.05f),
            size = androidx.compose.ui.geometry.Size(w * 0.3f, w * 0.5f),
            cornerRadius = androidx.compose.ui.geometry.CornerRadius(w * 0.15f),
        )
        drawArc(
            tint, startAngle = 0f, sweepAngle = 180f, useCenter = false,
            topLeft = Offset(w * 0.22f, w * 0.2f), size = androidx.compose.ui.geometry.Size(w * 0.56f, w * 0.52f),
            style = Stroke(stroke, cap = StrokeCap.Round),
        )
        drawLine(tint, Offset(w * 0.5f, w * 0.72f), Offset(w * 0.5f, w * 0.92f), strokeWidth = stroke, cap = StrokeCap.Round)
        if (crossed) drawLine(tint, Offset(w * 0.12f, w * 0.1f), Offset(w * 0.88f, w * 0.9f), strokeWidth = stroke, cap = StrokeCap.Round)
    }
}

/** Small mic that brightens with the sound level, so you can see the phone hears you. */
@Composable
private fun MicLevel(level: Float) {
    // Speech peaks sit around 0.1 to 0.5; a square root spreads that range out.
    val amount = minOf(1f, kotlin.math.sqrt(level) * 1.4f)
    Box(Modifier.semantics { contentDescription = "Microphone on" }) {
        MicIcon(Green.copy(alpha = 0.35f + 0.65f * amount), crossed = false, iconSize = 12.dp)
    }
}

/** Three slider lines with knobs: "adjust the camera". */
@Composable
private fun SlidersIcon(tint: Color) {
    Canvas(Modifier.size(20.dp)) {
        val stroke = 2.dp.toPx()
        val knob = 3.2.dp.toPx()
        listOf(0.22f to 0.32f, 0.5f to 0.68f, 0.78f to 0.42f).forEach { (y, x) ->
            val cy = size.height * y
            drawLine(tint, Offset(0f, cy), Offset(size.width, cy), strokeWidth = stroke, cap = StrokeCap.Round)
            drawCircle(tint, radius = knob, center = Offset(size.width * x, cy))
        }
    }
}

@Composable
private fun SettingsDialog(model: AppModel, onDismiss: () -> Unit) {
    var name by remember { mutableStateOf(model.deviceName) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Settings") },
        text = {
            Column {
                Text("Name shown on your computer", color = Muted, fontSize = 13.sp)
                OutlinedTextField(value = name, onValueChange = { name = it }, singleLine = true)
                Spacer(Modifier.size(16.dp))
                Text("Paired computers", color = Muted, fontSize = 13.sp)
                if (model.pairedComputers.isEmpty()) Text("None yet", fontSize = 14.sp)
                model.pairedComputers.forEach { (fingerprint, computer) ->
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(computer, Modifier.weight(1f), maxLines = 1, overflow = TextOverflow.Ellipsis)
                        TextButton(onClick = { model.forgetComputer(fingerprint) }) { Text("Forget", color = Muted) }
                    }
                }
                Text("Only paired computers can connect, and everything they send each other is encrypted.", color = Muted, fontSize = 12.sp)
                model.identityError?.let {
                    Spacer(Modifier.size(8.dp))
                    Text(it, color = Red, fontSize = 13.sp)
                }
                Spacer(Modifier.size(12.dp))
                Text("Pocket Webcam ${BuildConfig.VERSION_NAME}", color = Muted, fontSize = 12.sp)
            }
        },
        confirmButton = { TextButton(onClick = { model.rename(name); onDismiss() }) { Text("Save", color = Yellow) } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel", color = Muted) } },
    )
}

/** The live preview, rotated upright relative to the screen. */
@Composable
private fun Preview(model: AppModel) {
    // Re-read when the screen rotates.
    val orientation = LocalConfiguration.current.orientation
    val display = LocalView.current.display
    val displayDegrees = when (display?.rotation) {
        android.view.Surface.ROTATION_90 -> 90
        android.view.Surface.ROTATION_180 -> 180
        android.view.Surface.ROTATION_270 -> 270
        else -> 0
    }
    @Suppress("UNUSED_VARIABLE") val unused = orientation
    // The camera already turns preview frames upright for the phone's natural (portrait) orientation,
    // so only the screen's own rotation is left to undo. Rotating by the sensor angle as well turned
    // the preview sideways and stretched it.
    val rotation = (360 - displayDegrees) % 360
    // Mirror along the scene's left/right, which is the screen's up/down when the phone is held
    // sideways with the screen locked in portrait (and the other way round).
    val mirror = when {
        !model.config.mirror -> Mirror.None
        (model.physicalOrientation + displayDegrees) % 180 != 0 -> Mirror.Vertical
        else -> Mirror.Horizontal
    }
    val portraitScreen = displayDegrees % 180 == 0
    val aspect = model.config.width.toFloat() / model.config.height.toFloat()

    Box(
        Modifier
            .padding(8.dp)
            .aspectRatio(if (portraitScreen) 1f / aspect else aspect)
            .clip(RoundedCornerShape(16.dp))
            .background(Card)
            .border(BorderStroke(1.dp, Line), RoundedCornerShape(16.dp)),
    ) {
        AndroidView(
            modifier = Modifier.fillMaxSize(),
            factory = { ctx ->
                TextureView(ctx).apply {
                    surfaceTextureListener = object : TextureView.SurfaceTextureListener {
                        override fun onSurfaceTextureAvailable(st: SurfaceTexture, w: Int, h: Int) = model.setPreviewTexture(st)
                        override fun onSurfaceTextureSizeChanged(st: SurfaceTexture, w: Int, h: Int) {}
                        override fun onSurfaceTextureDestroyed(st: SurfaceTexture): Boolean {
                            model.setPreviewTexture(null)
                            return true
                        }
                        override fun onSurfaceTextureUpdated(st: SurfaceTexture) {}
                    }
                    addOnLayoutChangeListener { v, _, _, _, _, _, _, _, _ ->
                        @Suppress("UNCHECKED_CAST")
                        (v.tag as? Pair<Int, Mirror>)?.let { (r, m) -> applyTransform(v as TextureView, r, m) }
                    }
                }
            },
            update = { view ->
                view.tag = rotation to mirror
                applyTransform(view, rotation, mirror)
            },
        )
    }
}

/**
 * The texture is stretched to the view. Undo the stretch for a sideways image, rotate it
 * upright, then optionally mirror it.
 */
private enum class Mirror { None, Horizontal, Vertical }

private fun applyTransform(view: TextureView, rotation: Int, mirror: Mirror) {
    val w = view.width.toFloat()
    val h = view.height.toFloat()
    if (w == 0f || h == 0f) return
    val cx = w / 2
    val cy = h / 2
    val m = Matrix()
    if (rotation % 180 != 0) m.postScale(h / w, w / h, cx, cy)
    m.postRotate(rotation.toFloat(), cx, cy)
    when (mirror) {
        Mirror.Horizontal -> m.postScale(-1f, 1f, cx, cy)
        Mirror.Vertical -> m.postScale(1f, -1f, cx, cy)
        Mirror.None -> {}
    }
    view.setTransform(m)
}

/**
 * The drop-down camera panel: lens, resolution, zoom and quick toggles, all visible without scrolling.
 * Frame rate, quality, codec and dimming live in the "More" sheet.
 */
@Composable
private fun Controls(model: AppModel) {
    val c = model.config
    val lens = model.currentLens
    var showMore by remember { mutableStateOf(false) }
    Column(
        Modifier
            .padding(horizontal = 10.dp)
            .widthIn(max = 520.dp)
            .fillMaxWidth()
            .clip(RoundedCornerShape(20.dp))
            .background(Card.copy(alpha = 0.97f))
            .border(BorderStroke(1.dp, Line), RoundedCornerShape(20.dp))
            // Only scrolls on very small screens.
            .verticalScroll(rememberScrollState())
            .padding(horizontal = 16.dp, vertical = 14.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        ChipRow {
            model.camera.lenses.forEach { l ->
                Chip(model.lensLabel(l), l.info.id == c.camera) { model.selectLens(l.info) }
            }
        }
        if (model.thermalLimit != ThermalLimit.None) {
            Text(
                "The phone is hot, so it sends ${resolutionLabel(c.height)} at ${c.fps} fps until it cools down.",
                color = Orange, fontSize = 12.sp,
            )
        }
        val formats = lens?.info?.formats.orEmpty()
        if (formats.size > 1) {
            ChipRow {
                formats.forEach { f ->
                    Chip(resolutionLabel(f.height), f.width == c.width && f.height == c.height) {
                        model.setResolution(f.width, f.height)
                    }
                }
            }
            if (c.height > 1080) {
                Text(
                    "${resolutionLabel(c.height)} uses more battery and makes the phone warmer. " +
                        "Pocket Webcam on your computer outputs 1080p by default, so 1080p is usually the best choice.",
                    color = Muted, fontSize = 12.sp,
                )
            }
        }
        if (lens != null && lens.info.zoomMax > 1.05) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text("Zoom ${"%.1f".format(c.zoom)}×", color = Muted, fontSize = 13.sp, modifier = Modifier.padding(end = 10.dp))
                Slider(
                    value = c.zoom.toFloat(),
                    onValueChange = { model.setZoom(it.toDouble()) },
                    valueRange = lens.info.zoomMin.toFloat()..lens.info.zoomMax.toFloat(),
                    colors = SliderDefaults.colors(thumbColor = Yellow, activeTrackColor = Yellow),
                    modifier = Modifier.weight(1f),
                )
            }
        }
        ChipRow {
            if (lens?.torch == true) Chip("Light", c.torch) { model.setTorch(!c.torch) }
            Chip("Mirror", c.mirror) { model.setMirror(!c.mirror) }
            if (lens?.stabilization == true) Chip("Stabilize", c.stabilization) { model.setStabilization(!c.stabilization) }
            Chip("More…", false) { showMore = true }
        }
    }
    if (showMore) MoreSheet(model) { showMore = false }
}

private fun resolutionLabel(height: Int) = if (height >= 2160) "4K" else "${height}p"

/** Less-used settings: frame rate, quality, codec, stabilization note and screen dimming. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun MoreSheet(model: AppModel, onDismiss: () -> Unit) {
    val c = model.config
    val lens = model.currentLens
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = Card,
    ) {
        Column(
            Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, bottom = 24.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            lens?.info?.formats?.firstOrNull { it.width == c.width && it.height == c.height }?.fps?.let { rates ->
                if (rates.size > 1) {
                    Section("Frame rate") { rates.forEach { fps -> Chip("$fps", fps == c.fps) { model.setFps(fps) } } }
                }
            }
            Section("Quality") {
                Quality.entries.forEach { q -> Chip(q.label, model.quality == q) { model.setQuality(q) } }
            }
            if (model.camera.capabilities.codecs.size > 1) {
                Section("Codec") {
                    model.camera.capabilities.codecs.forEach { codec ->
                        Chip(if (codec == "hevc") "HEVC" else "H.264", c.codec == codec) { model.setCodec(codec) }
                    }
                }
                Text("HEVC looks better at the same bitrate. Use H.264 if the picture stutters.", color = Muted, fontSize = 12.sp)
            }
            if (lens?.stabilization == true) {
                Text("Stabilize smooths out shaky hands but adds a little delay.", color = Muted, fontSize = 12.sp)
            }
            if (model.connectedTo != null) {
                Button(
                    onClick = { model.dimmed = true; onDismiss() },
                    colors = ButtonDefaults.buttonColors(containerColor = Yellow, contentColor = Ink),
                ) { Text("Dim screen to save battery", fontWeight = FontWeight.Bold) }
            }
        }
    }
}

/** One line of chips that scrolls sideways if it doesn't fit, instead of wrapping. */
@Composable
private fun ChipRow(content: @Composable () -> Unit) {
    Row(
        Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) { content() }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun Section(title: String, content: @Composable () -> Unit) {
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Text(title, color = Muted, fontSize = 12.sp)
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            content()
        }
    }
}

@Composable
private fun Chip(label: String, selected: Boolean, onClick: () -> Unit) {
    Box(
        Modifier
            .clip(RoundedCornerShape(50))
            .background(if (selected) Yellow else Bg)
            .border(BorderStroke(1.dp, if (selected) Yellow else Line), RoundedCornerShape(50))
            .clickable(onClick = onClick)
            .padding(horizontal = 16.dp, vertical = 9.dp),
    ) {
        Text(label, color = if (selected) Ink else Color.White, fontWeight = FontWeight.Bold, fontSize = 14.sp)
    }
}
