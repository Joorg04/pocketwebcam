package com.pocketwebcam.app.net

import android.content.Context
import android.net.Uri
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import org.json.JSONObject
import java.math.BigInteger
import java.net.Socket
import java.security.KeyPairGenerator
import java.security.KeyStore
import java.security.MessageDigest
import java.security.Principal
import java.security.PrivateKey
import java.security.SecureRandom
import java.security.cert.X509Certificate
import java.security.spec.ECGenParameterSpec
import java.util.Base64
import java.util.Date
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec
import javax.net.ssl.SSLContext
import javax.net.ssl.SSLEngine
import javax.net.ssl.X509ExtendedKeyManager
import javax.net.ssl.X509ExtendedTrustManager
import javax.security.auth.x500.X500Principal

/**
 * Pairing rules from PROTOCOL.md ("Encryption", "Pairing"). The values must match the
 * receiver's byte for byte: `protocol-fixtures/pairing.json` has the test vectors.
 */
object Pairing {
    private val b64 = Base64.getUrlEncoder().withoutPadding()
    private val unb64 = Base64.getUrlDecoder()

    fun base64url(bytes: ByteArray): String = b64.encodeToString(bytes)
    fun unbase64url(text: String): ByteArray? = try { unb64.decode(text) } catch (_: IllegalArgumentException) { null }

    private fun sha256(vararg parts: ByteArray): ByteArray =
        MessageDigest.getInstance("SHA-256").run { parts.forEach(::update); digest() }

    /** SHA-256 of a certificate (DER), base64url without padding. */
    fun fingerprint(certificateDer: ByteArray): String = base64url(sha256(certificateDer))

    /** TXT `pair`: first 8 bytes of SHA-256(secret), lowercase hex. */
    fun tag(secret: ByteArray): String = sha256(secret).take(8).joinToString("") { "%02x".format(it) }

    /** The 6-digit code both screens show. */
    fun code(desktop: String, phone: String): String? {
        val d = unbase64url(desktop) ?: return null
        val p = unbase64url(phone) ?: return null
        val h = sha256("pocketwebcam pairing code".toByteArray(), d, p)
        val n = ((h[0].toLong() and 0xFF) shl 24) or ((h[1].toLong() and 0xFF) shl 16) or
            ((h[2].toLong() and 0xFF) shl 8) or (h[3].toLong() and 0xFF)
        return "%06d".format(n % 1_000_000)
    }

    fun spaced(code: String) = if (code.length == 6) "${code.take(3)} ${code.drop(3)}" else code

    /** PAIR_PROOF: shows the computer that this phone scanned its QR code. */
    fun proof(secret: ByteArray, phone: String, desktop: String): String? {
        val p = unbase64url(phone) ?: return null
        val d = unbase64url(desktop) ?: return null
        val mac = Mac.getInstance("HmacSHA256").apply { init(SecretKeySpec(secret, "HmacSHA256")) }
        mac.update("pocketwebcam pairing proof".toByteArray())
        mac.update(p)
        mac.update(d)
        return base64url(mac.doFinal())
    }

    /** What a scanned QR code says. */
    data class Scan(val desktop: String, val secret: ByteArray, val name: String) {
        override fun equals(other: Any?) =
            other is Scan && desktop == other.desktop && secret.contentEquals(other.secret) && name == other.name
        override fun hashCode() = desktop.hashCode()
    }

    /** Reads `pocketwebcam://pair?v=2&fp=…&s=…&n=…`; null if it isn't one this app understands. */
    fun parse(link: String): Scan? {
        val uri = try { Uri.parse(link) } catch (_: Exception) { return null }
        if (uri.scheme != "pocketwebcam" || uri.host != "pair") return null
        return parse(uri.getQueryParameter("v"), uri.getQueryParameter("fp"), uri.getQueryParameter("s"), uri.getQueryParameter("n"))
    }

    /** The checks behind [parse], on already decoded query values (testable without Android). */
    fun parse(v: String?, fp: String?, s: String?, n: String?): Scan? {
        if (v != "2" || fp == null || s == null) return null
        if (unbase64url(fp)?.size != 32) return null
        val secret = unbase64url(s)?.takeIf { it.size == 16 } ?: return null
        return Scan(fp, secret, n?.takeIf { it.isNotEmpty() } ?: "Computer")
    }
}

/** The computers this phone is paired with: fingerprint to name. Kept until forgotten. */
class PairedComputers(context: Context) {
    private val prefs = context.getSharedPreferences("paired_computers", Context.MODE_PRIVATE)

    @Synchronized
    fun all(): List<Pair<String, String>> =
        prefs.all.mapNotNull { (fp, name) -> (name as? String)?.let { fp to it } }.sortedBy { it.second.lowercase() }

