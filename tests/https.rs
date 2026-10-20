//! HTTPS: a real server with a self-signed certificate, spoken to by a TLS client that trusts it.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use live_serve::{Config, rustls, serve, tls};
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, ServerName};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsConnector;

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("live-serve-https-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn site(name: &str) -> PathBuf {
    let dir = temp(name).join("site");
    std::fs::create_dir_all(dir.join("docs")).unwrap();
    std::fs::write(dir.join("index.html"), "<html><body><h1>secure</h1></body></html>").unwrap();
    std::fs::write(dir.join("app.js"), "console.log(1)").unwrap();
    dir
}

/// Serves `root` over HTTPS with a self-signed certificate made in a folder of its own; returns the
/// port and the certificate file. The settings are read back from the files, as `--cert`/`--key` would.
async fn start(name: &str, root: PathBuf) -> (u16, PathBuf) {
    let certs = temp(&format!("{name}-cert"));
    let (_, cert) = tls::self_signed(&certs, &tls::certificate_names("127.0.0.1", None)).unwrap();
    let mut cfg = Config::new(root);
    cfg.tls = Some(tls::from_pem_files(&cert, &cert.with_file_name("key.pem")).unwrap());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { serve(listener, cfg).await.unwrap() });
    tokio::time::sleep(Duration::from_millis(200)).await;
    (port, cert)
}

/// A TLS client that trusts `cert` (and nothing else when `cert` is None).
fn connector(cert: Option<&Path>) -> TlsConnector {
    let mut roots = rustls::RootCertStore::empty();
    if let Some(cert) = cert {
        for c in CertificateDer::pem_file_iter(cert).unwrap() {
            roots.add(c.unwrap()).unwrap();
        }
    }
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    TlsConnector::from(Arc::new(config))
}

async fn connect(port: u16, cert: &Path, name: &str) -> tokio_rustls::client::TlsStream<TcpStream> {
    let tcp = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let name = ServerName::try_from(name.to_string()).unwrap();
    connector(Some(cert)).connect(name, tcp).await.expect("handshake with a trusted certificate")
}

/// Reads until the server closes; TLS servers may close without a close_notify, which is fine here.
async fn read_all(stream: &mut (impl AsyncRead + Unpin)) -> String {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match tokio::time::timeout(Duration::from_secs(5), stream.read(&mut chunk)).await {
            Ok(Ok(0)) | Ok(Err(_)) | Err(_) => break,
            Ok(Ok(n)) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    String::from_utf8_lossy(&buf).to_string()
}

async fn get(port: u16, cert: &Path, name: &str, path: &str) -> String {
    let mut s = connect(port, cert, name).await;
    s.write_all(format!("GET {path} HTTP/1.1\r\nHost: {name}:{port}\r\nConnection: close\r\n\r\n").as_bytes())
        .await
        .unwrap();
    read_all(&mut s).await
}

#[tokio::test]
async fn serves_pages_over_https_by_name_and_by_address() {
    let (port, cert) = start("pages", site("pages")).await;
    for name in ["localhost", "127.0.0.1"] {
        let response = get(port, &cert, name, "/").await;
        assert!(response.starts_with("HTTP/1.1 200"), "{name}: {response}");
        assert!(response.contains("<h1>secure</h1>"));
        assert!(response.contains("/__live/ws"), "the reload script, which picks wss:// itself");
    }
    let script = get(port, &cert, "localhost", "/app.js").await;
    assert!(script.ends_with("console.log(1)"), "{script}");
}

#[tokio::test]
async fn a_client_that_does_not_trust_the_certificate_is_refused() {
    let (port, _) = start("untrusted", site("untrusted")).await;
    let tcp = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let result = connector(None).connect(ServerName::try_from("localhost").unwrap(), tcp).await;
    assert!(result.is_err(), "a real certificate check, not plain text");
}

#[tokio::test]
async fn the_certificate_is_for_the_names_it_was_made_for_only() {
    let (port, cert) = start("names", site("names")).await;
    let tcp = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let other = ServerName::try_from("example.com").unwrap();
    assert!(connector(Some(&cert)).connect(other, tcp).await.is_err());
}

#[tokio::test]
async fn plain_http_to_the_https_port_is_sent_to_https() {
    let (port, _) = start("redirect", site("redirect")).await;
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    s.write_all(format!("GET /docs/?q=1 HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n").as_bytes()).await.unwrap();
    let response = read_all(&mut s).await;
    // Temporary, and not kept: a later server without HTTPS on this port must not be sent to https://.
    assert!(response.starts_with("HTTP/1.1 307"), "{response}");
    assert!(response.contains(&format!("Location: https://localhost:{port}/docs/?q=1\r\n")), "{response}");
    assert!(response.contains("Cache-Control: no-store\r\n"), "{response}");

    // A Host that is not a plain host name is not echoed into the redirect.
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    s.write_all(b"GET / HTTP/1.1\r\nHost: evil.example/path\r\n\r\n").await.unwrap();
    assert!(read_all(&mut s).await.starts_with("HTTP/1.1 400"));
}

#[tokio::test]
async fn a_stalled_handshake_holds_up_nobody() {
    let (port, cert) = start("stall", site("stall")).await;
    // Connects, starts a TLS record and goes quiet.
    let mut stalled = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    stalled.write_all(&[0x16, 0x03, 0x01]).await.unwrap();
    let response = tokio::time::timeout(Duration::from_secs(3), get(port, &cert, "localhost", "/")).await;
    assert!(response.expect("answered while another handshake hangs").starts_with("HTTP/1.1 200"));
}

/// Writes one masked client text frame (clients must mask).
async fn ws_send(s: &mut (impl AsyncWrite + Unpin), text: &str) {
    let mask = [1u8, 2, 3, 4];
    let mut frame = vec![0x81, 0x80 | text.len() as u8];
    frame.extend_from_slice(&mask);
    frame.extend(text.bytes().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    s.write_all(&frame).await.unwrap();
}

/// Reads one short text frame from the server.
async fn ws_read(s: &mut (impl AsyncRead + Unpin)) -> String {
    let mut h = [0u8; 2];
    s.read_exact(&mut h).await.unwrap();
    let mut payload = vec![0u8; (h[1] & 0x7f) as usize];
    s.read_exact(&mut payload).await.unwrap();
    String::from_utf8(payload).unwrap()
}

#[tokio::test]
async fn live_reload_works_over_wss() {
    let root = site("wss");
    let (port, cert) = start("wss", root.clone()).await;
    let mut s = connect(port, &cert, "localhost").await;
    let upgrade = format!(
        "GET /__live/ws HTTP/1.1\r\nHost: localhost:{port}\r\nOrigin: https://localhost:{port}\r\nUpgrade: websocket\r\n\
Connection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
    );
    s.write_all(upgrade.as_bytes()).await.unwrap();
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        s.read_exact(&mut byte).await.unwrap();
        head.push(byte[0]);
    }
    assert!(String::from_utf8_lossy(&head).starts_with("HTTP/1.1 101"), "{}", String::from_utf8_lossy(&head));
    let token = tokio::time::timeout(Duration::from_secs(5), ws_read(&mut s)).await.unwrap();
    assert!(token.starts_with("id:"), "{token}");
    ws_send(&mut s, "hello").await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    std::fs::write(root.join("app.js"), "console.log(2)").unwrap();
    let message = loop {
        let m = tokio::time::timeout(Duration::from_secs(5), ws_read(&mut s)).await.expect("a change within 5 s");
        if !m.starts_with("id:") {
            break m;
        }
    };
    assert_eq!(message, "reload");
}
