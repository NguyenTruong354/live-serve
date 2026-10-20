//! HTTPS: a certificate from PEM files (`--cert`/`--key`, as mkcert writes them), or a self-signed one
//! made and kept for `--https`; and a listener that hands axum the connections that complete a handshake.

use std::io;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::{self, ServerConfig};
use tokio_rustls::server::TlsStream;

/// How long a self-signed certificate is valid. Short: browsers and macOS distrust long-lived ones, and
/// a new one is made a day before it runs out.
const SELF_SIGNED_DAYS: u64 = 30;

/// A client that connects but never finishes its handshake is dropped after this long.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

fn invalid(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

/// Server settings from a certificate chain and its private key in PEM files.
pub fn from_pem_files(cert: &Path, key: &Path) -> io::Result<Arc<ServerConfig>> {
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(cert)
        .and_then(|certs| certs.collect())
        .map_err(|e| invalid(format!("cannot read the certificate {}: {e}", cert.display())))?;
    if certs.is_empty() {
        return Err(invalid(format!("no certificate in {}", cert.display())));
    }
    let key = PrivateKeyDer::from_pem_file(key).map_err(|e| {
        invalid(format!("cannot read the key {}: {e} (it must be an unencrypted PEM key, as mkcert writes)", key.display()))
    })?;
    server_config(certs, key)
}

fn server_config(certs: Vec<CertificateDer<'static>>, key: PrivateKeyDer<'static>) -> io::Result<Arc<ServerConfig>> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(io::Error::other)?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| invalid(format!("the certificate and the key do not go together: {e}")))?;
    // HTTP/1.1 only: the reload WebSocket upgrades over it.
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

/// The names a self-signed certificate is for: `localhost` and loopback, the address live-serve was asked
/// to listen on when it is a particular one, and `lan` (this machine's network address) when it listens
/// on all of them, so a phone that opens it by IP sees a matching name.
pub fn certificate_names(host: &str, lan: Option<IpAddr>) -> Vec<String> {
    let mut names: Vec<String> = ["localhost", "127.0.0.1", "::1"].map(String::from).into();
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let everywhere = host.parse::<IpAddr>().is_ok_and(|ip| ip.is_unspecified());
    let extra = if everywhere { lan.map(|ip| ip.to_string()) } else { Some(host.to_ascii_lowercase()) };
    if let Some(name) = extra.filter(|n| !n.is_empty() && !names.contains(n)) {
        names.push(name);
    }
    names
}

/// Where `--https` keeps its certificate: `LIVE_SERVE_CACHE_DIR` if set, else the system's cache folder
/// (`%LOCALAPPDATA%`, `~/Library/Caches`, `$XDG_CACHE_HOME` or `~/.cache`), else the temporary folder.
pub fn cache_dir() -> PathBuf {
    let var = |name| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(dir) = var("LIVE_SERVE_CACHE_DIR") {
        return dir;
    }
    #[cfg(windows)]
    let base = var("LOCALAPPDATA");
    #[cfg(target_os = "macos")]
    let base = var("HOME").map(|home| home.join("Library").join("Caches"));
    #[cfg(all(unix, not(target_os = "macos")))]
    let base = var("XDG_CACHE_HOME").or_else(|| var("HOME").map(|home| home.join(".cache")));
    base.unwrap_or_else(std::env::temp_dir).join("live-serve")
}

/// A self-signed certificate for `names`, kept in `dir` (`cert.pem`, `key.pem`) and used again while it
/// was made for the same names and has more than a day left. Returns the server settings and the
/// certificate's file, which a browser or phone can be told to trust.
pub fn self_signed(dir: &Path, names: &[String]) -> io::Result<(Arc<ServerConfig>, PathBuf)> {
    let (cert, key, made_for) = (dir.join("cert.pem"), dir.join("key.pem"), dir.join("names.txt"));
    let wanted = names.join("\n");
    let renew_after = Duration::from_secs((SELF_SIGNED_DAYS - 1) * 24 * 60 * 60);
    let reusable = std::fs::read_to_string(&made_for).is_ok_and(|n| n == wanted)
        && std::fs::metadata(&cert)
            .and_then(|m| m.modified())
            .is_ok_and(|made| made.elapsed().is_ok_and(|age| age < renew_after));
    if reusable && let Ok(config) = from_pem_files(&cert, &key) {
        return Ok((config, cert));
    }
    let (cert_pem, key_pem) = generate(names, SystemTime::now())?;
    std::fs::create_dir_all(dir)?;
    write_private(&key, key_pem.as_bytes())?;
    std::fs::write(&cert, cert_pem)?;
    std::fs::write(&made_for, wanted)?;
    Ok((from_pem_files(&cert, &key)?, cert))
}

