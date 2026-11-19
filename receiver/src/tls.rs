//! TLS 1.3 between the desktop (client) and the phone (server), with certificates that
//! are trusted by fingerprint instead of by a certificate authority (PROTOCOL.md, "Encryption").
//!
//! During the handshake any certificate is accepted, as long as the peer proves it holds
//! the certificate's key (the TLS signature is always checked). Whether that certificate
//! belongs to a paired phone is decided right after the handshake, before a single
//! message is read or sent ([`Link::peer`]).
//!
//! A [`Link`] is one encrypted connection that a reader thread and any number of writer
//! threads can share, like the plain `TcpStream` clones it replaces.

use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context, Result};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls13_signature, CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, SignatureScheme};

use crate::pairing::{Fingerprint, Identity};

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// Accepts any certificate whose TLS 1.3 signature is valid; the caller pins it afterwards.
#[derive(Debug)]
pub(crate) struct AnyCertificate(WebPkiSupportedAlgorithms);

impl AnyCertificate {
    pub(crate) fn new() -> AnyCertificate {
        AnyCertificate(provider().signature_verification_algorithms)
    }

    fn tls12() -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::PeerIncompatible(rustls::PeerIncompatible::Tls13RequiredForQuic))
    }
}

impl ServerCertVerifier for AnyCertificate {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Self::tls12()
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.0)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.supported_schemes()
    }
}

#[cfg(test)]
impl rustls::server::danger::ClientCertVerifier for AnyCertificate {
    fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<rustls::server::danger::ClientCertVerified, rustls::Error> {
        Ok(rustls::server::danger::ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Self::tls12()
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.0)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.supported_schemes()
    }
}

/// The client side: TLS 1.3 only, our certificate for the phone to check.
pub fn client_config(identity: &Identity) -> Result<Arc<ClientConfig>> {
    let config = ClientConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AnyCertificate::new()))
        .with_client_auth_cert(vec![identity.cert.clone()], identity.key.clone_key())
        .context("using this computer's certificate")?;
    Ok(Arc::new(config))
}

/// One encrypted connection, shared by a reader and writers.
pub struct Link {
    conn: Mutex<ClientConnection>,
    /// Writes go here, under the `conn` lock so records leave in order.
    sock: TcpStream,
    peer: Fingerprint,
}

impl Link {
    /// Runs the TLS handshake on a connected socket. The phone's certificate isn't
    /// judged here: check [`Link::peer`] before trusting anything it sends.
    pub fn handshake(mut sock: TcpStream, config: Arc<ClientConfig>) -> Result<(Arc<Link>, LinkReader)> {
        // The name isn't checked (phones have no domain names); it only fills in the SNI.
        let name = ServerName::try_from("phone.pocketwebcam.invalid").expect("a valid DNS name");
        let mut conn = ClientConnection::new(config, name)?;
        while conn.is_handshaking() {
            conn.complete_io(&mut sock).map_err(|e| handshake_error(e, &conn))?;
        }
        let cert = conn
            .peer_certificates()
            .and_then(|c| c.first())
            .ok_or_else(|| anyhow!("the phone sent no certificate"))?;
        let peer = Fingerprint::of(cert);
        let link = Arc::new(Link { conn: Mutex::new(conn), sock: sock.try_clone()?, peer });
        let reader = LinkReader { link: link.clone(), sock, raw: vec![0; 16 * 1024] };
        Ok((link, reader))
    }

    /// The phone's certificate fingerprint.
    pub fn peer(&self) -> Fingerprint {
        self.peer
    }

    fn write_all_plain(&self, data: &[u8]) -> io::Result<()> {
        let mut conn = self.conn.lock().unwrap();
        conn.writer().write_all(data)?;
        while conn.wants_write() {
            conn.write_tls(&mut &self.sock)?;
        }
        Ok(())
    }

    /// Says goodbye (best effort) and closes the socket, which also wakes the reader.
    pub fn shutdown(&self) {
        if let Ok(mut conn) = self.conn.try_lock() {
            conn.send_close_notify();
            let _ = conn.write_tls(&mut &self.sock);
        }
        let _ = self.sock.shutdown(Shutdown::Both);
    }

    pub fn set_read_timeout(&self, timeout: Option<std::time::Duration>) -> io::Result<()> {
        self.sock.set_read_timeout(timeout)
    }
}

