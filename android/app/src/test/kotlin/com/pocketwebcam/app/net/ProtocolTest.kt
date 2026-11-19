package com.pocketwebcam.app.net

import org.json.JSONObject
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.nio.ByteBuffer

class ProtocolTest {
    @Test
    fun audioFrameLayout() {
        val samples = ByteArray(AudioFormat.CHUNK_SAMPLES * 2) { it.toByte() }
        val frame = Wire.audioFrame(0x0102030405060708L, samples)
        val buf = ByteBuffer.wrap(frame)
        assertEquals(Msg.AUDIO_FRAME, buf.get(0).toInt())
        assertEquals(8 + 960, buf.getInt(1))
        assertEquals(0x0102030405060708L, buf.getLong(5))
        assertArrayEquals(samples, frame.copyOfRange(13, frame.size))
    }

    @Test
    fun audioFormatIsTheV1Format() {
        val json = AudioFormat.toJson()
        assertEquals("pcm_s16le", json.getString("codec"))
        assertEquals(48_000, json.getInt("sampleRate"))
        assertEquals(1, json.getInt("channels"))
        assertEquals(480, AudioFormat.CHUNK_SAMPLES)
    }

    @Test
    fun capabilitiesListAudioOnlyWithAMic() {
        val withMic = Capabilities(emptyList(), listOf("h264"), audio = true).toJson()
        assertEquals("pcm_s16le", withMic.getJSONObject("audio").getJSONArray("codecs").getString(0))
        val without = Capabilities(emptyList(), listOf("h264")).toJson()
        assertFalse(without.has("audio"))
    }

    @Test
    fun audioIsNotPartOfTheSavedConfig() {
        val c = StreamConfig().merged(JSONObject().put("audio", true).put("fps", 60))
        assertEquals(60, c.fps)
        assertFalse(c.toJson().has("audio"))
        assertTrue(c.toJson().has("mirror"))
    }
}
