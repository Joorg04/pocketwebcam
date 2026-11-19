//! Pairing rules shared by the desktop and the phone apps (PROTOCOL.md, "Pairing").
//!
//! Every app makes a key pair and a self-signed certificate once. A peer is known by the
//! SHA-256 fingerprint of its certificate, and pairing means both sides remember the other's
//! fingerprint. The values below (QR link, TXT tag, 6-digit code, proof) must come out
//! byte for byte the same in Rust, Swift and Kotlin: `protocol-fixtures/pairing.json`
//! has test vectors that all three check.

use std::fmt;
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use ring::rand::{SecureRandom, SystemRandom};
use ring::{digest, hmac};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

/// How long the secret in a QR code can be used.
pub const QR_VALID_FOR: std::time::Duration = std::time::Duration::from_secs(5 * 60);
/// How long both people get to compare the 6-digit code.
pub const CODE_VALID_FOR: std::time::Duration = std::time::Duration::from_secs(90);

const CODE_LABEL: &[u8] = b"pocketwebcam pairing code";
const PROOF_LABEL: &[u8] = b"pocketwebcam pairing proof";

/// SHA-256 of a certificate (DER). Written as base64url without padding (43 characters).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Fingerprint(pub [u8; 32]);

impl Fingerprint {
    pub fn of(cert_der: &[u8]) -> Fingerprint {
        let d = digest::digest(&digest::SHA256, cert_der);
        Fingerprint(d.as_ref().try_into().expect("SHA-256 is 32 bytes"))
    }

    pub fn parse(text: &str) -> Result<Fingerprint> {
        let bytes = URL_SAFE_NO_PAD.decode(text.trim()).context("fingerprint isn't base64url")?;
        Ok(Fingerprint(bytes.try_into().map_err(|_| anyhow!("a fingerprint is 32 bytes"))?))
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&URL_SAFE_NO_PAD.encode(self.0))
    }
}

impl fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Fingerprint({self})")
    }
}

/// The one-time secret in a QR code: 16 random bytes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Secret(pub [u8; 16]);

impl Secret {
    pub fn random() -> Secret {
        let mut s = [0u8; 16];
        SystemRandom::new().fill(&mut s).expect("the system has randomness");
        Secret(s)
    }

    /// What the phone puts in its TXT record (`pair=`) after scanning this secret, so the
    /// desktop knows which phone to connect to: the first 8 bytes of SHA-256(secret), hex.
    /// It gives nothing away: the secret can't be worked out from it.
    pub fn tag(&self) -> String {
        digest::digest(&digest::SHA256, &self.0).as_ref()[..8].iter().map(|b| format!("{b:02x}")).collect()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(…)")
    }
}

/// The link in the QR code the desktop shows.
pub fn qr_url(desktop: &Fingerprint, secret: &Secret, name: &str) -> String {
    format!("pocketwebcam://pair?v=2&fp={desktop}&s={}&n={}", URL_SAFE_NO_PAD.encode(secret.0), percent_encode(name))
}

/// What a phone reads from a scanned QR code.
#[derive(Debug, PartialEq, Eq)]
pub struct QrCode {
    pub desktop: Fingerprint,
    pub secret: Secret,
    pub name: String,
}

/// Reads a QR link (what the phone apps do; here for the tests and the fake phone).
pub fn parse_qr_url(url: &str) -> Result<QrCode> {
    let query = url.strip_prefix("pocketwebcam://pair?").ok_or_else(|| anyhow!("not a Pocket Webcam pairing code"))?;
    let (mut v, mut fp, mut s, mut n) = (None, None, None, None);
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        match key {
            "v" => v = Some(value),
            "fp" => fp = Some(value),
            "s" => s = Some(value),
            "n" => n = Some(value),
            _ => {} // newer keys are ignored
        }
    }
    if v != Some("2") {
        bail!("this pairing code is for a different version of Pocket Webcam");
    }
    let secret = URL_SAFE_NO_PAD.decode(s.ok_or_else(|| anyhow!("no secret"))?)?;
    Ok(QrCode {
        desktop: Fingerprint::parse(fp.ok_or_else(|| anyhow!("no fingerprint"))?)?,
        secret: Secret(secret.try_into().map_err(|_| anyhow!("the secret is 16 bytes"))?),
        name: percent_decode(n.unwrap_or(""))?,
    })
}