/// `&Link` can be written to from any thread; each call sends whole messages in order.
impl Write for &Link {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.write_all_plain(buf)?;
        Ok(buf.len())
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.write_all_plain(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The reading end. Blocks on the socket without holding the lock, so writers aren't held up.
pub struct LinkReader {
    link: Arc<Link>,
    sock: TcpStream,
    raw: Vec<u8>,
}

impl Read for LinkReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        loop {
            {
                let mut conn = self.link.conn.lock().unwrap();
                match conn.reader().read(out) {
                    Ok(n) => return Ok(n), // 0 = the phone closed the connection properly
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                    Err(e) => return Err(e),
                }
            }
            // Nothing decrypted is waiting: read more from the network. The buffer is no
            // bigger than rustls' plaintext limit, so the plaintext always fits.
            let n = self.sock.read(&mut self.raw)?;
            let mut conn = self.link.conn.lock().unwrap();
            if n == 0 {
                // Lets rustls tell a clean close_notify from a cut connection.
                conn.read_tls(&mut io::empty())?;
                conn.process_new_packets().map_err(io::Error::other)?;
                continue;
            }
            let mut data = &self.raw[..n];
            while !data.is_empty() {
                conn.read_tls(&mut data)?;
                conn.process_new_packets().map_err(io::Error::other)?;
            }
            // Key updates and alerts need an answer.
            while conn.wants_write() {
                conn.write_tls(&mut &self.sock)?;
            }
        }
    }
}

fn handshake_error(e: io::Error, conn: &ClientConnection) -> anyhow::Error {
    let _ = conn;
    if e.kind() == io::ErrorKind::UnexpectedEof || e.kind() == io::ErrorKind::ConnectionReset {
        anyhow!(
            "the phone closed the connection during the encrypted handshake. Its Pocket Webcam app may be \
             too old for this computer's: update both"
        )
    } else {
        anyhow!("encrypted handshake with the phone failed: {e}")
    }
}

/// A phone-side TLS server for tests: accepts one connection on a random port.
#[cfg(test)]
pub(crate) mod test_phone {
    use super::*;
    use rustls::{ServerConfig, ServerConnection, StreamOwned};
    use std::net::TcpListener;

    pub struct Accepted {
        pub stream: StreamOwned<ServerConnection, TcpStream>,
        /// The desktop's certificate fingerprint.
        pub desktop: Fingerprint,
    }

    pub fn server_config(identity: &Identity) -> Arc<ServerConfig> {
        Arc::new(
            ServerConfig::builder_with_provider(provider())
                .with_protocol_versions(&[&rustls::version::TLS13])
                .unwrap()
                .with_client_cert_verifier(Arc::new(AnyCertificate::new()))
                .with_single_cert(vec![identity.cert.clone()], identity.key.clone_key())
                .unwrap(),
        )
    }

    /// Listens on 127.0.0.1; `serve` runs on its own thread with the accepted connection.
    pub fn spawn<T: Send + 'static>(
        identity: Identity,
        serve: impl FnOnce(Accepted) -> T + Send + 'static,
    ) -> (std::net::SocketAddr, std::thread::JoinHandle<T>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut conn = ServerConnection::new(server_config(&identity)).unwrap();
            while conn.is_handshaking() {
                conn.complete_io(&mut sock).unwrap();
            }
            let desktop = Fingerprint::of(conn.peer_certificates().unwrap().first().unwrap());
            serve(Accepted { stream: StreamOwned::new(conn, sock), desktop })
        });
        (addr, handle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{read_message, write_message, PING, PONG};
    use std::io::BufReader;

    #[test]
    fn both_sides_see_each_others_fingerprint_and_messages_flow_both_ways() {
        let (desktop, _) = Identity::generate().unwrap();
        let (phone, _) = Identity::generate().unwrap();
        let phone_fp = phone.fingerprint;
        let (addr, server) = test_phone::spawn(phone, |mut a| {
            // A big message, like a 4K keyframe, then a reply to the desktop's ping.
            let big: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
            write_message(&mut a.stream, 0x11, &big).unwrap();
            let msg = read_message(&mut a.stream).unwrap();
            write_message(&mut a.stream, PONG, &msg.payload).unwrap();
            a.stream.flush().unwrap();
            a.desktop
        });

        let sock = TcpStream::connect(addr).unwrap();
        let (link, reader) = Link::handshake(sock, client_config(&desktop).unwrap()).unwrap();
        assert_eq!(link.peer(), phone_fp);
        let mut reader = BufReader::new(reader);
        let big = read_message(&mut reader).unwrap();
        assert_eq!(big.payload.len(), 3_000_000);
        assert!(big.payload.iter().enumerate().all(|(i, b)| *b == (i % 251) as u8));
        // Written from another thread while the reader waits.
        let writer = link.clone();
        std::thread::spawn(move || write_message(&mut &*writer, PING, b"{\"t\":1}").unwrap());
        let pong = read_message(&mut reader).unwrap();
        assert_eq!((pong.kind, pong.payload.as_slice()), (PONG, b"{\"t\":1}".as_slice()));
        assert_eq!(server.join().unwrap(), desktop.fingerprint);
    }

    #[test]
    fn a_plain_tcp_peer_is_refused() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            // A v1 phone answers with a HELLO message instead of a TLS ServerHello.
            let _ = write_message(&mut s, 0x01, b"{\"protocol\":1,\"name\":\"old\"}");
        });
        let (desktop, _) = Identity::generate().unwrap();
        let sock = TcpStream::connect(addr).unwrap();
        assert!(Link::handshake(sock, client_config(&desktop).unwrap()).is_err());
    }
}
