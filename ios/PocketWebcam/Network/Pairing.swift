import CryptoKit
import Foundation
import Security

/// Pairing rules from PROTOCOL.md ("Encryption", "Pairing"). The values must match the
/// receiver's byte for byte: `protocol-fixtures/pairing.json` has the test vectors.
enum Pairing {
    /// SHA-256 of a certificate (DER), base64url without padding.
    static func fingerprint(of certificateDER: Data) -> String {
        base64url(Data(SHA256.hash(data: certificateDER)))
    }

    /// TXT `pair`: first 8 bytes of SHA-256(secret), lowercase hex.
    static func tag(secret: Data) -> String {
        SHA256.hash(data: secret).prefix(8).map { String(format: "%02x", $0) }.joined()
    }

    /// The 6-digit code both screens show.
    static func code(desktop: String, phone: String) -> String? {
        guard let d = unbase64url(desktop), let p = unbase64url(phone) else { return nil }
        var input = Data("pocketwebcam pairing code".utf8)
        input.append(d)
        input.append(p)
        let hash = Array(SHA256.hash(data: input))
        let n = (UInt32(hash[0]) << 24 | UInt32(hash[1]) << 16 | UInt32(hash[2]) << 8 | UInt32(hash[3])) % 1_000_000
        return String(format: "%06u", n)
    }

    /// "123456" -> "123 456".
    static func spaced(_ code: String) -> String {
        code.count == 6 ? "\(code.prefix(3)) \(code.suffix(3))" : code
    }

    /// PAIR_PROOF: shows the computer that this phone scanned its QR code.
    static func proof(secret: Data, phone: String, desktop: String) -> String? {
        guard let p = unbase64url(phone), let d = unbase64url(desktop) else { return nil }
        var message = Data("pocketwebcam pairing proof".utf8)
        message.append(p)
        message.append(d)
        let mac = HMAC<SHA256>.authenticationCode(for: message, using: SymmetricKey(data: secret))
        return base64url(Data(mac))
    }

    /// What a scanned QR code says.
    struct Scan: Equatable {
        var desktop: String
        var secret: Data
        var name: String
    }

    /// Reads `pocketwebcam://pair?v=2&fp=…&s=…&n=…`. Nil if it isn't a pairing link this
    /// app understands.
    static func parse(_ link: String) -> Scan? {
        guard let parts = URLComponents(string: link), parts.scheme == "pocketwebcam", parts.host == "pair" else { return nil }
        var q: [String: String] = [:]
        for item in parts.queryItems ?? [] { q[item.name] = item.value ?? "" }
        guard q["v"] == "2", let fp = q["fp"], let fpBytes = unbase64url(fp), fpBytes.count == 32,
              let s = q["s"], let secret = unbase64url(s), secret.count == 16 else { return nil }
        return Scan(desktop: fp, secret: secret, name: q["n"] ?? "Computer")
    }

    static func base64url(_ data: Data) -> String {
        data.base64EncodedString()
            .replacingOccurrences(of: "+", with: "-")
            .replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "=", with: "")
    }

    static func unbase64url(_ text: String) -> Data? {
        var s = text.replacingOccurrences(of: "-", with: "+").replacingOccurrences(of: "_", with: "/")
        while s.count % 4 != 0 { s += "=" }
        return Data(base64Encoded: s)
    }
}

/// The computers this phone is paired with: fingerprint -> name. Fingerprints aren't
/// secret, so UserDefaults is fine. Pairings last until the user forgets them.
final class PairedComputers: @unchecked Sendable {
    private let lock = NSLock()
    private let key = "pairedComputers"
    private var computers: [String: String]

    init() {
        computers = (UserDefaults.standard.dictionary(forKey: key) as? [String: String]) ?? [:]
    }

    var all: [(fingerprint: String, name: String)] {
        lock.lock(); defer { lock.unlock() }
        return computers
            .map { (fingerprint: $0.key, name: $0.value) }
            .sorted { $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending }
    }