/// A new self-signed certificate and its key (ECDSA P-256), both PEM, valid from a day before `now` (for
/// clocks a little behind) for SELF_SIGNED_DAYS. Names that parse as IP addresses become IP entries.
fn generate(names: &[String], now: SystemTime) -> io::Result<(String, String)> {
    let mut params = rcgen::CertificateParams::new(names.to_vec()).map_err(io::Error::other)?;
    params.distinguished_name = rcgen::DistinguishedName::new();
    params.distinguished_name.push(rcgen::DnType::CommonName, "live-serve (local development)");
    let today = now.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs() / 86_400);
    let date = |days| {
        let (year, month, day) = ymd(days);
        rcgen::date_time_ymd(year, month, day)
    };
    params.not_before = date(today.saturating_sub(1));
    params.not_after = date(today + SELF_SIGNED_DAYS);
    params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];
    params.key_usages = vec![rcgen::KeyUsagePurpose::DigitalSignature];
    let key = rcgen::KeyPair::generate().map_err(io::Error::other)?;
    let cert = params.self_signed(&key).map_err(io::Error::other)?;
    Ok((cert.pem(), key.serialize_pem()))
}

/// Days since 1970-01-01 → (year, month, day).
fn ymd(days: u64) -> (i32, u8, u8) {
    let (year, month, day) = crate::listing::civil_from_days(days);
    (year as i32, month as u8, day as u8)
}

/// Writes a private key readable only by its owner where the system has such permissions.
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    std::io::Write::write_all(&mut options.open(path)?, bytes)
}

/// Accepts TCP connections and hands axum those that complete a TLS handshake. Handshakes run on tasks
/// of their own, so a client that stalls in one holds up nobody else. A plain `http://` request is
/// answered with a redirect to the same address over `https://`, the usual slip after switching.
pub struct TlsListener {
    connections: mpsc::Receiver<(TlsStream<TcpStream>, SocketAddr)>,
    local: SocketAddr,
}

impl TlsListener {
    pub fn new(tcp: TcpListener, config: Arc<ServerConfig>) -> io::Result<TlsListener> {
        let local = tcp.local_addr()?;
        let acceptor = TlsAcceptor::from(config);
        let (tx, connections) = mpsc::channel(64);
        tokio::spawn(async move {
            while !tx.is_closed() {
                let Ok((stream, addr)) = tcp.accept().await else {
                    // Out of file handles and the like: wait a moment rather than spin.
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                };
                let (acceptor, tx) = (acceptor.clone(), tx.clone());
                tokio::spawn(async move {
                    let handshake = async {
                        if starts_like_plain_http(&stream).await {
                            redirect_to_https(stream).await;
                            return None;
                        }
                        acceptor.accept(stream).await.ok()
                    };
                    if let Ok(Some(tls)) = tokio::time::timeout(HANDSHAKE_TIMEOUT, handshake).await {
                        let _ = tx.send((tls, addr)).await;
                    }
                });
            }
        });
        Ok(TlsListener { connections, local })
    }
}

impl axum::serve::Listener for TlsListener {
    type Io = TlsStream<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        match self.connections.recv().await {
            Some(connection) => connection,
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        Ok(self.local)
    }
}

/// Whether the client opened with a letter (`GET …`) rather than a TLS handshake (byte 0x16).
async fn starts_like_plain_http(stream: &TcpStream) -> bool {
    let mut first = [0u8; 1];
    matches!(stream.peek(&mut first).await, Ok(1) if first[0].is_ascii_alphabetic())
}

