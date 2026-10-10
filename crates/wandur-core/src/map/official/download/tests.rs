//! Downloads against a local https server (a self-signed certificate the test agent trusts):
//! http refused, redirects capped and https only, the size cap, ETag or Last-Modified and 304.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use super::*;

/// Each request's path and its validator ([`validator`]), in order.
type Seen = Arc<Mutex<Vec<(String, Option<String>)>>>;

struct Server {
    port: u16,
    certificate: Vec<u8>,
    seen: Seen,
}

impl Server {
    fn url(&self, path: &str) -> String {
        format!("https://127.0.0.1:{}{path}", self.port)
    }

    fn agent(&self) -> ureq::Agent {
        agent_trusting(std::slice::from_ref(&self.certificate))
    }
}

/// Serve `route(path, validator)` over TLS, one request per connection, until the test ends.
fn serve(route: impl Fn(&str, Option<&str>) -> Vec<u8> + Send + Sync + 'static) -> Server {
    let cert = rcgen::generate_simple_self_signed(vec!["127.0.0.1".into(), "localhost".into()]).unwrap();
    let certificate = cert.cert.der().to_vec();
    let key = cert.signing_key.serialize_der();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = Arc::new(
        rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![rustls::pki_types::CertificateDer::from(certificate.clone())],
                rustls::pki_types::PrivateKeyDer::Pkcs8(key.into()),
            )
            .unwrap(),
    );
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    // `route` answers with the whole response, head and body.
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(tcp) = stream else { return };
            let Ok(connection) = rustls::ServerConnection::new(Arc::clone(&config)) else {
                continue;
            };
            let mut tls = rustls::StreamOwned::new(connection, tcp);
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                match tls.read(&mut byte) {
                    Ok(1) => head.push(byte[0]),
                    _ => break,
                }
            }
            let head = String::from_utf8_lossy(&head).into_owned();
            let path = head.split_whitespace().nth(1).unwrap_or("/").to_string();
            let etag = validator(&head).unwrap_or_default();
            log.lock()
                .unwrap()
                .push((path.clone(), (!etag.is_empty()).then(|| etag.clone())));
            let _ = tls.write_all(&route(&path, (!etag.is_empty()).then_some(etag.as_str())));
            tls.conn.send_close_notify();
            let _ = tls.flush();
        }
    });
    Server {
        port,
        certificate,
        seen,
    }
}

/// The request's `If-None-Match` value, else `since <If-Modified-Since value>`.
fn validator(head: &str) -> Option<String> {
    let header = |wanted: &str| {
        head.lines().find_map(|l| {
            let (name, value) = l.split_once(':')?;
            name.eq_ignore_ascii_case(wanted).then(|| value.trim().to_string())
        })
    };
    header("if-none-match").or_else(|| header("if-modified-since").map(|d| format!("since {d}")))
}

fn none() -> Validators {
    Validators::default()
}

fn ok(body: &[u8], etag: Option<&str>) -> Vec<u8> {
    let mut out = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    if let Some(etag) = etag {
        out.push_str(&format!("ETag: {etag}\r\n"));
    }
    out.push_str("\r\n");
    let mut out = out.into_bytes();
    out.extend_from_slice(body);
    out
}

fn redirect(to: &str) -> Vec<u8> {
    format!("HTTP/1.1 302 Found\r\nLocation: {to}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").into_bytes()
}

#[test]
fn http_addresses_are_refused_before_connecting() {
    let agent = agent();
    assert_eq!(
        download(&agent, "http://maps.fixture.example/map.xml", &none()),
        Err(DownloadError::NotHttps)
    );
    assert_eq!(
        download(&agent, "not an address", &none()),
        Err(DownloadError::NotHttps)
    );
}

#[test]
fn a_file_comes_with_its_etag_and_a_304_means_unchanged() {
    let server = serve(|_, etag| match etag {
        Some("\"v1\"") => b"HTTP/1.1 304 Not Modified\r\nConnection: close\r\n\r\n".to_vec(),
        _ => ok(b"<map/>", Some("\"v1\"")),
    });
    let agent = server.agent();
    let first = download(&agent, &server.url("/map.xml"), &none()).unwrap();
    assert_eq!(
        first,
        Downloaded::File {
            bytes: b"<map/>".to_vec(),
            etag: Some("\"v1\"".into()),
            last_modified: None,
        }
    );
    let known = Validators {
        etag: Some("\"v1\"".into()),
        last_modified: Some("Tue, 01 Sep 2026 10:00:00 GMT".into()),
    };
    let again = download(&agent, &server.url("/map.xml"), &known).unwrap();
    assert_eq!(again, Downloaded::NotModified);
    let seen = server.seen.lock().unwrap().clone();
    assert_eq!(seen[1].1.as_deref(), Some("\"v1\""), "If-None-Match, the ETag first");
}

