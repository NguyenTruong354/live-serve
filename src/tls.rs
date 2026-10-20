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

/// How long a self-signed certificate is valid. Short: browsers and macOS distrust long-lived ones.
const SELF_SIGNED_DAYS: u64 = 30;

/// A self-signed certificate is made again at start once it has this few days left, so a server left
/// running (an editor open for days) does not outlive it.
const RENEW_DAYS_LEFT: u64 = 7;

/// The names every self-signed certificate is for.
const LOOPBACK: [&str; 3] = ["localhost", "127.0.0.1", "::1"];

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
    let mut names: Vec<String> = LOOPBACK.map(String::from).into();
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

/// A self-signed certificate for `names`, kept in a folder of `cache` named after them (`localhost`,
/// `192.168.1.20`), so switching `--host` back and forth keeps each one and the browser warns once for
/// each. Used again while it has more than RENEW_DAYS_LEFT days left. Returns the server settings and the
/// certificate's file, which a browser or phone can be told to trust.
pub fn self_signed(cache: &Path, names: &[String]) -> io::Result<(Arc<ServerConfig>, PathBuf)> {
    self_signed_on(cache, names, day_number(SystemTime::now()))
}

/// `self_signed` as on day `today` (days since 1970-01-01).
fn self_signed_on(cache: &Path, names: &[String], today: u64) -> io::Result<(Arc<ServerConfig>, PathBuf)> {
    let dir = cache.join(folder_for(names));
    let (cert, key, about) = (dir.join("cert.pem"), dir.join("key.pem"), dir.join("about.txt"));
    if is_current(&about, names, today) && let Ok(config) = from_pem_files(&cert, &key) {
        return Ok((config, cert));
    }
    let (cert_pem, key_pem) = generate(names, today)?;
    std::fs::create_dir_all(&dir)?;
    // Each file is written beside its place and renamed into it, so nobody reads half of one. Two
    // live-serves starting at once may still leave one's key with the other's certificate: then this one
    // serves what it made, and the next start finds the pair broken and makes one again.
    replace(&key, key_pem.as_bytes(), true)?;
    replace(&cert, cert_pem.as_bytes(), false)?;
    replace(&about, about_text(names, today + SELF_SIGNED_DAYS).as_bytes(), false)?;
    let config = from_pem_files(&cert, &key).or_else(|_| from_pem(&cert_pem, &key_pem))?;
    Ok((config, cert))
}

/// The cache folder for a certificate for `names`: the names besides loopback ("192.168.1.20",
/// "mybox.lan", "fd00--1"), or "localhost" when there are none.
fn folder_for(names: &[String]) -> String {
    let extra: Vec<&str> = names.iter().map(String::as_str).filter(|n| !LOOPBACK.contains(n)).collect();
    let folder: String = extra
        .join("+")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || "-.+".contains(c) { c } else { '-' })
        .collect();
    // Never "", "." or "..", which would be the cache folder or above it.
    match folder.trim_start_matches('.') {
        "" if extra.is_empty() => "localhost".to_string(),
        "" => "-".to_string(),
        rest => rest.to_string(),
    }
}

/// What `about.txt` says: the day the certificate runs out (days since 1970-01-01), then its names.
fn about_text(names: &[String], until: u64) -> String {
    format!("{until}\n{}", names.join("\n"))
}

/// Whether the certificate `about` tells of is for `names` and good to use on `today`: valid already (the
/// clock may have been set back) and with more than RENEW_DAYS_LEFT days to go.
fn is_current(about: &Path, names: &[String], today: u64) -> bool {
    let Ok(text) = std::fs::read_to_string(about) else { return false };
    let Some((until, made_for)) = text.split_once('\n') else { return false };
    let Ok(until) = until.parse::<u64>() else { return false };
    // Valid from the day before it was made, until the start of day `until`.
    let valid_from = until.saturating_sub(SELF_SIGNED_DAYS + 1);
    made_for == names.join("\n") && valid_from <= today && today + RENEW_DAYS_LEFT < until
}