/// The 6-digit code both screens show when pairing without the QR code. A man in the
/// middle would have different certificates on each side, so the two codes wouldn't match.
pub fn code(desktop: &Fingerprint, phone: &Fingerprint) -> String {
    let mut ctx = digest::Context::new(&digest::SHA256);
    ctx.update(CODE_LABEL);
    ctx.update(&desktop.0);
    ctx.update(&phone.0);
    let d = ctx.finish();
    let n = u32::from_be_bytes(d.as_ref()[..4].try_into().unwrap()) % 1_000_000;
    format!("{n:06}")
}

/// `123456` -> `123 456`, easier to compare at a glance.
pub fn spaced(code: &str) -> String {
    if code.len() == 6 {
        format!("{} {}", &code[..3], &code[3..])
    } else {
        code.to_string()
    }
}

/// PAIR_PROOF: the phone shows that it scanned `secret`, for exactly these two certificates.
pub fn proof(secret: &Secret, phone: &Fingerprint, desktop: &Fingerprint) -> [u8; 32] {
    let key = hmac::Key::new(hmac::HMAC_SHA256, &secret.0);
    let mut ctx = hmac::Context::with_key(&key);
    ctx.update(PROOF_LABEL);
    ctx.update(&phone.0);
    ctx.update(&desktop.0);
    ctx.sign().as_ref().try_into().unwrap()
}

/// Checks a PAIR_PROOF in constant time.
pub fn proof_is_valid(secret: &Secret, phone: &Fingerprint, desktop: &Fingerprint, proof: &[u8]) -> bool {
    let key = hmac::Key::new(hmac::HMAC_SHA256, &secret.0);
    let mut msg = PROOF_LABEL.to_vec();
    msg.extend_from_slice(&phone.0);
    msg.extend_from_slice(&desktop.0);
    hmac::verify(&key, &msg, proof).is_ok()
}

pub fn encode_proof(proof: &[u8; 32]) -> String {
    URL_SAFE_NO_PAD.encode(proof)
}

pub fn decode_proof(text: &str) -> Result<Vec<u8>> {
    Ok(URL_SAFE_NO_PAD.decode(text)?)
}

fn percent_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn percent_decode(s: &str) -> Result<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = s.get(i + 1..i + 3).ok_or_else(|| anyhow!("bad %-escape"))?;
                out.push(u8::from_str_radix(hex, 16).context("bad %-escape")?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    Ok(String::from_utf8(out)?)
}

/// This computer's key and certificate, made once and kept in the config folder.
pub struct Identity {
    pub cert: CertificateDer<'static>,
    pub key: PrivateKeyDer<'static>,
    pub fingerprint: Fingerprint,
}

impl Clone for Identity {
    fn clone(&self) -> Self {
        Identity { cert: self.cert.clone(), key: self.key.clone_key(), fingerprint: self.fingerprint }
    }
}

impl Identity {
    /// Loads the identity from `path`, or makes a new one there.
    pub fn load_or_create(path: &Path) -> Result<Identity> {
        if path.exists() {
            return Self::load(path).with_context(|| format!("reading {}", path.display()));
        }
        let (identity, pem) = Self::generate()?;
        write_private(path, pem.as_bytes()).with_context(|| format!("writing {}", path.display()))?;
        log::info!("made this computer's pairing identity {}", identity.fingerprint);
        Ok(identity)
    }

    /// A new key and certificate, plus both as PEM text for saving.
    pub fn generate() -> Result<(Identity, String)> {
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
        let mut params = rcgen::CertificateParams::new(vec!["pocketwebcam".to_string()])?;
        params.distinguished_name = rcgen::DistinguishedName::new();
        params.distinguished_name.push(rcgen::DnType::CommonName, "Pocket Webcam");
        let cert = params.self_signed(&key)?;
        let pem = format!("{}{}", cert.pem(), key.serialize_pem());
        Ok((Self::from_pem(pem.as_bytes())?, pem))
    }

    fn load(path: &Path) -> Result<Identity> {
        Self::from_pem(&std::fs::read(path)?)
    }

    pub fn from_pem(pem: &[u8]) -> Result<Identity> {
        let cert = CertificateDer::from_pem_slice(pem).map_err(|e| anyhow!("no certificate: {e}"))?;
        let key = PrivateKeyDer::from_pem_slice(pem).map_err(|e| anyhow!("no private key: {e}"))?;
        let fingerprint = Fingerprint::of(&cert);
        Ok(Identity { cert, key, fingerprint })
    }
}

