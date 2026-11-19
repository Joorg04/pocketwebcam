package com.pocketwebcam.app.net

import android.content.Context
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.os.Build
import android.util.Log
import org.json.JSONObject
import java.io.BufferedOutputStream
import java.io.DataInputStream
import java.io.IOException
import java.net.InetSocketAddress
import java.net.ServerSocket
import java.net.Socket
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import javax.net.ssl.SSLServerSocket
import javax.net.ssl.SSLSocket
import kotlin.concurrent.thread

/** What happens while pairing with a computer, for the UI. */
sealed class PairingEvent {
    /** A computer wants to pair by code: show it and ask the user. */
    data class CodeRequested(val computer: String, val code: String) : PairingEvent()
    data class Paired(val computer: String) : PairingEvent()
    data class Failed(val why: String) : PairingEvent()
}

/**
 * Listens for one desktop receiver, advertises over mDNS, and sends video without ever
 * queueing it (see "Latency and backpressure" in PROTOCOL.md).
 *
 * Every connection is TLS 1.3. Only paired computers get anything beyond the handshake;
 * others can only pair (PROTOCOL.md, "Pairing").
 *
 * Callbacks arrive on network threads: hop to your own thread before touching UI state.
 */
class StreamServer(context: Context) {
    var onHello: ((JSONObject) -> Unit)? = null
    var onPairing: ((PairingEvent) -> Unit)? = null
    var onDisconnected: (() -> Unit)? = null
    var onSetConfig: ((JSONObject) -> Unit)? = null
    var onKeyframeRequest: (() -> Unit)? = null
    /** The server dropped video and needs the encoder to produce a keyframe. */
    var onNeedKeyframe: (() -> Unit)? = null
    var onListening: ((Int) -> Unit)? = null

    private val nsd = context.getSystemService(Context.NSD_SERVICE) as NsdManager
    private val connectivity = context.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
    private var serverSocket: ServerSocket? = null
    private var registration: NsdManager.RegistrationListener? = null
    private var serviceName = Build.MODEL
    /** This phone's own identifier (TXT key `id`); stays the same when it's renamed. */
    private var deviceId = ""
    private var port = 0
    @Volatile private var running = false

    private val lock = Any()
    private var session: Session? = null
    private lateinit var identity: PhoneIdentity
    private lateinit var paired: PairedComputers
    /** The QR code the user scanned last, until it's used or too old. */
    @Volatile private var scan: Pair<Pairing.Scan, Long>? = null

    fun start(name: String, id: String, identity: PhoneIdentity, paired: PairedComputers) {
        serviceName = name
        deviceId = id
        this.identity = identity
        this.paired = paired
        running = true
        thread(name = "pw-accept", isDaemon = true) { acceptLoop() }
    }

    /** The user scanned a computer's QR code: announce it, so that computer connects. */
    fun scanned(code: Pairing.Scan) {
        val until = System.nanoTime() + SCAN_VALID_NANOS
        scan = code to until
        reannounce()
        thread(isDaemon = true) {
            Thread.sleep(SCAN_VALID_NANOS / 1_000_000)
            // Still the same scan and nobody used it: take it back off the network.
            if (scan?.second == until) {
                scan = null
                reannounce()
                onPairing?.invoke(PairingEvent.Failed("The computer didn't connect. Check that it's on the same Wi-Fi, then scan again."))
            }
        }
    }

    fun cancelScan() {
        if (scan == null) return
        scan = null
        reannounce()
    }

    /** The user's answer to "does the computer show the same code?". */
    fun answerCode(yes: Boolean) = session?.answerCode(yes)

    private fun reannounce() {
        if (port != 0) {
            unregister()
            register()
        }
    }

    fun rename(name: String) {
        serviceName = name
        if (port != 0) {
            unregister()
            register()
        }
    }

    fun stop() {
        running = false
        unregister()
        try { serverSocket?.close() } catch (_: IOException) {}
        synchronized(lock) { session?.close() }
    }

    // region Sending

    fun send(type: Int, json: JSONObject) = session?.enqueue(Wire.frame(type, json))