/// Server settings from a certificate and key held as PEM text.
fn from_pem(cert: &str, key: &str) -> io::Result<Arc<ServerConfig>> {
    let certs = CertificateDer::pem_slice_iter(cert.as_bytes()).collect::<Result<Vec<_>, _>>().map_err(io::Error::other)?;
    server_config(certs, PrivateKeyDer::from_pem_slice(key.as_bytes()).map_err(io::Error::other)?)
}

/// Days since 1970-01-01 at `time`.
fn day_number(time: SystemTime) -> u64 {
    time.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs() / 86_400)
}

/// A new self-signed certificate and its key (ECDSA P-256), both PEM, valid from the day before `today`
/// (for clocks a little behind) until SELF_SIGNED_DAYS after it. Names that parse as IP addresses become
/// IP entries.
fn generate(names: &[String], today: u64) -> io::Result<(String, String)> {
    let mut params = rcgen::CertificateParams::new(names.to_vec()).map_err(io::Error::other)?;
    params.distinguished_name = rcgen::DistinguishedName::new();
    params.distinguished_name.push(rcgen::DnType::CommonName, "live-serve (local development)");
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

/// Writes `bytes` to a new file beside `path` and renames it into place. `private`: readable only by its
/// owner, where the system has such permissions; the file is always a new one, so a key that was once
/// readable by others does not stay so.
fn replace(path: &Path, bytes: &[u8], private: bool) -> io::Result<()> {
    static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut temp = path.as_os_str().to_owned();
    let n = WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    temp.push(format!(".{}-{n}.tmp", std::process::id()));
    let temp = PathBuf::from(temp);
    let _ = std::fs::remove_file(&temp);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    if private {
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    }
    #[cfg(not(unix))]
    let _ = private;
    let written = options.open(&temp).and_then(|mut file| std::io::Write::write_all(&mut file, bytes));
    let renamed = written.and_then(|()| std::fs::rename(&temp, path));
    if renamed.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    renamed
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
    // Temporary and not to be stored: browsers keep a 301 for good, and would still send http://localhost:5500
    // to https:// after live-serve (or another server) runs on that port without HTTPS.
    let response = match host {
        Some(host) => format!(
            "HTTP/1.1 307 Temporary Redirect\r\nLocation: https://{host}{target}\r\nCache-Control: no-store\r\n\
             Content-Length: 0\r\nConnection: close\r\n\r\n"
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

    /// The files in `dir`, sorted.
    fn files(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> =
            std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }

    #[test]
    fn each_set_of_names_has_a_folder_of_its_own() {
        let names = |extra: &[&str]| -> Vec<String> { LOOPBACK.iter().chain(extra).map(|n| n.to_string()).collect() };
        assert_eq!(folder_for(&names(&[])), "localhost");
        assert_eq!(folder_for(&names(&["192.168.1.20"])), "192.168.1.20");
        assert_eq!(folder_for(&names(&["mybox.lan"])), "mybox.lan");
        assert_eq!(folder_for(&names(&["fd00::1"])), "fd00--1");
        assert_eq!(folder_for(&names(&["a", "b/c"])), "a+b-c");
        // Never the cache folder itself or one above it.
        assert_eq!(folder_for(&names(&[".."])), "-");
        assert_eq!(folder_for(&names(&["..x"])), "x");
    }

    #[test]
    fn a_self_signed_certificate_is_made_once_and_kept() {
        let dir = temp("keep");
        let names: Vec<String> = certificate_names("127.0.0.1", None);
        let (_, cert) = self_signed(&dir, &names).unwrap();
        assert_eq!(cert, dir.join("localhost").join("cert.pem"));
        let first = std::fs::read(&cert).unwrap();
        let (_, again) = self_signed(&dir, &names).unwrap();
        assert_eq!(std::fs::read(again).unwrap(), first, "kept, not made again");
        assert_eq!(files(&cert.parent().unwrap()), ["about.txt", "cert.pem", "key.pem"], "no temporary files left");

        // Other names (--host 0.0.0.0) get one of their own, and the first is still there afterwards:
        // switching back and forth does not make the browser warn again each time.
        let more = certificate_names("0.0.0.0", Some("10.0.0.7".parse().unwrap()));
        let (_, other) = self_signed(&dir, &more).unwrap();
        assert_eq!(other, dir.join("10.0.0.7").join("cert.pem"));
        assert_ne!(std::fs::read(&other).unwrap(), first);
        let (_, back) = self_signed(&dir, &names).unwrap();
        assert_eq!(std::fs::read(back).unwrap(), first);
    }

    #[test]
    fn a_certificate_is_renewed_a_week_before_it_runs_out() {
        let dir = temp("renew");
        let names = certificate_names("127.0.0.1", None);
        let today = day_number(SystemTime::now());
        let made = |day| std::fs::read(self_signed_on(&dir, &names, day).unwrap().1).unwrap();
        let first = made(today);
        // Valid until the start of day today + 30: kept while more than 7 days are left.
        assert_eq!(made(today + SELF_SIGNED_DAYS - RENEW_DAYS_LEFT - 1), first, "8 days left: kept");
        let second = made(today + SELF_SIGNED_DAYS - RENEW_DAYS_LEFT);
        assert_ne!(second, first, "7 days left: made again");
        let made_on = today + SELF_SIGNED_DAYS - RENEW_DAYS_LEFT;
        // A clock set back: kept while the certificate is already valid (from the day before it was made).
        assert_eq!(made(made_on - 1), second, "valid from the day before");
        assert_ne!(made(made_on - 2), second, "not valid yet: made again");
    }

    #[test]
    fn a_broken_certificate_is_made_again() {
        let dir = temp("broken");
        let names = certificate_names("127.0.0.1", None);
        let (_, cert) = self_signed(&dir, &names).unwrap();
        let first = std::fs::read(&cert).unwrap();
        std::fs::write(cert.with_file_name("key.pem"), "not a key").unwrap();
        let (_, repaired) = self_signed(&dir, &names).unwrap();
        let second = std::fs::read(&repaired).unwrap();
        assert_ne!(second, first, "unreadable: made again");
        std::fs::write(cert.with_file_name("about.txt"), "not a date").unwrap();
        assert_ne!(std::fs::read(self_signed(&dir, &names).unwrap().1).unwrap(), second, "unknown age: made again");
    }

    #[test]
    fn live_serves_starting_together_all_start() {
        let dir = temp("together");
        let names = certificate_names("127.0.0.1", None);
        let starts: Vec<_> = (0..8)
            .map(|_| {
                let (dir, names) = (dir.clone(), names.clone());
                std::thread::spawn(move || self_signed(&dir, &names).map(|_| ()))
            })
            .collect();
        for start in starts {
            start.join().unwrap().unwrap();
        }
        // Whatever pair was left on disk, the next start has a working one.
        let (_, cert) = self_signed(&dir, &names).unwrap();
        assert!(from_pem_files(&cert, &cert.with_file_name("key.pem")).is_ok());
        assert_eq!(files(&cert.parent().unwrap()), ["about.txt", "cert.pem", "key.pem"]);
    }

    #[test]
    fn reads_pem_files_and_names_what_is_wrong() {
        let dir = temp("pem");
        std::fs::create_dir_all(&dir).unwrap();
        let (cert_pem, key_pem) = generate(&["localhost".into()], day_number(SystemTime::now())).unwrap();
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
        let (_, other_key) = generate(&["localhost".into()], day_number(SystemTime::now())).unwrap();
        std::fs::write(dir.join("other.pem"), other_key).unwrap();
        let mismatch = from_pem_files(&cert, &dir.join("other.pem")).unwrap_err().to_string();
        assert!(mismatch.contains("do not go together"), "{mismatch}");
    }

    #[cfg(unix)]
    #[test]
    fn the_key_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp("mode");
        let names = certificate_names("127.0.0.1", None);
        let key = self_signed(&dir, &names).unwrap().1.with_file_name("key.pem");
        let mode = |key: &Path| std::fs::metadata(key).unwrap().permissions().mode();
        assert_eq!(mode(&key) & 0o077, 0, "{:o}", mode(&key));
        // A key someone made readable to all is not written over in place, which would keep that.
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).unwrap();
        std::fs::write(key.with_file_name("about.txt"), "").unwrap();
        self_signed(&dir, &names).unwrap();
        assert_eq!(mode(&key) & 0o077, 0, "{:o}", mode(&key));
    }
}
