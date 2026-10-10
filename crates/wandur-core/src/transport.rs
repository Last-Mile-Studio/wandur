//! How a session reaches the server. The protocol pieces (telnet, decoding, prompts) never touch
//! a socket; the session reads and writes through a [`Link`] that a [`Transport`] opens. TCP is
//! built in; TLS (rustls with the operating system's certificate verifier) is behind the `tls`
//! feature. A browser build would add a WebSocket transport with the same shape.

use crate::l10n::{S, tf};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use crate::endpoint::Endpoint;

/// An open connection, split so that one thread can block reading while another writes.
pub struct Link {
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
    /// Ends the connection from any thread; a blocked read returns.
    pub closer: Box<dyn Fn() + Send + Sync>,
    /// For people: the address actually connected to.
    pub peer: String,
    /// Whether the link is encrypted.
    pub secure: bool,
}

/// Opens links. Called on the session's reader thread, so it may block (up to `timeout` to connect).
pub trait Transport: Send + Sync {
    fn open(&self, endpoint: &Endpoint, timeout: Duration) -> Result<Link, String>;
}

/// The transport for an endpoint: TLS when it asks for it (and the feature is built), else TCP.
pub fn for_endpoint(endpoint: &Endpoint) -> Arc<dyn Transport> {
    if endpoint.tls {
        #[cfg(feature = "tls")]
        return Arc::new(tls::TlsTransport::platform());
        #[cfg(not(feature = "tls"))]
        return Arc::new(Unsupported("TLS is not built into this client"));
    }
    Arc::new(TcpTransport)
}