/// Answers a plain HTTP request with a redirect to the same host and path over HTTPS.
async fn redirect_to_https(mut stream: TcpStream) {
    let mut head = Vec::with_capacity(1024);
    let mut buf = [0u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") && head.len() < 8192 {
        match stream.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => head.extend_from_slice(&buf[..n]),
        }
    }
    let head = String::from_utf8_lossy(&head);
    let target = head.split_whitespace().nth(1).filter(|t| t.starts_with('/')).unwrap_or("/");
    let host = head
        .lines()
        .find_map(|line| line.split_once(':').filter(|(name, _)| name.trim().eq_ignore_ascii_case("host")))
        .map(|(_, value)| value.trim())
        .filter(|h| !h.is_empty() && h.chars().all(|c| c.is_ascii_alphanumeric() || "-.:[]".contains(c)));
    let response = match host {
        Some(host) => format!(
            "HTTP/1.1 301 Moved Permanently\r\nLocation: https://{host}{target}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        ),
        None => "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
    };
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("live-serve-tls-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn names_cover_loopback_and_the_address_asked_for() {
        let lan: IpAddr = "192.168.1.20".parse().unwrap();
        assert_eq!(certificate_names("127.0.0.1", Some(lan)), ["localhost", "127.0.0.1", "::1"]);
        assert_eq!(certificate_names("0.0.0.0", Some(lan)), ["localhost", "127.0.0.1", "::1", "192.168.1.20"]);
        assert_eq!(certificate_names("::", Some(lan)), ["localhost", "127.0.0.1", "::1", "192.168.1.20"]);
        assert_eq!(certificate_names("0.0.0.0", None), ["localhost", "127.0.0.1", "::1"]);
        assert_eq!(certificate_names("MyBox.lan", Some(lan)), ["localhost", "127.0.0.1", "::1", "mybox.lan"]);
        assert_eq!(certificate_names("[fd00::1]", None), ["localhost", "127.0.0.1", "::1", "fd00::1"]);
    }

    #[test]
    fn dates_are_calendar_days() {
        assert_eq!(ymd(0), (1970, 1, 1));
        assert_eq!(ymd(59), (1970, 3, 1));
        assert_eq!(ymd(20_746), (2026, 10, 20));
    }

    #[test]
    fn a_self_signed_certificate_is_made_once_and_kept() {
        let dir = temp("keep");
        let names: Vec<String> = certificate_names("127.0.0.1", None);
        let (_, cert) = self_signed(&dir, &names).unwrap();
        let first = std::fs::read(&cert).unwrap();
        let (_, again) = self_signed(&dir, &names).unwrap();
        assert_eq!(std::fs::read(again).unwrap(), first, "kept, not made again");
        // For other names (the network address changed), a new one.
        let more = certificate_names("0.0.0.0", Some("10.0.0.7".parse().unwrap()));
        let (_, other) = self_signed(&dir, &more).unwrap();
        assert_ne!(std::fs::read(other).unwrap(), first);
    }

    #[test]
    fn an_old_or_broken_certificate_is_made_again() {
        let dir = temp("renew");
        let names = certificate_names("127.0.0.1", None);
        let (_, cert) = self_signed(&dir, &names).unwrap();
        let first = std::fs::read(&cert).unwrap();
        let old = SystemTime::now() - Duration::from_secs(SELF_SIGNED_DAYS * 24 * 60 * 60);
        std::fs::File::options().write(true).open(&cert).unwrap().set_modified(old).unwrap();
        let (_, renewed) = self_signed(&dir, &names).unwrap();
        let second = std::fs::read(&renewed).unwrap();
        assert_ne!(second, first, "a month old: made again");
        std::fs::write(dir.join("key.pem"), "not a key").unwrap();
        let (_, repaired) = self_signed(&dir, &names).unwrap();
        assert_ne!(std::fs::read(repaired).unwrap(), second, "unreadable: made again");
    }

    #[test]
    fn reads_pem_files_and_names_what_is_wrong() {
        let dir = temp("pem");
        std::fs::create_dir_all(&dir).unwrap();
        let (cert_pem, key_pem) = generate(&["localhost".into()], SystemTime::now()).unwrap();
        let (cert, key) = (dir.join("c.pem"), dir.join("k.pem"));
        std::fs::write(&cert, &cert_pem).unwrap();
        std::fs::write(&key, &key_pem).unwrap();
        assert!(from_pem_files(&cert, &key).is_ok());

        let missing = from_pem_files(&dir.join("nope.pem"), &key).unwrap_err().to_string();
        assert!(missing.contains("nope.pem"), "{missing}");
        std::fs::write(dir.join("empty.pem"), "").unwrap();
        let empty = from_pem_files(&dir.join("empty.pem"), &key).unwrap_err().to_string();
        assert!(empty.contains("no certificate"), "{empty}");
        let swapped = from_pem_files(&key, &cert).unwrap_err().to_string();
        assert!(swapped.contains("certificate"), "{swapped}");
        let (_, other_key) = generate(&["localhost".into()], SystemTime::now()).unwrap();
        std::fs::write(dir.join("other.pem"), other_key).unwrap();
        let mismatch = from_pem_files(&cert, &dir.join("other.pem")).unwrap_err().to_string();
        assert!(mismatch.contains("do not go together"), "{mismatch}");
    }

    #[cfg(unix)]
    #[test]
    fn the_key_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp("mode");
        self_signed(&dir, &certificate_names("127.0.0.1", None)).unwrap();
        let mode = std::fs::metadata(dir.join("key.pem")).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "{mode:o}");
    }
}