    /** Announces a new stream shape. Video is held back until the next keyframe. */
    fun sendStreamFormat(format: StreamFormat) {
        session?.let {
            it.waitingForKeyframe = true
            it.enqueue(Wire.frame(Msg.STREAM_FORMAT, format.toJson()))
        }
    }

    fun sendVideo(keyframe: Boolean, ptsMicros: Long, annexB: ByteArray) {
        val s = session ?: return
        if (!s.handshakeDone) return
        if (s.waitingForKeyframe && !keyframe) return
        if (!s.videoInFlight.compareAndSet(false, true)) {
            // The socket is still busy with the previous frame: drop, then resync on a keyframe.
            s.waitingForKeyframe = true
            onNeedKeyframe?.invoke()
            return
        }
        s.waitingForKeyframe = false
        s.enqueue(Wire.videoFrame(keyframe, ptsMicros, annexB), isVideo = true)
    }

    /**
     * Unlike video, audio is queued even while a video frame is in flight (a gap in sound
     * is audible, a skipped frame isn't). It's only dropped once 200 ms is backed up.
     */
    fun sendAudio(ptsMicros: Long, samples: ByteArray) {
        val s = session ?: return
        if (!s.handshakeDone) return
        if (s.audioInFlight.incrementAndGet() > MAX_AUDIO_IN_FLIGHT) {
            s.audioInFlight.decrementAndGet()
            return
        }
        s.enqueue(Wire.audioFrame(ptsMicros, samples), isAudio = true)
    }

    // endregion

    // region Listener

    private fun acceptLoop() {
        while (running) {
            val factory = identity.serverContext().serverSocketFactory
            fun tls(socket: ServerSocket) = (socket as SSLServerSocket).apply {
                enabledProtocols = arrayOf("TLSv1.3")
                // The computer must show a certificate (see PhoneIdentity.AnyClient).
                needClientAuth = true
            }
            val server = try {
                tls(factory.createServerSocket()).apply {
                    reuseAddress = true
                    bind(InetSocketAddress(Wire.DEFAULT_PORT))
                }
            } catch (_: IOException) {
                // Port taken: let the system pick one. Receivers read the port from mDNS.
                try { tls(factory.createServerSocket(0)) } catch (e: IOException) {
                    Log.e(TAG, "cannot listen", e)
                    Thread.sleep(1000)
                    continue
                }
            }
            serverSocket = server
            port = server.localPort
            register()
            onListening?.invoke(port)
            try {
                while (running) accept(server.accept())
            } catch (e: IOException) {
                if (running) Log.w(TAG, "accept failed: ${e.message}")
            } finally {
                unregister()
                try { server.close() } catch (_: IOException) {}
            }
            if (running) Thread.sleep(1000)
        }
    }

    private fun register() {
        val info = NsdServiceInfo().apply {
            serviceName = this@StreamServer.serviceName
            serviceType = Wire.SERVICE_TYPE
            port = this@StreamServer.port
            setAttribute("v", Wire.PROTOCOL.toString())
            setAttribute("model", Build.MODEL)
            if (deviceId.isNotEmpty()) setAttribute("id", deviceId)
            setAttribute("fp", identity.fingerprint)
            scan?.let { (code, until) -> if (until > System.nanoTime()) setAttribute("pair", Pairing.tag(code.secret)) }
        }
        val listener = object : NsdManager.RegistrationListener {
            override fun onServiceRegistered(i: NsdServiceInfo) { Log.i(TAG, "advertising as ${i.serviceName}") }
            override fun onRegistrationFailed(i: NsdServiceInfo, code: Int) { Log.e(TAG, "mDNS registration failed: $code") }
            override fun onServiceUnregistered(i: NsdServiceInfo) {}
            override fun onUnregistrationFailed(i: NsdServiceInfo, code: Int) {}
        }
        registration = listener
        try {
            nsd.registerService(info, NsdManager.PROTOCOL_DNS_SD, listener)
        } catch (e: Exception) {
            Log.e(TAG, "mDNS register error", e)
        }
    }

