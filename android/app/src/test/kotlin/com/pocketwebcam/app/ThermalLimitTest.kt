package com.pocketwebcam.app

import com.pocketwebcam.app.net.StreamConfig
import org.junit.Assert.assertEquals
import org.junit.Assert.assertSame
import org.junit.Test

class ThermalLimitTest {
    private val uhd60 = StreamConfig(width = 3840, height = 2160, fps = 60)

    @Test
    fun thermalNamesMapToLimits() {
        assertEquals(ThermalLimit.None, ThermalLimit.forThermal("nominal"))
        assertEquals(ThermalLimit.None, ThermalLimit.forThermal("fair"))
        assertEquals(ThermalLimit.Hot, ThermalLimit.forThermal("serious"))
        assertEquals(ThermalLimit.TooHot, ThermalLimit.forThermal("critical"))
    }

    @Test
    fun hotStepsDownTo1080p30() {
        val c = ThermalLimit.Hot.cap(uhd60)
        assertEquals(1920, c.width)
        assertEquals(1080, c.height)
        assertEquals(30, c.fps)
    }

    @Test
    fun tooHotStepsDownTo720p30() {
        val c = ThermalLimit.TooHot.cap(uhd60)
        assertEquals(1280 to 720, c.width to c.height)
        assertEquals(30, c.fps)
    }

    @Test
    fun smallStreamsAreLeftAlone() {
        val c = StreamConfig(width = 1280, height = 720, fps = 30)
        assertSame(c, ThermalLimit.Hot.cap(c))
        assertSame(uhd60, ThermalLimit.None.cap(uhd60))
    }

    @Test
    fun onlyTheFrameRateDropsAt1080p60() {
        val c = ThermalLimit.Hot.cap(StreamConfig(width = 1920, height = 1080, fps = 60))
        assertEquals(1920 to 1080, c.width to c.height)
        assertEquals(30, c.fps)
    }

    @Test
    fun presetBitrateFollowsTheSmallerStream() {
        val wanted = uhd60.copy(bitrate = Quality.High.bitrate(uhd60))
        val actual = ThermalLimit.Hot.cap(wanted)
        assertEquals(Quality.High.bitrate(actual), ThermalLimit.bitrate(wanted, actual))
    }

    @Test
    fun customBitrateScalesWithPixelRate() {
        val wanted = uhd60.copy(bitrate = 40_000_000)
        val actual = ThermalLimit.Hot.cap(wanted)
        // A quarter of the pixels at half the frame rate.
        assertEquals(5_000_000, ThermalLimit.bitrate(wanted, actual))
        assertEquals(40_000_000, ThermalLimit.bitrate(wanted, wanted))
    }
}
