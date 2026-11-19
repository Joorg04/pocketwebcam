import Security
import XCTest
@testable import PocketWebcam

/// The pairing rules must match the receiver's exactly: protocol-fixtures/pairing.json
/// holds the shared test vectors (the Rust and Android tests check the same file).
final class PairingTests: XCTestCase {
    private func fixture() throws -> [String: String] {
        let url = try XCTUnwrap(Bundle(for: Self.self).url(forResource: "pairing", withExtension: "json"))
        let json = try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: String]
        return try XCTUnwrap(json)
    }

    private func hex(_ s: String) -> Data {
        var data = Data()
        var i = s.startIndex
        while i < s.endIndex {
            let j = s.index(i, offsetBy: 2)
            data.append(UInt8(s[i..<j], radix: 16)!)
            i = j
        }
        return data
    }

    func testMatchesTheSharedVectors() throws {
        let f = try fixture()
        XCTAssertEqual(Pairing.fingerprint(of: hex(f["certificateDerHex"]!)), f["certificateFingerprint"])
        let secret = hex(f["secretHex"]!)
        XCTAssertEqual(Pairing.tag(secret: secret), f["txtPairTag"])
        XCTAssertEqual(Pairing.code(desktop: f["desktopFingerprint"]!, phone: f["phoneFingerprint"]!), f["code"])
        XCTAssertEqual(Pairing.proof(secret: secret, phone: f["phoneFingerprint"]!, desktop: f["desktopFingerprint"]!), f["proof"])

        let scan = try XCTUnwrap(Pairing.parse(f["qrUrl"]!))
        XCTAssertEqual(scan, Pairing.Scan(desktop: f["desktopFingerprint"]!, secret: secret, name: f["computerName"]!))
    }

    func testRejectsLinksItDoesntUnderstand() throws {
        let url = try fixture()["qrUrl"]!
        XCTAssertNil(Pairing.parse(url.replacingOccurrences(of: "v=2", with: "v=3")))
        XCTAssertNil(Pairing.parse("https://pocketwebcam.com/pair?v=2"))
        XCTAssertNil(Pairing.parse("pocketwebcam://pair?v=2&fp=short&s=AAECAwQFBgcICQoLDA0ODw"))
        XCTAssertEqual(Pairing.spaced("012345"), "012 345")
    }

    func testTheCertificateParsesAndIsSignedByItsKey() throws {
        let attributes: [CFString: Any] = [kSecAttrKeyType: kSecAttrKeyTypeECSECPrimeRandom, kSecAttrKeySizeInBits: 256]
        var error: Unmanaged<CFError>?
        let key = try XCTUnwrap(SecKeyCreateRandomKey(attributes as CFDictionary, &error))
        let publicKey = try XCTUnwrap(SecKeyCopyPublicKey(key))
        let point = try XCTUnwrap(SecKeyCopyExternalRepresentation(publicKey, &error) as Data?)
        let tbs = Certificate.tbs(publicKeyPoint: point, serial: Certificate.randomSerial())
        let signature = try XCTUnwrap(SecKeyCreateSignature(key, .ecdsaSignatureMessageX962SHA256, tbs as CFData, &error) as Data?)
        let der = Certificate.assemble(tbs: tbs, signature: signature)

        let cert = try XCTUnwrap(SecCertificateCreateWithData(nil, der as CFData), "iOS can't read the certificate")
        let certKey = try XCTUnwrap(SecCertificateCopyKey(cert))
        XCTAssertEqual(SecKeyCopyExternalRepresentation(certKey, nil) as Data?, point)
        XCTAssertTrue(SecKeyVerifySignature(certKey, .ecdsaSignatureMessageX962SHA256, tbs as CFData, signature as CFData, nil))
        XCTAssertEqual(SecCertificateCopySubjectSummary(cert) as String?, "Pocket Webcam")
    }

    func testDERLengthsAndIntegers() {
        XCTAssertEqual(DER.tlv(0x04, Data(count: 3)), Data([0x04, 0x03, 0, 0, 0]))
        XCTAssertEqual(DER.tlv(0x04, Data(count: 200)).prefix(3), Data([0x04, 0x81, 200]))
        XCTAssertEqual(DER.tlv(0x04, Data(count: 300)).prefix(4), Data([0x04, 0x82, 0x01, 0x2C]))
        XCTAssertEqual(DER.integer(Data([0x80])), Data([0x02, 0x02, 0x00, 0x80]))
        XCTAssertEqual(DER.integer(Data([0x00, 0x05])), Data([0x02, 0x01, 0x05]))
    }

    func testTheIdentityIsMadeOnceAndKept() throws {
        PhoneIdentity.deleteAll()
        defer { PhoneIdentity.deleteAll() }
        let made = try PhoneIdentity.loadOrCreate()
        let again = try PhoneIdentity.loadOrCreate()
        XCTAssertEqual(made.fingerprint, again.fingerprint)
        XCTAssertEqual(made.fingerprint, Pairing.fingerprint(of: made.certificateDER))
        XCTAssertNotNil(sec_identity_create(made.identity), "Network.framework can use it for TLS")
    }

    func testPairedComputersAreRememberedAndForgotten() {
        let store = PairedComputers()
        let fp = "test-\(UUID().uuidString)"
        store.add(fp, name: "Test PC")
        XCTAssertTrue(PairedComputers().contains(fp))
        store.forget(fp)
        XCTAssertFalse(PairedComputers().contains(fp))
    }
}
