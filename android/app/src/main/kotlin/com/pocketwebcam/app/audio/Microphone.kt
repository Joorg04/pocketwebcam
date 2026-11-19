package com.pocketwebcam.app.audio

import android.Manifest
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.AudioTimestamp
import android.media.MediaRecorder
import android.os.Process
import android.os.SystemClock
import android.util.Log
import androidx.annotation.RequiresPermission
import com.pocketwebcam.app.net.AudioFormat as Wire
import kotlin.concurrent.thread
import kotlin.math.abs

/**
 * Records the phone's microphone as 10 ms chunks of 48 kHz mono signed 16-bit
 * little-endian PCM, the AUDIO_FRAME format, and hands them to [onChunk] on its own thread.
 *
 * Uses the CAMCORDER source: tuned for video and, where the phone has several mics, the
 * one facing the same way as the camera. No echo cancellation on the phone: the computer's
 * call app already does that on our virtual mic (see docs/PHASE4_MICROPHONE.md).
 */
class Microphone(
    private val onChunk: (ptsMicros: Long, pcm: ByteArray) -> Unit,
    /** Loudest sample of the last ~100 ms, 0 to 1, for the level indicator. */
    private val onLevel: (Float) -> Unit,
    /** Recording stopped by itself (the mic was taken away or failed). */
    private val onFailed: () -> Unit,
) {
    @Volatile private var running = false
    private var worker: Thread? = null

    /**
     * Starts recording. `bootTime` puts timestamps on [SystemClock.elapsedRealtimeNanos], the
     * clock some cameras stamp frames with; otherwise it's [System.nanoTime] like the rest.
     * Returns false if the microphone can't be opened (for example, another app holds it).
     */
    @RequiresPermission(Manifest.permission.RECORD_AUDIO)
    fun start(bootTime: Boolean): Boolean {
        if (running) return true
        val minBuffer = AudioRecord.getMinBufferSize(Wire.SAMPLE_RATE, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT)
        if (minBuffer <= 0) return false
        val record = try {
            AudioRecord(
                MediaRecorder.AudioSource.CAMCORDER, Wire.SAMPLE_RATE, AudioFormat.CHANNEL_IN_MONO,
                AudioFormat.ENCODING_PCM_16BIT,
                // ~100 ms of room, so a busy moment doesn't lose samples.
                maxOf(minBuffer, CHUNK_BYTES * 10),
            )
        } catch (e: Exception) {
            Log.e(TAG, "cannot create AudioRecord", e)
            return false
        }
        if (record.state != AudioRecord.STATE_INITIALIZED) {
            record.release()
            return false
        }
        try {
            record.startRecording()
        } catch (e: IllegalStateException) {
            Log.e(TAG, "cannot start recording", e)
            record.release()
            return false
        }
        running = true
        worker = thread(name = "pw-mic") { readLoop(record, bootTime) }
        return true
    }

    fun stop() {
        running = false
        worker?.join(500)
        worker = null
    }

    private fun readLoop(record: AudioRecord, bootTime: Boolean) {
        Process.setThreadPriority(Process.THREAD_PRIORITY_URGENT_AUDIO)
        val samples = ShortArray(Wire.CHUNK_SAMPLES)
        val stamp = AudioTimestamp()
        val timebase = if (bootTime) AudioTimestamp.TIMEBASE_BOOTTIME else AudioTimestamp.TIMEBASE_MONOTONIC
        var framesRead = 0L
        var peak = 0
        var chunks = 0
        try {
            while (running) {
                var filled = 0
                while (filled < samples.size && running) {
                    val n = record.read(samples, filled, samples.size - filled)
                    if (n < 0) {
                        Log.e(TAG, "read failed: $n")
                        running = false
                        onFailed()
                        return
                    }
                    filled += n
                }
                if (!running) break
                val pts = ptsNanos(record, stamp, timebase, framesRead, bootTime)
                framesRead += samples.size
                val pcm = ByteArray(CHUNK_BYTES)
                for (i in samples.indices) {
                    val v = samples[i].toInt()
                    pcm[2 * i] = v.toByte()
                    pcm[2 * i + 1] = (v shr 8).toByte()
                    peak = maxOf(peak, abs(v))
                }
                onChunk(pts / 1000, pcm)
                if (++chunks % 10 == 0) {
                    onLevel(minOf(1f, peak / 32768f))
                    peak = 0
                }
            }
        } finally {
            try { record.stop() } catch (_: IllegalStateException) {}
            record.release()
            onLevel(0f)
        }
    }

    /** Capture time of the chunk starting at `frame`, from the driver's timestamp when it has one. */
    private fun ptsNanos(record: AudioRecord, stamp: AudioTimestamp, timebase: Int, frame: Long, bootTime: Boolean): Long {
        if (record.getTimestamp(stamp, timebase) == AudioRecord.SUCCESS) {
            return stamp.nanoTime + (frame - stamp.framePosition) * 1_000_000_000L / Wire.SAMPLE_RATE
        }
        // No timestamp: the chunk just finished arriving, so it started ~10 ms ago.
        val now = if (bootTime) SystemClock.elapsedRealtimeNanos() else System.nanoTime()
        return now - CHUNK_NANOS
    }

    private companion object {
        const val TAG = "PocketWebcamMic"
        const val CHUNK_BYTES = Wire.CHUNK_SAMPLES * 2
        const val CHUNK_NANOS = 10_000_000L
    }
}
