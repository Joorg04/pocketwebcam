package com.pocketwebcam.app

import com.pocketwebcam.app.net.StreamConfig
import kotlin.math.min

/**
 * How far a hot phone steps its stream down. Encoding 4K or 60 fps is what heats a phone
 * most, so a hot phone sends at most 1080p at 30 fps, and a very hot one 720p at 30 fps.
 * The camera then rounds the result down to a format it has.
 */
enum class ThermalLimit(val maxHeight: Int, val maxFps: Int) {
    None(Int.MAX_VALUE, Int.MAX_VALUE),
    Hot(1080, 30),
    TooHot(720, 30);

    /** The stream to run instead of `wanted`, keeping its shape. */
    fun cap(wanted: StreamConfig): StreamConfig {
        if (wanted.height <= maxHeight && wanted.fps <= maxFps) return wanted
        val height = min(wanted.height, maxHeight)
        val width = if (height == wanted.height) wanted.width else wanted.width * height / wanted.height
        return wanted.copy(width = width, height = height, fps = min(wanted.fps, maxFps))
    }

    companion object {
        /** From STATE.thermal: `serious` steps down once, `critical` twice. */
        fun forThermal(thermal: String) = when (thermal) {
            "serious" -> Hot
            "critical" -> TooHot
            else -> None
        }

        /**
         * Bitrate for the stepped-down stream: the same quality preset if `wanted` used one,
         * otherwise scaled with the pixel rate so a smaller stream doesn't keep a 4K bitrate.
         */
        fun bitrate(wanted: StreamConfig, actual: StreamConfig): Int {
            if (actual.width == wanted.width && actual.height == wanted.height && actual.fps == wanted.fps) return wanted.bitrate
            Quality.entries.firstOrNull { it.bitrate(wanted) == wanted.bitrate }?.let { return it.bitrate(actual) }
            val ratio = actual.width.toDouble() * actual.height * actual.fps / (wanted.width.toDouble() * wanted.height * wanted.fps)
            return (wanted.bitrate * ratio).toInt().coerceAtLeast(1_000_000)
        }
    }
}