    func contains(_ fingerprint: String) -> Bool {
        lock.lock(); defer { lock.unlock() }
        return computers[fingerprint] != nil
    }

    func add(_ fingerprint: String, name: String) {
        lock.lock()
        computers[fingerprint] = name
        let copy = computers
        lock.unlock()
        UserDefaults.standard.set(copy, forKey: key)
    }

    func forget(_ fingerprint: String) {
        lock.lock()
        computers[fingerprint] = nil
        let copy = computers
        lock.unlock()
        UserDefaults.standard.set(copy, forKey: key)
    }
}

/// This phone's key (in the Keychain, never leaves the device) and its self-signed
/// certificate, made on first use.
struct PhoneIdentity {
    let identity: SecIdentity
    let certificateDER: Data
    let fingerprint: String

    private static let label = "Pocket Webcam identity"
    private static let keyTag = Data("com.pocketwebcam.identity.key".utf8)

    enum IdentityError: Error { case keychain(OSStatus), key(String), certificate }

    /// The saved identity, or a new one.
    static func loadOrCreate() throws -> PhoneIdentity {
        if let saved = load() { return saved }
        deleteAll()
        return try create()
    }

    static func load() -> PhoneIdentity? {
        let query: [CFString: Any] = [
            kSecClass: kSecClassIdentity,
            kSecAttrLabel: label,
            kSecReturnRef: true,
        ]
        var item: CFTypeRef?
        guard SecItemCopyMatching(query as CFDictionary, &item) == errSecSuccess, let item,
              CFGetTypeID(item) == SecIdentityGetTypeID() else { return nil }
        let identity = item as! SecIdentity
        var cert: SecCertificate?
        guard SecIdentityCopyCertificate(identity, &cert) == errSecSuccess, let cert else { return nil }
        let der = SecCertificateCopyData(cert) as Data
        return PhoneIdentity(identity: identity, certificateDER: der, fingerprint: Pairing.fingerprint(of: der))
    }

    /// Removes this app's key and certificate (a half-made identity, or for a fresh start).
    static func deleteAll() {
        SecItemDelete([kSecClass: kSecClassKey, kSecAttrApplicationTag: keyTag] as CFDictionary)
        SecItemDelete([kSecClass: kSecClassCertificate, kSecAttrLabel: label] as CFDictionary)
    }

    private static func create() throws -> PhoneIdentity {
        let attributes: [CFString: Any] = [
            kSecAttrKeyType: kSecAttrKeyTypeECSECPrimeRandom,
            kSecAttrKeySizeInBits: 256,
            kSecPrivateKeyAttrs: [
                kSecAttrIsPermanent: true,
                kSecAttrApplicationTag: keyTag,
                // Stays on this phone: not in backups or on a new phone (pair that one again).
                kSecAttrAccessible: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
            ] as [CFString: Any],
        ]
        var error: Unmanaged<CFError>?
        guard let privateKey = SecKeyCreateRandomKey(attributes as CFDictionary, &error) else {
            throw IdentityError.key(error?.takeRetainedValue().localizedDescription ?? "unknown")
        }
        guard let publicKey = SecKeyCopyPublicKey(privateKey),
              let point = SecKeyCopyExternalRepresentation(publicKey, &error) as Data? else {
            throw IdentityError.key(error?.takeRetainedValue().localizedDescription ?? "no public key")
        }
        let tbs = Certificate.tbs(publicKeyPoint: point, serial: Certificate.randomSerial())
        guard let signature = SecKeyCreateSignature(privateKey, .ecdsaSignatureMessageX962SHA256, tbs as CFData, &error) as Data? else {
            throw IdentityError.key(error?.takeRetainedValue().localizedDescription ?? "signing failed")
        }
        let der = Certificate.assemble(tbs: tbs, signature: signature)
        guard let cert = SecCertificateCreateWithData(nil, der as CFData) else { throw IdentityError.certificate }
        let add: [CFString: Any] = [
            kSecClass: kSecClassCertificate,
            kSecValueRef: cert,
            kSecAttrLabel: label,
            kSecAttrAccessible: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
        ]
        let status = SecItemAdd(add as CFDictionary, nil)
        guard status == errSecSuccess || status == errSecDuplicateItem else { throw IdentityError.keychain(status) }
        guard let made = load() else { throw IdentityError.keychain(errSecItemNotFound) }
        return made
    }
}