/// A phone this computer has paired with.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PairedPhone {
    /// Its certificate's fingerprint (base64url): what it's recognised by.
    pub fingerprint: String,
    /// Its name when it was paired, for the list in the app.
    pub name: String,
    /// The phone app's identifier (TXT `id`), if it sent one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

/// The paired phones, kept in `paired-phones.json` next to the settings. Pairings stay
/// until the user forgets the phone.
///
/// A file of its own, not part of `Settings`: the settings window saves its whole copy of
/// the settings, which must never put back a phone that was just forgotten or drop one
/// that was just paired.
pub struct PairedPhones {
    path: Option<std::path::PathBuf>,
    phones: std::sync::Mutex<Vec<PairedPhone>>,
}

impl PairedPhones {
    /// The list for this user (empty when there's no config folder).
    pub fn load() -> PairedPhones {
        Self::load_from(crate::paths::config_dir().map(|d| d.join("paired-phones.json")))
    }

    /// `None` keeps the list in memory only (tests, a computer without a home folder).
    pub fn load_from(path: Option<std::path::PathBuf>) -> PairedPhones {
        let phones = path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|bytes| serde_json::from_slice::<Vec<PairedPhone>>(&bytes).ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|p| Fingerprint::parse(&p.fingerprint).is_ok())
            .collect();
        PairedPhones { path, phones: std::sync::Mutex::new(phones) }
    }

    pub fn list(&self) -> Vec<PairedPhone> {
        self.phones.lock().unwrap().clone()
    }

    pub fn contains(&self, fp: &Fingerprint) -> bool {
        let fp = fp.to_string();
        self.phones.lock().unwrap().iter().any(|p| p.fingerprint == fp)
    }

    /// Remembers a phone (or updates its name), and saves.
    pub fn add(&self, fp: &Fingerprint, name: &str, id: Option<&str>) {
        let entry = PairedPhone { fingerprint: fp.to_string(), name: name.to_string(), id: id.map(str::to_string) };
        let mut phones = self.phones.lock().unwrap();
        phones.retain(|p| p.fingerprint != entry.fingerprint);
        phones.push(entry);
        self.save(&phones);
    }

    /// Keeps a paired phone's name and identifier current (from its HELLO); saves only on change.
    pub fn refresh(&self, fp: &Fingerprint, name: &str, id: Option<&str>) {
        let fp = fp.to_string();
        let mut phones = self.phones.lock().unwrap();
        let Some(p) = phones.iter_mut().find(|p| p.fingerprint == fp) else { return };
        if p.name == name && (id.is_none() || p.id.as_deref() == id) {
            return;
        }
        p.name = name.to_string();
        if id.is_some() {
            p.id = id.map(str::to_string);
        }
        self.save(&phones);
    }

    /// Forgets a phone; it has to be paired again before it can connect.
    pub fn forget(&self, fp: &Fingerprint) -> bool {
        let fp = fp.to_string();
        let mut phones = self.phones.lock().unwrap();
        let before = phones.len();
        phones.retain(|p| p.fingerprint != fp);
        let changed = phones.len() != before;
        if changed {
            self.save(&phones);
        }
        changed
    }

    fn save(&self, phones: &[PairedPhone]) {
        let Some(path) = &self.path else { return };
        match serde_json::to_vec_pretty(phones) {
            Ok(json) => {
                if let Err(e) = write_private(path, &json) {
                    log::warn!("could not save the paired phones to {}: {e}", path.display());
                }
            }
            Err(e) => log::warn!("could not save the paired phones: {e}"),
        }
    }
}

