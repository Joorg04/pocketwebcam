package com.pocketwebcam.app.net

import org.json.JSONObject
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File
import java.nio.ByteBuffer

/**
 * Checks this app's protocol code against the sample messages in `protocol-fixtures/`,
 * which the desktop receiver's tests read too (see the README there).
 */
class FixturesTest {
    /** Gradle runs unit tests from the module folder (`android/app`). */
    private fun fixture(name: String): String {
        val dir = generateSequence(File("").absoluteFile) { it.parentFile }
            .map { File(it, "protocol-fixtures") }
            .first { it.isDirectory }
        return File(dir, name).readText()
    }

    private fun bytes(hex: String): ByteArray {
        val clean = hex.filterNot { it.isWhitespace() }
        return ByteArray(clean.length / 2) { clean.substring(it * 2, it * 2 + 2).toInt(16).toByte() }
    }

    private fun frame(name: String): JSONObject {
        val messages = JSONObject(fixture("frames.json")).getJSONArray("messages")
        return (0 until messages.length()).map { messages.getJSONObject(it) }.first { it.getString("name") == name }
    }

    @Test
    fun setConfigWithEveryKeyChangesEveryField() {
        val merged = StreamConfig().merged(JSONObject(fixture("set_config.json")))
        assertEquals(
            StreamConfig(
                camera = "com.apple.avfoundation.avcapturedevice.built-in_video:0",
                width = 1920, height = 1080, fps = 30, codec = "h264", bitrate = 12_000_000,
                zoom = 2.5, torch = true, stabilization = true, mirror = false,
            ),
            merged,
        )
    }

    @Test
    fun setConfigIsAPartialUpdate() {
        val before = StreamConfig(camera = "0", width = 1280, height = 720, fps = 30, codec = "hevc", zoom = 3.0, torch = true)
        val after = before.merged(JSONObject(fixture("set_config_partial.json")))
        assertEquals(before.copy(fps = 60), after)
    }

    @Test
    fun stateCarriesTheWholeConfigUnderTheSharedKeyNames() {
        // STATE is the config's JSON plus device health; a receiver must find every config
        // key from the fixture in what we send, with the same kind of value.
        val ours = StreamConfig(camera = "0").toJson()
        val wanted = JSONObject(fixture("state_minimal.json"))
        for (key in wanted.keys()) {
            assertTrue("STATE is missing \"$key\"", ours.has(key))
            assertEquals("\"$key\" has another type", wanted.get(key)::class, ours.get(key)::class)
        }
        // And what a phone reports can be read back as a config.
        val read = StreamConfig.fromJson(fixture("state.json"))!!
        assertEquals("hevc", read.codec)
        assertEquals(2.5, read.zoom, 0.0)
        assertTrue(read.torch)
    }

    @Test
    fun streamFormatMatchesTheAndroidStyleFixture() {
        val ours = StreamFormat("hevc", 1280, 720, 60, 270, true).toJson()
        val wanted = JSONObject(fixture("stream_format_mirror.json"))
        assertEquals(wanted.keySet(), ours.keySet())
        for (key in wanted.keys()) assertEquals(key, wanted.get(key), ours.get(key))
    }

    @Test
    fun capabilitiesUseTheSharedKeyNames() {
        val camera = CameraInfo("0", "Wide", "back", "wide", 1.0, 10.0, listOf(FormatInfo(1920, 1080, listOf(30, 60))), listOf("torch"))
        val ours = Capabilities(listOf(camera), listOf("h264", "hevc")).toJson()
        val wanted = JSONObject(fixture("capabilities.json"))
        // `audio` is optional (phones without a microphone leave it out).
        assertTrue(wanted.keySet().minus("audio").all { ours.has(it) })
        val ourCamera = ours.getJSONArray("cameras").getJSONObject(0)
        val wantedCamera = wanted.getJSONArray("cameras").getJSONObject(0)
        assertEquals(wantedCamera.keySet(), ourCamera.keySet())
        assertEquals(
            wantedCamera.getJSONArray("formats").getJSONObject(0).keySet(),
            ourCamera.getJSONArray("formats").getJSONObject(0).keySet(),
        )
        assertEquals(wanted.getJSONObject("bitrate").keySet(), ours.getJSONObject("bitrate").keySet())
    }

    @Test
    fun videoFramesAreByteForByteTheFixtures() {
        for (name in listOf("VIDEO_FRAME, keyframe", "VIDEO_FRAME, not a keyframe")) {
            val f = frame(name)
            val v = f.getJSONObject("video")
            val ours = Wire.videoFrame(v.getBoolean("keyframe"), v.getLong("ptsMicros"), bytes(v.getString("dataHex")))
            assertArrayEquals(name, bytes(f.getString("hex")), ours)
        }
    }

    @Test
    fun controlMessagesAreFramedLikeTheFixtures() {
        val keyframeRequest = frame("KEYFRAME_REQ, empty payload")
        assertArrayEquals(bytes(keyframeRequest.getString("hex")), Wire.frame(Msg.KEYFRAME_REQ))

        // A PONG echoes the PING's payload: same bytes, only the type differs.
        val ping = bytes(frame("PING").getString("hex"))
        val payload = ping.copyOfRange(5, ping.size)
        val pong = Wire.frame(Msg.PONG, payload)
        assertEquals(Msg.PONG, pong[0].toInt())
        assertEquals(payload.size, ByteBuffer.wrap(pong).getInt(1))
        assertArrayEquals(payload, pong.copyOfRange(5, pong.size))
        assertArrayEquals(ping, Wire.frame(Msg.PING, payload))
    }

    @Test
    fun helloFromTheDesktopHasWhatTheAppReads() {
        val hello = JSONObject(fixture("hello_desktop.json"))
        assertEquals(Wire.PROTOCOL, hello.optInt("protocol", -1))
        assertEquals("nic-pc", hello.optString("name", "computer"))
    }
}
