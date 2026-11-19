package com.pocketwebcam.app.net

import org.json.JSONObject
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Test
import java.io.File
import java.net.URLDecoder

/**
 * The pairing rules must match the receiver's exactly: `protocol-fixtures/pairing.json`
 * holds the shared test vectors (the Rust and iOS tests check the same file).
 */
class PairingTest {
    private val f: JSONObject = run {
        val dir = generateSequence(File("").absoluteFile) { it.parentFile }
            .map { File(it, "protocol-fixtures") }
            .first { it.isDirectory }
        JSONObject(File(dir, "pairing.json").readText())
    }

    private fun hex(s: String) = ByteArray(s.length / 2) { s.substring(it * 2, it * 2 + 2).toInt(16).toByte() }

    /** Query values of the QR link, decoded the way android.net.Uri does it. */
    private fun query(link: String): Map<String, String> =
        link.substringAfter('?').split('&').associate {
            val (k, v) = it.split('=', limit = 2)
            k to URLDecoder.decode(v, "UTF-8")
        }

    @Test
    fun matchesTheSharedVectors() {
        assertEquals(f.getString("certificateFingerprint"), Pairing.fingerprint(hex(f.getString("certificateDerHex"))))
        val secret = hex(f.getString("secretHex"))
        val desktop = f.getString("desktopFingerprint")
        val phone = f.getString("phoneFingerprint")
        assertEquals(f.getString("txtPairTag"), Pairing.tag(secret))
        assertEquals(f.getString("code"), Pairing.code(desktop, phone))
        assertEquals(f.getString("proof"), Pairing.proof(secret, phone, desktop))

        val q = query(f.getString("qrUrl"))
        val scan = Pairing.parse(q["v"], q["fp"], q["s"], q["n"])!!
        assertEquals(desktop, scan.desktop)
        assertArrayEquals(secret, scan.secret)
        assertEquals(f.getString("computerName"), scan.name)
    }

    @Test
    fun rejectsLinksItDoesntUnderstand() {
        val q = query(f.getString("qrUrl"))
        assertNull(Pairing.parse("3", q["fp"], q["s"], q["n"]))
        assertNull(Pairing.parse("2", "short", q["s"], q["n"]))
        assertNull(Pairing.parse("2", q["fp"], "AAEC", q["n"]))
        assertNull(Pairing.parse("2", null, q["s"], q["n"]))
    }

    @Test
    fun codesDifferWhenACertificateDiffers() {
        val a = Pairing.base64url(ByteArray(32) { 1 })
        val b = Pairing.base64url(ByteArray(32) { 2 })
        assertEquals(6, Pairing.code(a, b)!!.length)
        assertNotEquals(Pairing.code(a, b), Pairing.code(b, a))
        assertEquals("012 345", Pairing.spaced("012345"))
    }
}