    private fun unregister() {
        registration?.let { try { nsd.unregisterService(it) } catch (_: Exception) {} }
        registration = null
    }

    // endregion

    // region Connection

    /**
     * True if this connection came in over the mobile network. The camera is for computers
     * on the same local network (or on a USB cable), never for the internet; some carriers
     * hand out addresses that can be reached from outside.
     */
    private fun arrivedOverMobileData(socket: Socket): Boolean = try {
        val local = socket.localAddress
        @Suppress("DEPRECATION") // no replacement that lists every network at once
        connectivity.allNetworks.any { network ->
            val caps = connectivity.getNetworkCapabilities(network)
            val links = connectivity.getLinkProperties(network)
            caps != null && links != null &&
                caps.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR) &&
                links.linkAddresses.any { it.address == local }
        }
    } catch (_: Exception) {
        false
    }

    private fun accept(socket: Socket) {
        if (arrivedOverMobileData(socket)) {
            Log.w(TAG, "refused a connection over mobile data")
            try { socket.close() } catch (_: IOException) {}
            return
        }
        synchronized(lock) {
            if (session != null) {
                // One receiver at a time: tell the newcomer and hang up.
                thread(isDaemon = true) {
                    try {
                        val busy = JSONObject().put("code", "busy").put("message", "Another computer is already connected.")
                        socket.getOutputStream().apply { write(Wire.frame(Msg.ERROR, busy)); flush() }
                    } catch (_: IOException) {
                    } finally {
                        try { socket.close() } catch (_: IOException) {}
                    }
                }
                return
            }
            session = Session(socket).also { it.start() }
        }
    }

    private inner class Session(private val socket: Socket) {
        @Volatile var handshakeDone = false
        /** The computer's certificate fingerprint, once the TLS handshake is done. */
        @Volatile private var peer = ""
        @Volatile private var phase: Phase = Phase.First
        @Volatile var waitingForKeyframe = true
        val videoInFlight = AtomicBoolean(false)
        /** Audio chunks queued but not yet written to the socket. */
        val audioInFlight = AtomicInteger(0)
        private val queue = LinkedBlockingQueue<ByteArray>()
        private val closed = AtomicBoolean(false)
        private val connectedAt = System.nanoTime()

        fun start() {
            socket.tcpNoDelay = true
            socket.sendBufferSize = 256 * 1024
            // The receiver pings every 2 s; silence for 10 s means it's gone.
            socket.soTimeout = 10_000
            thread(name = "pw-read", isDaemon = true) { readLoop() }
            thread(name = "pw-write", isDaemon = true) { writeLoop() }
        }

        fun enqueue(data: ByteArray, isVideo: Boolean = false, isAudio: Boolean = false) {
            if (closed.get()) {
                if (isVideo) videoInFlight.set(false)
                if (isAudio) audioInFlight.decrementAndGet()
                return
            }
            queue.add(data)
        }

        /** Sends a last message, then hangs up (once the writer has sent it). */
        private fun sendThenClose(data: ByteArray) {
            enqueue(data)
            queue.add(CLOSE)
        }

        fun answerCode(yes: Boolean) {
            val p = phase as? Phase.ByCode ?: return
            if (yes) {
                if (p.answered) return
                phase = Phase.ByCode(p.computer, answered = true)
                enqueue(Wire.frame(Msg.PAIR_RESULT, PairMsg.result(true)))
            } else {
                // Cancelling, before or after tapping Pair: the computer is told and dropped.
                phase = Phase.First
                sendThenClose(Wire.frame(Msg.PAIR_RESULT, PairMsg.result(false, "declined")))
            }
        }

        fun close() {
            if (!closed.compareAndSet(false, true)) return
            if (phase is Phase.ByCode || phase is Phase.ByScan) {
                onPairing?.invoke(PairingEvent.Failed("The computer stopped pairing."))
            }
            try { socket.close() } catch (_: IOException) {}
            queue.add(ByteArray(0)) // wake the writer
            val wasConnected = handshakeDone
            synchronized(lock) { if (session === this) session = null }
            if (wasConnected) onDisconnected?.invoke()
        }

        private fun writeLoop() {
            try {
                val out = BufferedOutputStream(socket.getOutputStream(), 64 * 1024)
                while (!closed.get()) {
                    val data = queue.take()
                    if (data === CLOSE) { out.flush(); break }
                    if (data.isEmpty()) continue
                    val type = data[0].toInt()
                    out.write(data)
                    if (queue.isEmpty()) out.flush()
                    if (type == Msg.VIDEO_FRAME) videoInFlight.set(false)
                    if (type == Msg.AUDIO_FRAME) audioInFlight.decrementAndGet()
                }
            } catch (e: IOException) {
                Log.w(TAG, "write failed: ${e.message}")
            } catch (_: InterruptedException) {
            } finally {
                close()
            }
        }

        private fun readLoop() {
            try {
                val tls = socket as SSLSocket
                tls.startHandshake()
                val cert = tls.session.peerCertificates.firstOrNull() ?: throw IOException("the computer sent no certificate")
                peer = Pairing.fingerprint(cert.encoded)
                val input = DataInputStream(socket.getInputStream().buffered())
                while (!closed.get()) {
                    val type = input.readUnsignedByte()
                    val length = input.readInt()
                    if (length < 0 || length > MAX_INBOUND_PAYLOAD) throw IOException("payload too large")
                    val payload = ByteArray(length)
                    input.readFully(payload)
                    handle(type, payload)
                    // Nobody pings while people compare codes: wait longer then.
                    val pairing = phase is Phase.ByCode || phase is Phase.ByScan
                    socket.soTimeout = if (pairing) PAIRING_DEADLINE_MILLIS else 10_000
                    // Whoever connects has to introduce itself; otherwise it would keep
                    // real computers out (one receiver at a time) just by sending pings.
                    val deadline = if (pairing) PAIRING_DEADLINE_MILLIS * 1_000_000L else HELLO_DEADLINE_NANOS
                    if (!handshakeDone && System.nanoTime() - connectedAt > deadline) {
                        throw IOException("no HELLO")
                    }
                }
            } catch (e: IOException) {
                Log.i(TAG, "receiver gone: ${e.message}")
            } finally {
                close()
            }
        }

        private fun handle(type: Int, payload: ByteArray) {
            when (type) {
                Msg.PAIR_REQUEST -> if (phase == Phase.First) {
                    val request = try { JSONObject(String(payload, Charsets.UTF_8)) } catch (_: Exception) { return }
                    startPairing(request.optString("mode"), request.optString("name").ifEmpty { "Computer" })
                }
                Msg.PAIR_RESULT -> {
                    val result = try { JSONObject(String(payload, Charsets.UTF_8)) } catch (_: Exception) { return }
                    finishPairing(result.optBoolean("accepted"), result.optString("reason"))
                }
                Msg.HELLO -> {
                    when (phase) {
                        Phase.First -> if (!paired.contains(peer)) {
                            // Not paired: this computer gets nothing, not even our HELLO.
                            Log.i(TAG, "refused a computer that isn't paired")
                            val err = JSONObject().put("code", "not_paired").put("message", "This phone isn't paired with this computer.")
                            sendThenClose(Wire.frame(Msg.ERROR, err))
                            return
                        }
                        Phase.Paired -> {}
                        else -> return
                    }
                    val hello = try { JSONObject(String(payload, Charsets.UTF_8)) } catch (_: Exception) { return }
                    if (hello.optInt("protocol", -1) != Wire.PROTOCOL) {
                        val err = JSONObject().put("code", "version").put("message", "Update Pocket Webcam on this computer.")
                        enqueue(Wire.frame(Msg.ERROR, err))
                        close()
                        return
                    }
                    handshakeDone = true
                    phase = Phase.Streaming
                    onHello?.invoke(hello)
                }
                // Nothing may change the camera before the receiver has said a valid HELLO.
                Msg.SET_CONFIG -> if (handshakeDone) try {
                    onSetConfig?.invoke(JSONObject(String(payload, Charsets.UTF_8)))
                } catch (_: Exception) {}
                Msg.KEYFRAME_REQ -> if (handshakeDone) onKeyframeRequest?.invoke()
                Msg.PING -> enqueue(Wire.frame(Msg.PONG, payload))
                Msg.ERROR -> Log.w(TAG, "receiver error: ${String(payload, Charsets.UTF_8)}")
                else -> {} // PONG and unknown types are ignored
            }
        }

        private fun startPairing(mode: String, computer: String) {
            when (mode) {
                "qr" -> {
                    val (code, _) = scan?.takeIf { it.second > System.nanoTime() } ?: run {
                        sendThenClose(Wire.frame(Msg.PAIR_RESULT, PairMsg.result(false, "no_code")))
                        return
                    }
                    if (code.desktop != peer) {
                        // Another computer, or someone in between who can't have the scanned
                        // computer's key: not the one whose code was scanned.
                        Log.i(TAG, "pairing refused: not the computer whose code was scanned")
                        sendThenClose(Wire.frame(Msg.PAIR_RESULT, PairMsg.result(false, "wrong_computer")))
                        return
                    }
                    val proof = Pairing.proof(code.secret, identity.fingerprint, peer) ?: return
                    phase = Phase.ByScan(code.name.ifEmpty { computer })
                    enqueue(Wire.frame(Msg.PAIR_PROOF, PairMsg.proof(proof)))
                }
                "code" -> {
                    val code = Pairing.code(peer, identity.fingerprint) ?: return
                    phase = Phase.ByCode(computer, answered = false)
                    onPairing?.invoke(PairingEvent.CodeRequested(computer, code))
                }
                else -> sendThenClose(Wire.frame(Msg.PAIR_RESULT, PairMsg.result(false, "unsupported")))
            }
        }

        private fun finishPairing(accepted: Boolean, reason: String) {
            val computer = when (val p = phase) {
                is Phase.ByScan -> p.computer
                is Phase.ByCode -> {
                    if (!p.answered) {
                        // The computer gave up before this phone's user answered.
                        phase = Phase.First
                        onPairing?.invoke(PairingEvent.Failed("Pairing was cancelled on the computer."))
                        close()
                        return
                    }
                    p.computer
                }
                else -> return
            }
            if (!accepted) {
                phase = Phase.First
                onPairing?.invoke(PairingEvent.Failed(if (reason == "timeout") "Pairing timed out. Try again." else "The computer didn't accept the pairing."))
                close()
                return
            }
            paired.add(peer, computer)
            if (phase is Phase.ByScan) {
                scan = null
                reannounce()
            }
            phase = Phase.Paired
            Log.i(TAG, "paired with $computer")
            onPairing?.invoke(PairingEvent.Paired(computer))
        }
    }

    private sealed class Phase {
        /** Waiting for HELLO (paired computer) or PAIR_REQUEST. */
        object First : Phase()
        /** Sent PAIR_PROOF; waiting for the computer's PAIR_RESULT. */
        data class ByScan(val computer: String) : Phase()
        /** Showing the code; `answered` once this phone's user tapped Pair. */
        data class ByCode(val computer: String, val answered: Boolean) : Phase()
        /** Paired just now; HELLO comes next. */
        object Paired : Phase()
        object Streaming : Phase()
    }

    // endregion

    private companion object {
        const val TAG = "PocketWebcamNet"
        /** 200 ms of 10 ms chunks: more than this waiting means Wi-Fi stalled. */
        const val MAX_AUDIO_IN_FLIGHT = 20
    }
}

/** Everything a receiver sends is small (see "Framing" in PROTOCOL.md). */
private const val MAX_INBOUND_PAYLOAD = 1024 * 1024

/** A receiver that hasn't said HELLO after this long is dropped. */
private const val HELLO_DEADLINE_NANOS = 10_000_000_000L

/** While pairing, people compare codes: give them time. */
private const val PAIRING_DEADLINE_MILLIS = 100_000

/** A scanned QR code can be used for this long. */
private const val SCAN_VALID_NANOS = 5 * 60 * 1_000_000_000L

/** Tells the writer to stop after what's queued before it. */
private val CLOSE = ByteArray(0)