#[test]
fn without_an_etag_last_modified_goes_back_as_if_modified_since() {
    const DATE: &str = "Tue, 01 Sep 2026 10:00:00 GMT";
    let server = serve(|_, validator| match validator {
        Some(v) if v == format!("since {DATE}") => b"HTTP/1.1 304 Not Modified\r\nConnection: close\r\n\r\n".to_vec(),
        _ => {
            format!("HTTP/1.1 200 OK\r\nContent-Length: 6\r\nLast-Modified: {DATE}\r\nConnection: close\r\n\r\n<map/>")
                .into_bytes()
        }
    });
    let agent = server.agent();
    let Downloaded::File {
        last_modified, etag, ..
    } = download(&agent, &server.url("/map.xml"), &none()).unwrap()
    else {
        panic!("the file");
    };
    assert_eq!((etag, last_modified.as_deref()), (None, Some(DATE)));
    let known = Validators {
        etag: None,
        last_modified,
    };
    assert_eq!(
        download(&agent, &server.url("/map.xml"), &known),
        Ok(Downloaded::NotModified)
    );
    assert_eq!(
        server.seen.lock().unwrap()[1].1,
        Some(format!("since {DATE}")),
        "If-Modified-Since was sent"
    );
}

#[test]
fn redirects_are_followed_up_to_three_and_only_to_https() {
    let port = Arc::new(Mutex::new(0u16));
    let p = Arc::clone(&port);
    let server = serve(move |path, _| {
        let port = *p.lock().unwrap();
        match path {
            "/three" => redirect("/two"),
            "/two" => redirect(&format!("https://localhost:{port}/one")),
            "/one" => redirect("/file"),
            "/file" => ok(b"<map/>", None),
            "/four" => redirect("/three"),
            "/plain" => redirect("http://127.0.0.1:9/file"),
            "/loop" => redirect("/loop"),
            _ => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
        }
    });
    *port.lock().unwrap() = server.port;
    let agent = server.agent();
    assert!(matches!(
        download(&agent, &server.url("/three"), &none()),
        Ok(Downloaded::File { .. })
    ));
    assert_eq!(
        download(&agent, &server.url("/four"), &none()),
        Err(DownloadError::TooManyRedirects)
    );
    assert_eq!(
        download(&agent, &server.url("/loop"), &none()),
        Err(DownloadError::TooManyRedirects)
    );
    assert_eq!(
        download(&agent, &server.url("/plain"), &none()),
        Err(DownloadError::RedirectNotHttps)
    );
    assert_eq!(
        download(&agent, &server.url("/missing"), &none()),
        Err(DownloadError::Status(404))
    );
}

#[test]
fn the_size_cap_holds_while_streaming() {
    let server = serve(|path, _| match path {
        // No length: the body runs until the connection closes.
        "/stream" => {
            let mut out = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_vec();
            out.extend(std::iter::repeat_n(b'x', 5000));
            out
        }
        // A length past the real cap: refused from the header.
        "/declared" => format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            MAX_BYTES + 1
        )
        .into_bytes(),
        _ => ok(&[b'y'; 1000], None),
    });
    let agent = server.agent();
    assert_eq!(
        fetch(&agent, &server.url("/stream"), &none(), 4096),
        Err(DownloadError::TooLarge)
    );
    assert!(matches!(
        fetch(&agent, &server.url("/small"), &none(), 4096),
        Ok(Downloaded::File { bytes, .. }) if bytes.len() == 1000
    ));
    assert_eq!(
        download(&agent, &server.url("/declared"), &none()),
        Err(DownloadError::TooLarge)
    );
}

#[test]
fn an_untrusted_certificate_fails() {
    let server = serve(|_, _| ok(b"<map/>", None));
    let other = rcgen::generate_simple_self_signed(vec!["127.0.0.1".into()]).unwrap();
    let agent = agent_trusting(&[other.cert.der().to_vec()]);
    assert!(matches!(
        download(&agent, &server.url("/map.xml"), &none()),
        Err(DownloadError::Network(_))
    ));
}