/// A minimal X.509 v3 certificate for an ECDSA P-256 key, in DER. Only what TLS needs:
/// names and dates mean nothing here (peers are trusted by fingerprint).
enum Certificate {
    static func randomSerial() -> Data {
        var bytes = [UInt8](repeating: 0, count: 16)
        _ = SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes)
        bytes[0] &= 0x7F  // positive
        bytes[0] |= 0x01  // and no leading zero byte
        return Data(bytes)
    }

    /// TBSCertificate for a self-signed certificate with CN "Pocket Webcam".
    static func tbs(publicKeyPoint: Data, serial: Data) -> Data {
        let name = DER.sequence([DER.set([DER.sequence([DER.oid(commonName), DER.utf8("Pocket Webcam")])])])
        let validity = DER.sequence([DER.utcTime("260101000000Z"), DER.generalizedTime("40000101000000Z")])
        let spki = DER.sequence([
            DER.sequence([DER.oid(ecPublicKey), DER.oid(prime256v1)]),
            DER.bitString(publicKeyPoint),
        ])
        return DER.sequence([
            DER.explicit(0, DER.integer(Data([2]))),  // v3
            DER.integer(serial),
            DER.sequence([DER.oid(ecdsaWithSHA256)]),
            name,
            validity,
            name,
            spki,
        ])
    }

    static func assemble(tbs: Data, signature: Data) -> Data {
        DER.sequence([tbs, DER.sequence([DER.oid(ecdsaWithSHA256)]), DER.bitString(signature)])
    }

    static let ecdsaWithSHA256: [UInt8] = [0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x04, 0x03, 0x02]  // 1.2.840.10045.4.3.2
    static let ecPublicKey: [UInt8] = [0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x02, 0x01]  // 1.2.840.10045.2.1
    static let prime256v1: [UInt8] = [0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x03, 0x01, 0x07]  // 1.2.840.10045.3.1.7
    static let commonName: [UInt8] = [0x55, 0x04, 0x03]  // 2.5.4.3
}

/// Just enough DER for `Certificate`.
enum DER {
    static func tlv(_ tag: UInt8, _ content: Data) -> Data {
        var out = Data([tag])
        let n = content.count
        if n < 0x80 {
            out.append(UInt8(n))
        } else {
            var len = withUnsafeBytes(of: UInt32(n).bigEndian) { Data($0) }
            while len.first == 0 { len.removeFirst() }
            out.append(0x80 | UInt8(len.count))
            out.append(len)
        }
        out.append(content)
        return out
    }

    static func sequence(_ items: [Data]) -> Data { tlv(0x30, items.reduce(Data(), +)) }
    static func set(_ items: [Data]) -> Data { tlv(0x31, items.reduce(Data(), +)) }
    static func oid(_ bytes: [UInt8]) -> Data { tlv(0x06, Data(bytes)) }
    static func utf8(_ s: String) -> Data { tlv(0x0C, Data(s.utf8)) }
    static func utcTime(_ s: String) -> Data { tlv(0x17, Data(s.utf8)) }
    static func generalizedTime(_ s: String) -> Data { tlv(0x18, Data(s.utf8)) }
    static func bitString(_ bytes: Data) -> Data { tlv(0x03, Data([0]) + bytes) }
    static func explicit(_ n: UInt8, _ inner: Data) -> Data { tlv(0xA0 | n, inner) }

    /// A non-negative INTEGER: a leading zero is added when the top bit is set.
    static func integer(_ bytes: Data) -> Data {
        var b = bytes
        while b.count > 1, b.first == 0, (b[b.startIndex + 1] & 0x80) == 0 { b.removeFirst() }
        if let first = b.first, first & 0x80 != 0 { b.insert(0, at: 0) }
        return tlv(0x02, b)
    }
}