/// A transport that always fails, for features left out of the build.
pub struct Unsupported(pub &'static str);

impl Transport for Unsupported {
    fn open(&self, _: &Endpoint, _: Duration) -> Result<Link, String> {
        Err(self.0.to_string())
    }
}

/// Connect TCP to the first address of `endpoint` that answers.
pub fn connect_tcp(endpoint: &Endpoint, timeout: Duration) -> Result<TcpStream, String> {
    let host = &endpoint.host;
    let addresses = (host.as_str(), endpoint.port)
        .to_socket_addrs()
        .map_err(|e| tf(S::CouldNotFindHost, &[&host, &e]))?;
    let mut last = tf(S::NoAddressFoundFor, &[&host]);
    for address in addresses {
        match TcpStream::connect_timeout(&address, timeout) {
            Ok(stream) => {
                let _ = stream.set_nodelay(true);
                return Ok(stream);
            }
            Err(e) => last = tf(S::CouldNotConnectTo, &[&endpoint, &e]),
        }
    }
    Err(last)
}

/// Plain telnet over TCP.
pub struct TcpTransport;

impl Transport for TcpTransport {
    fn open(&self, endpoint: &Endpoint, timeout: Duration) -> Result<Link, String> {
        let stream = connect_tcp(endpoint, timeout)?;
        let peer = stream.peer_addr().map(|a| a.to_string()).unwrap_or_default();
        let clone = |s: &TcpStream| {
            s.try_clone()
                .map_err(|e| crate::l10n::tf(crate::l10n::S::CouldNotUseTheConnection, &[&e]))
        };
        let writer = clone(&stream)?;
        let closer = clone(&stream)?;
        Ok(Link {
            reader: Box::new(stream),
            writer: Box::new(writer),
            closer: Box::new(move || {
                let _ = closer.shutdown(Shutdown::Both);
            }),
            peer,
            secure: false,
        })
    }
}

#[cfg(feature = "tls")]
pub mod tls {
    //! TLS over TCP with rustls (ring provider). The rustls connection is shared by the reading
    //! and the writing side behind a mutex that is never held during a blocking socket read:
    //! the reader reads raw bytes without the lock, then takes it to decrypt.

    use std::io::{self, Read, Write};
    use std::net::{Shutdown, TcpStream};
    use std::sync::{Arc, Mutex, PoisonError};
    use std::time::Duration;

    use rustls::pki_types::{CertificateDer, ServerName};
    use rustls::{ClientConfig, ClientConnection, RootCertStore};

    use super::{Link, Transport, connect_tcp};
    use crate::endpoint::Endpoint;
    use crate::l10n::{S, tf};

    /// Read from the socket, retrying interrupted reads.
    fn read_retrying(stream: &mut TcpStream, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            match stream.read(buf) {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                other => return other,
            }
        }
    }

    pub struct TlsTransport {
        config: Result<Arc<ClientConfig>, String>,
    }

    fn provider() -> Arc<rustls::crypto::CryptoProvider> {
        Arc::new(rustls::crypto::ring::default_provider())
    }

    impl TlsTransport {
        /// Certificates checked by the operating system, as the C# client's `SslStream` does.
        pub fn platform() -> Self {
            use rustls_platform_verifier::BuilderVerifierExt;
            let config = ClientConfig::builder_with_provider(provider())
                .with_safe_default_protocol_versions()
                .map_err(|e| e.to_string())
                .and_then(|b| b.with_platform_verifier().map_err(|e| e.to_string()))
                .map(|b| Arc::new(b.with_no_client_auth()));
            Self { config }
        }

        /// Trust only the given certificates (DER), for tests against a local server.
        pub fn with_roots(roots: &[Vec<u8>]) -> Self {
            let mut store = RootCertStore::empty();
            for der in roots {
                let _ = store.add(CertificateDer::from(der.clone()));
            }
            let config = ClientConfig::builder_with_provider(provider())
                .with_safe_default_protocol_versions()
                .map(|b| Arc::new(b.with_root_certificates(store).with_no_client_auth()))
                .map_err(|e| e.to_string());
            Self { config }
        }
    }

    impl Transport for TlsTransport {
        fn open(&self, endpoint: &Endpoint, timeout: Duration) -> Result<Link, String> {
            let config = self.config.clone()?;
            let name =
                ServerName::try_from(endpoint.host.clone()).map_err(|_| tf(S::TlsBadServerName, &[&endpoint.host]))?;
            let mut connection = ClientConnection::new(config, name).map_err(|e| tf(S::TlsSetupFailed, &[&e]))?;
            let mut stream = connect_tcp(endpoint, timeout)?;
            // Handshake before splitting, bounded by the connect timeout.
            let _ = stream.set_read_timeout(Some(timeout));
            while connection.is_handshaking() {
                connection
                    .complete_io(&mut stream)
                    .map_err(|e| tf(S::TlsHandshakeFailed, &[endpoint, &e]))?;
            }
            let _ = stream.set_read_timeout(None);
            let peer = stream.peer_addr().map(|a| a.to_string()).unwrap_or_default();
            let clone = |s: &TcpStream| {
                s.try_clone()
                    .map_err(|e| crate::l10n::tf(crate::l10n::S::CouldNotUseTheConnection, &[&e]))
            };
            let shared = Arc::new(Mutex::new(connection));
            let writer = TlsWriter {
                connection: Arc::clone(&shared),
                socket: clone(&stream)?,
            };
            let closer = clone(&stream)?;
            let reader = TlsReader {
                connection: shared,
                socket: stream,
                raw: vec![0; 16 * 1024],
                start: 0,
                end: 0,
            };
            Ok(Link {
                reader: Box::new(reader),
                writer: Box::new(writer),
                closer: Box::new(move || {
                    let _ = closer.shutdown(Shutdown::Both);
                }),
                peer,
                secure: true,
            })
        }
    }

    struct TlsReader {
        connection: Arc<Mutex<ClientConnection>>,
        socket: TcpStream,
        /// Raw bytes read from the socket; `start..end` not yet taken by rustls.
        raw: Vec<u8>,
        start: usize,
        end: usize,
    }

    impl Read for TlsReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            loop {
                {
                    let mut conn = self.connection.lock().unwrap_or_else(PoisonError::into_inner);
                    match conn.reader().read(buf) {
                        // Ok(0) is a clean close (close_notify).
                        Ok(n) => return Ok(n),
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                        // Closed without close_notify: report the end of the stream.
                        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(0),
                        Err(e) => return Err(e),
                    }
                    if self.start < self.end {
                        let mut pending = &self.raw[self.start..self.end];
                        let taken = conn.read_tls(&mut pending)?;
                        self.start += taken;
                        conn.process_new_packets()
                            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                        // Alerts or key updates may need sending.
                        while conn.wants_write() {
                            conn.write_tls(&mut self.socket)?;
                        }
                        continue;
                    }
                }
                // Nothing decrypted and nothing pending: block on the socket without the lock.
                let n = read_retrying(&mut self.socket, &mut self.raw)?;
                if n == 0 {
                    return Ok(0);
                }
                self.start = 0;
                self.end = n;
            }
        }
    }

    struct TlsWriter {
        connection: Arc<Mutex<ClientConnection>>,
        socket: TcpStream,
    }

    impl Write for TlsWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.write_all(buf)?;
            Ok(buf.len())
        }

        fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
            let mut conn = self.connection.lock().unwrap_or_else(PoisonError::into_inner);
            conn.writer().write_all(buf)?;
            while conn.wants_write() {
                conn.write_tls(&mut self.socket)?;
            }
            Ok(())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
}