    @Synchronized
    fun contains(fingerprint: String) = prefs.contains(fingerprint)

    @Synchronized
    fun add(fingerprint: String, name: String) = prefs.edit().putString(fingerprint, name).apply()

    @Synchronized
    fun forget(fingerprint: String) = prefs.edit().remove(fingerprint).apply()
}

/**
 * This phone's key in AndroidKeyStore (never leaves the device) and the self-signed
 * certificate the KeyStore makes for it.
 */
class PhoneIdentity private constructor(val key: PrivateKey, val certificate: X509Certificate) {
    val fingerprint: String = Pairing.fingerprint(certificate.encoded)

    /** TLS 1.3 for the phone's server side: our certificate, any client certificate (pinned later). */
    fun serverContext(): SSLContext = SSLContext.getInstance("TLSv1.3").apply {
        init(arrayOf(KeyManager(this@PhoneIdentity)), arrayOf(AnyClient), SecureRandom())
    }

    companion object {
        private const val ALIAS = "pocketwebcam-identity"

        fun loadOrCreate(): PhoneIdentity {
            val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
            (store.getEntry(ALIAS, null) as? KeyStore.PrivateKeyEntry)?.let { entry ->
                (entry.certificate as? X509Certificate)?.let { return PhoneIdentity(entry.privateKey, it) }
            }
            val spec = KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_SIGN)
                .setAlgorithmParameterSpec(ECGenParameterSpec("secp256r1"))
                .setDigests(KeyProperties.DIGEST_NONE, KeyProperties.DIGEST_SHA256, KeyProperties.DIGEST_SHA384, KeyProperties.DIGEST_SHA512)
                .setCertificateSubject(X500Principal("CN=Pocket Webcam"))
                .setCertificateSerialNumber(BigInteger(127, SecureRandom()).add(BigInteger.ONE))
                .setCertificateNotBefore(Date(1_767_225_600_000L)) // 2026-01-01
                .setCertificateNotAfter(Date(64_060_588_800_000L)) // 4000-01-01
                .build()
            KeyPairGenerator.getInstance(KeyProperties.KEY_ALGORITHM_EC, "AndroidKeyStore").apply { initialize(spec) }.generateKeyPair()
            val entry = store.getEntry(ALIAS, null) as KeyStore.PrivateKeyEntry
            return PhoneIdentity(entry.privateKey, entry.certificate as X509Certificate)
        }
    }

    /** Offers our one certificate for the server side. */
    private class KeyManager(private val id: PhoneIdentity) : X509ExtendedKeyManager() {
        override fun chooseServerAlias(keyType: String?, issuers: Array<out Principal>?, socket: Socket?) = ALIAS
        override fun chooseEngineServerAlias(keyType: String?, issuers: Array<out Principal>?, engine: SSLEngine?) = ALIAS
        override fun getCertificateChain(alias: String?) = arrayOf(id.certificate)
        override fun getPrivateKey(alias: String?) = id.key
        override fun getServerAliases(keyType: String?, issuers: Array<out Principal>?) = arrayOf(ALIAS)
        override fun getClientAliases(keyType: String?, issuers: Array<out Principal>?) = null
        override fun chooseClientAlias(keyType: Array<out String>?, issuers: Array<out Principal>?, socket: Socket?) = null
    }

    /**
     * Any client certificate passes the handshake (TLS still checks the computer holds its
     * key). Whether it belongs to a paired computer is decided right after, by fingerprint.
     */
    private object AnyClient : X509ExtendedTrustManager() {
        override fun checkClientTrusted(chain: Array<out X509Certificate>?, authType: String?) {}
        override fun checkClientTrusted(chain: Array<out X509Certificate>?, authType: String?, socket: Socket?) {}
        override fun checkClientTrusted(chain: Array<out X509Certificate>?, authType: String?, engine: SSLEngine?) {}
        override fun checkServerTrusted(chain: Array<out X509Certificate>?, authType: String?) = throw java.security.cert.CertificateException("server side only")
        override fun checkServerTrusted(chain: Array<out X509Certificate>?, authType: String?, socket: Socket?) = throw java.security.cert.CertificateException("server side only")
        override fun checkServerTrusted(chain: Array<out X509Certificate>?, authType: String?, engine: SSLEngine?) = throw java.security.cert.CertificateException("server side only")
        override fun getAcceptedIssuers(): Array<X509Certificate> = emptyArray()
    }
}

/** JSON for the pairing messages. */
object PairMsg {
    fun proof(proof: String): JSONObject = JSONObject().put("proof", proof)
    fun result(accepted: Boolean, reason: String? = null): JSONObject =
        JSONObject().put("accepted", accepted).apply { if (reason != null) put("reason", reason) }
}