/// Writes a file only this user can read.
fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(&tmp)?;
    f.write_all(data)?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> serde_json::Value {
        serde_json::from_str(include_str!("../../protocol-fixtures/pairing.json")).unwrap()
    }

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn matches_the_shared_test_vectors() {
        let f = fixture();
        let cert = hex(f["certificateDerHex"].as_str().unwrap());
        assert_eq!(Fingerprint::of(&cert).to_string(), f["certificateFingerprint"].as_str().unwrap());

        let desktop = Fingerprint::parse(f["desktopFingerprint"].as_str().unwrap()).unwrap();
        let phone = Fingerprint::parse(f["phoneFingerprint"].as_str().unwrap()).unwrap();
        let secret = Secret(hex(f["secretHex"].as_str().unwrap()).try_into().unwrap());
        let name = f["computerName"].as_str().unwrap();

        assert_eq!(qr_url(&desktop, &secret, name), f["qrUrl"].as_str().unwrap());
        assert_eq!(secret.tag(), f["txtPairTag"].as_str().unwrap());
        assert_eq!(code(&desktop, &phone), f["code"].as_str().unwrap());
        assert_eq!(encode_proof(&proof(&secret, &phone, &desktop)), f["proof"].as_str().unwrap());

        let qr = parse_qr_url(f["qrUrl"].as_str().unwrap()).unwrap();
        assert_eq!(qr, QrCode { desktop, secret, name: name.to_string() });
    }

    #[test]
    fn a_proof_only_fits_its_own_secret_and_certificates() {
        let (a, b, c) = (Fingerprint([1; 32]), Fingerprint([2; 32]), Fingerprint([3; 32]));
        let secret = Secret([7; 16]);
        let p = proof(&secret, &a, &b);
        assert!(proof_is_valid(&secret, &a, &b, &p));
        assert!(!proof_is_valid(&Secret([8; 16]), &a, &b, &p), "another secret");
        assert!(!proof_is_valid(&secret, &c, &b, &p), "a man in the middle's phone certificate");
        assert!(!proof_is_valid(&secret, &b, &a, &p), "swapped");
        assert!(!proof_is_valid(&secret, &a, &b, &p[..31]));
    }

    #[test]
    fn codes_differ_when_a_certificate_differs() {
        let (a, b, c) = (Fingerprint([1; 32]), Fingerprint([2; 32]), Fingerprint([3; 32]));
        assert_eq!(code(&a, &b).len(), 6);
        assert_ne!(code(&a, &b), code(&a, &c));
        assert_ne!(code(&a, &b), code(&b, &a));
        assert_eq!(spaced("012345"), "012 345");
    }

    #[test]
    fn qr_links_survive_odd_names_and_reject_other_versions() {
        let (fp, secret) = (Fingerprint([9; 32]), Secret::random());
        for name in ["Nic's PC", "Øystein & co", "a=b?c", ""] {
            assert_eq!(parse_qr_url(&qr_url(&fp, &secret, name)).unwrap().name, name);
        }
        let v3 = qr_url(&fp, &secret, "x").replace("v=2", "v=3");
        assert!(parse_qr_url(&v3).is_err());
        assert!(parse_qr_url("https://example.com").is_err());
        assert_ne!(Secret::random(), Secret::random());
    }

    #[test]
    fn paired_phones_are_saved_and_forgotten() {
        let dir = std::env::temp_dir().join(format!("pw-paired-{}", std::process::id()));
        let path = dir.join("paired-phones.json");
        let (a, b) = (Fingerprint([1; 32]), Fingerprint([2; 32]));
        let store = PairedPhones::load_from(Some(path.clone()));
        assert!(!store.contains(&a));
        store.add(&a, "Nic's iPhone", Some("id-a"));
        store.add(&b, "Pixel", None);
        store.add(&a, "Renamed", Some("id-a")); // pairing again updates, never duplicates
        store.refresh(&b, "Pixel 8", Some("id-b"));
        store.refresh(&Fingerprint([3; 32]), "Stranger", None); // not paired: stays out

        let again = PairedPhones::load_from(Some(path.clone()));
        assert!(again.contains(&a) && again.contains(&b));
        assert_eq!(again.list().len(), 2);
        assert_eq!(again.list().iter().find(|p| p.fingerprint == a.to_string()).unwrap().name, "Renamed");
        let pixel = again.list().into_iter().find(|p| p.fingerprint == b.to_string()).unwrap();
        assert_eq!((pixel.name.as_str(), pixel.id.as_deref()), ("Pixel 8", Some("id-b")));

        assert!(again.forget(&a));
        assert!(!again.forget(&a));
        assert!(!PairedPhones::load_from(Some(path.clone())).contains(&a));
        // A damaged file gives an empty list instead of an error.
        std::fs::write(&path, "not json").unwrap();
        assert!(PairedPhones::load_from(Some(path)).list().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_identity_is_made_once_and_kept_private() {
        let dir = std::env::temp_dir().join(format!("pw-identity-{}", std::process::id()));
        let path = dir.join("identity.pem");
        let made = Identity::load_or_create(&path).unwrap();
        let loaded = Identity::load_or_create(&path).unwrap();
        assert_eq!(made.fingerprint, loaded.fingerprint);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
