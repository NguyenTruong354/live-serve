//! Integration tests: a real server on a random port, spoken to over raw TCP (HTTP and WebSocket).
//! The reload script's own tests run it in Node.js (`NODE`, or `node` on `PATH`). Without it they print
//! SKIPPED and pass, except on CI (`CI` set), where they fail.

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use live_serve::{Config, serve};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

fn site(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("live-serve-it-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("node_modules")).unwrap();
    std::fs::write(dir.join("index.html"), "<html><body><h1>hi</h1></body></html>").unwrap();
    std::fs::write(dir.join("app.js"), "console.log(1)").unwrap();
    std::fs::write(dir.join("style.css"), "h1{}").unwrap();
    dir
}

async fn start(root: PathBuf) -> u16 {
    start_with(Config::new(root)).await
}

async fn start_with(cfg: Config) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { serve(listener, cfg).await.unwrap() });
    tokio::time::sleep(Duration::from_millis(200)).await;
    port
}

async fn get(port: u16, path: &str) -> (String, String) {
    request(port, path, "").await
}

/// GET with extra header lines (each ending in "\r\n").
async fn request(port: u16, path: &str, headers: &str) -> (String, String) {
    let (head, body) = request_raw(port, path, "localhost", headers).await;
    (head, String::from_utf8_lossy(&body).to_string())
}

/// GET for `host`; returns the lowercased head and the body's bytes.
async fn request_raw(port: u16, path: &str, host: &str, headers: &str) -> (String, Vec<u8>) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let req = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n{headers}Connection: close\r\n\r\n");
    s.write_all(req.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).await.unwrap();
    let split = buf.windows(4).position(|w| w == b"\r\n\r\n").map_or(buf.len(), |at| at + 4);
    let head = String::from_utf8_lossy(&buf[..split]).to_ascii_lowercase();
    (head, buf[split..].to_vec())
}

#[tokio::test]
async fn serves_html_with_reload_script_and_no_cache() {
    let port = start(site("html")).await;
    let (head, body) = get(port, "/").await;
    assert!(head.starts_with("http/1.1 200"), "{head}");
    assert!(head.contains("content-type: text/html; charset=utf-8"));
    assert!(head.contains("cache-control: no-store"));
    assert!(body.contains("<h1>hi</h1>"));
    assert!(body.contains("/__live/ws"));
}

#[tokio::test]
async fn serves_other_files_untouched_and_refuses_escapes() {
    let port = start(site("files")).await;
    let (head, body) = get(port, "/app.js?v=2").await;
    assert!(head.starts_with("http/1.1 200"));
    assert!(head.contains("text/javascript"));
    assert_eq!(body, "console.log(1)");

    let (head, _) = get(port, "/../Cargo.toml").await;
    assert!(head.starts_with("http/1.1 404"), "{head}");
    let (head, _) = get(port, "/nope.html").await;
    assert!(head.starts_with("http/1.1 404"));
}

#[tokio::test]
async fn refuses_unknown_host_names() {
    let port = start(site("hosts")).await;
    // DNS rebinding: an outside name pointed at 127.0.0.1.
    let (head, _) = request_raw(port, "/", "evil.example:5500", "").await;
    assert!(head.starts_with("http/1.1 403"), "{head}");
    for local in ["localhost:5500", "127.0.0.1:5500", "[::1]:5500", "app.localhost"] {
        let (head, _) = request_raw(port, "/", local, "").await;
        assert!(head.starts_with("http/1.1 200"), "{local}: {head}");
    }
}

#[tokio::test]
async fn answers_allowed_host_names() {
    let mut cfg = Config::new(site("allowed"));
    cfg.allowed_hosts = vec!["myapp.test".into()];
    let port = start_with(cfg).await;
    let (head, _) = request_raw(port, "/", "myapp.test:5500", "").await;
    assert!(head.starts_with("http/1.1 200"), "{head}");
    let (head, _) = request_raw(port, "/", "other.test", "").await;
    assert!(head.starts_with("http/1.1 403"), "{head}");
}

#[tokio::test]
async fn sends_cors_only_when_asked() {
    let root = site("cors");
    let port = start(root.clone()).await;
    for path in ["/", "/app.js", "/nope"] {
        let (head, _) = request(port, path, "Origin: https://evil.example\r\n").await;
        assert!(!head.contains("access-control-allow-origin"), "{path}: {head}");
    }

    let mut cfg = Config::new(root);
    cfg.cors = true;
    let port = start_with(cfg).await;
    let (head, _) = get(port, "/app.js").await;
    assert!(head.contains("access-control-allow-origin: *"), "{head}");
}

#[tokio::test]
async fn hides_dotfiles_unless_asked() {
    let root = site("dotfiles");
    std::fs::write(root.join(".env"), "SECRET=1").unwrap();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(root.join(".git").join("config"), "[core]").unwrap();
    std::fs::create_dir_all(root.join("docs")).unwrap();
    std::fs::write(root.join("docs").join(".secret"), "x").unwrap();
    std::fs::write(root.join("docs").join("readme.txt"), "r").unwrap();
    let port = start(root.clone()).await;

    for path in ["/.env", "/.git/config", "/.git/", "/docs/.secret"] {
        let (head, body) = get(port, path).await;
        assert!(head.starts_with("http/1.1 404"), "{path}: {head}");
        assert!(!body.contains("SECRET"));
    }
    let (_, listing) = get(port, "/docs/").await;
    assert!(listing.contains("readme.txt"));
    assert!(!listing.contains(".secret"), "hidden from listings too");

    let mut cfg = Config::new(root);
    cfg.dotfiles = true;
    let port = start_with(cfg).await;
    let (head, body) = get(port, "/.env").await;
    assert!(head.starts_with("http/1.1 200"), "{head}");
    assert_eq!(body, "SECRET=1");
}

#[tokio::test]
async fn keeps_the_file_servers_content_type_for_other_types() {
    let root = site("types");
    std::fs::write(root.join("feed.xml"), "<a/>").unwrap();
    let port = start(root).await;
    let (head, _) = get(port, "/feed.xml").await;
    assert!(head.starts_with("http/1.1 200"), "{head}");
    assert!(head.contains("xml"), "{head}");
    assert!(!head.contains("octet-stream"), "{head}");
}

#[tokio::test]
async fn keeps_html_that_is_not_utf8() {
    let root = site("latin1");
    std::fs::write(root.join("latin.html"), b"<html><body>caf\xe9</body></html>").unwrap();
    let port = start(root).await;
    let (head, body) = request_raw(port, "/latin.html", "localhost", "").await;
    assert!(head.starts_with("http/1.1 200"), "{head}");
    assert!(head.contains("content-type: text/html"));
    assert!(!head.contains("charset=utf-8"), "the page's own <meta charset> decides");
    assert!(body.starts_with(b"<html><body>caf\xe9<script>"), "bytes untouched");
}

#[tokio::test]
async fn redirects_directories_relatively() {
    let root = site("redirect");
    std::fs::create_dir_all(root.join("docs")).unwrap();
    let port = start(root).await;
    for path in ["/docs", "//docs"] {
        let (head, _) = get(port, path).await;
        assert!(head.starts_with("http/1.1 301"), "{path}: {head}");
        assert!(head.contains("location: ./docs/\r\n"), "{path}: {head}");
    }
}

/// Opens a WebSocket to /__live/ws with extra header lines and returns the response head and stream.
async fn ws_open(port: u16, headers: &str) -> (String, TcpStream) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let req = format!(
        "GET /__live/ws HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n{headers}\r\n"
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        s.read_exact(&mut byte).await.unwrap();
        head.push(byte[0]);
    }
    (String::from_utf8_lossy(&head).to_string(), s)
}

/// Opens a WebSocket to /__live/ws and returns the stream after the handshake and the version token.
async fn ws_connect(port: u16) -> TcpStream {
    let (head, mut s) = ws_open(port, "").await;
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");
    let token = ws_read_frame(&mut s).await;
    assert!(token.starts_with("id:"), "{token}");
    s
}

/// Reads one short text frame (server frames are not masked).
async fn ws_read_frame(s: &mut TcpStream) -> String {
    let mut h = [0u8; 2];
    s.read_exact(&mut h).await.unwrap();
    assert_eq!(h[0] & 0x0f, 1, "text frame");
    let len = (h[1] & 0x7f) as usize;
    let mut payload = vec![0u8; len];
    s.read_exact(&mut payload).await.unwrap();
    String::from_utf8(payload).unwrap()
}

/// Reads the next change message, skipping version tokens.
async fn ws_read_text(s: &mut TcpStream) -> String {
    loop {
        let text = ws_read_frame(s).await;
        if !text.starts_with("id:") {
            return text;
        }
    }
}

#[tokio::test]
async fn sends_a_version_token_on_connect_and_after_each_change() {
    let root = site("token");
    let port = start(root.clone()).await;
    // macOS's FSEvents may still report the files `site` wrote just before the server started, which
    // counts as a change: wait for that burst to pass, and compare counts instead of expecting 0.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let (_, mut ws) = ws_open(port, "").await;
    let first = ws_read_frame(&mut ws).await;
    let (boot, count) = first.rsplit_once('.').expect("id:<boot>.<changes>");
    assert!(boot.starts_with("id:"), "{first}");
    let count: u64 = count.parse().expect("a count");
    tokio::time::sleep(Duration::from_millis(300)).await;
    std::fs::write(root.join("app.js"), "console.log(2)").unwrap();
    let change = tokio::time::timeout(Duration::from_secs(5), ws_read_frame(&mut ws)).await.expect("change");
    assert_eq!(change, "reload");
    let next = ws_read_frame(&mut ws).await;
    assert_eq!(next, format!("{boot}.{}", count + 1), "same run, one change later");

    let (_, mut other) = ws_open(start(root).await, "").await;
    let other = ws_read_frame(&mut other).await;
    assert_ne!(other.split('.').next(), first.split('.').next(), "another run, another token");
}

/// Every message that arrives until none comes for `quiet`.
async fn ws_read_all(s: &mut TcpStream, quiet: Duration) -> Vec<String> {
    let mut messages = Vec::new();
    while let Ok(msg) = tokio::time::timeout(quiet, ws_read_text(s)).await {
        messages.push(msg);
    }
    messages
}

#[tokio::test]
async fn websocket_refuses_other_origins() {
    let port = start(site("origin")).await;
    let (head, _) = ws_open(port, "Origin: https://evil.example\r\n").await;
    assert!(head.starts_with("HTTP/1.1 403"), "{head}");
    let (head, _) = ws_open(port, &format!("Origin: http://localhost:{port}\r\n")).await;
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");
}

async fn expect_message(port: u16, root: &PathBuf, file: &str, expected: &str) {
    let mut ws = ws_connect(port).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    std::fs::write(root.join(file), format!("/* {} */", std::time::SystemTime::now().elapsed().map(|d| d.as_nanos()).unwrap_or(0))).unwrap();
    let msg = tokio::time::timeout(Duration::from_secs(5), ws_read_text(&mut ws))
        .await
        .expect("message within 5 s");
    assert_eq!(msg, expected);
}

#[tokio::test]
async fn tells_browsers_to_reload_on_js_change() {
    let root = site("js");
    let port = start(root.clone()).await;
    expect_message(port, &root, "app.js", "reload").await;
}

#[tokio::test]
async fn tells_browsers_to_swap_css_on_css_change() {
    let root = site("css");
    let port = start(root.clone()).await;
    expect_message(port, &root, "style.css", "css").await;
}

#[tokio::test]
async fn an_editor_save_with_temp_files_still_swaps_css() {
    let root = site("temp");
    let port = start(root.clone()).await;
    let mut ws = ws_connect(port).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    // What Vim does on :w — a probe file, a backup, then the file itself.
    std::fs::write(root.join("4913"), "").unwrap();
    std::fs::write(root.join("style.css~"), "h1{}").unwrap();
    std::fs::write(root.join("style.css"), "h1{color:red}").unwrap();
    let messages = ws_read_all(&mut ws, Duration::from_secs(1)).await;
    assert_eq!(messages, ["css"]);
}

#[tokio::test]
async fn a_save_through_a_temporary_file_still_swaps_css() {
    let root = site("atomic");
    let port = start(root.clone()).await;
    let mut ws = ws_connect(port).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    // What write-file-atomic and similar tools do: write a sibling, then rename it over the file.
    for (i, temp) in ["style.css.tmp.4242.1792413777242", "style.css.2948423"].into_iter().enumerate() {
        std::fs::write(root.join(temp), format!("h1{{color:#{i}{i}{i}}}")).unwrap();
        std::fs::rename(root.join(temp), root.join("style.css")).unwrap();
        let messages = ws_read_all(&mut ws, Duration::from_secs(1)).await;
        assert_eq!(messages, ["css"], "{temp}");
    }
}

#[tokio::test]
async fn one_reload_for_a_burst_of_writes() {
    let root = site("burst");
    let port = start(root.clone()).await;
    let mut ws = ws_connect(port).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    // A build writing a file every 20 ms for 600 ms: shorter gaps than the debounce, longer than it in all.
    for i in 0..30 {
        std::fs::write(root.join(format!("chunk{}.js", i % 5)), format!("// {i}")).unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let messages = ws_read_all(&mut ws, Duration::from_secs(1)).await;
    assert_eq!(messages, ["reload"]);
}

#[tokio::test]
async fn ignores_changes_in_node_modules() {
    let root = site("ignored");
    let port = start(root.clone()).await;
    let mut ws = ws_connect(port).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    std::fs::write(root.join("node_modules").join("x.js"), "x").unwrap();
    let got = tokio::time::timeout(Duration::from_millis(800), ws_read_text(&mut ws)).await;
    assert!(got.is_err(), "no message for node_modules");
}

#[tokio::test]
async fn lists_a_directory_without_index_html() {
    let root = site("listing");
    std::fs::create_dir_all(root.join("docs").join("guides")).unwrap();
    std::fs::write(root.join("docs").join("readme.txt"), "r").unwrap();
    let port = start(root).await;

    let (head, body) = get(port, "/docs/").await;
    assert!(head.starts_with("http/1.1 200"), "{head}");
    assert!(head.contains("text/html"));
    assert!(body.contains("Index of /docs/"));
    assert!(body.contains(r#"href="readme.txt""#));
    assert!(body.contains(r#"href="guides/""#));
    assert!(body.contains("/__live/ws"), "listing reloads too");
}

#[tokio::test]
async fn serves_byte_ranges_for_media() {
    let root = site("range");
    std::fs::write(root.join("clip.bin"), b"0123456789").unwrap();
    let port = start(root).await;

    let (head, body) = request(port, "/clip.bin", "Range: bytes=2-5\r\n").await;
    assert!(head.starts_with("http/1.1 206"), "{head}");
    assert!(head.contains("content-range: bytes 2-5/10"));
    assert!(head.contains("cache-control: no-store"));
    assert_eq!(body, "2345");
}

#[tokio::test]
async fn spa_falls_back_to_index_for_routes_but_not_assets() {
    let root = site("spa");
    let mut cfg = Config::new(root);
    cfg.spa = true;
    let port = start_with(cfg).await;

    let (head, body) = get(port, "/settings/profile").await;
    assert!(head.starts_with("http/1.1 200"), "{head}");
    assert!(body.contains("<h1>hi</h1>"));
    let (head, _) = get(port, "/missing.js").await;
    assert!(head.starts_with("http/1.1 404"), "assets still 404");
}

#[tokio::test]
async fn without_spa_unknown_routes_are_404() {
    let port = start(site("nospa")).await;
    let (head, _) = get(port, "/settings/profile").await;
    assert!(head.starts_with("http/1.1 404"));
}

#[tokio::test]
async fn no_reload_serves_plain_html() {
    let root = site("plain");
    let mut cfg = Config::new(root);
    cfg.reload = false;
    let port = start_with(cfg).await;

    let (head, body) = get(port, "/").await;
    assert!(head.starts_with("http/1.1 200"));
    assert_eq!(body, "<html><body><h1>hi</h1></body></html>");
    let (head, _) = get(port, "/__live/ws").await;
    assert!(head.starts_with("http/1.1 404"));
}

#[tokio::test]
async fn serves_whole_files_and_head_requests() {
    let root = site("whole");
    let big: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(root.join("bundle.js"), &big).unwrap();
    let port = start(root).await;

    let (head, body) = request_raw(port, "/bundle.js", "localhost", "").await;
    assert!(head.starts_with("http/1.1 200"), "{head}");
    assert!(head.contains("content-type: text/javascript"));
    assert!(head.contains("cache-control: no-store"));
    assert_eq!(body, big, "every byte, in order");

    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    s.write_all(b"HEAD /bundle.js HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").await.unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).await.unwrap();
    let text = String::from_utf8_lossy(&buf).to_ascii_lowercase();
    assert!(text.starts_with("http/1.1 200"), "{text}");
    assert!(text.contains("content-length: 3000000"), "{text}");
    assert!(text.ends_with("\r\n\r\n"), "no body for HEAD");
}


#[tokio::test]
async fn polling_finds_changes_and_still_ignores_node_modules() {
    let root = site("poll");
    let mut cfg = Config::new(root.clone());
    cfg.poll = true;
    let port = start_with(cfg).await;
    let mut ws = ws_connect(port).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    std::fs::write(root.join("node_modules").join("x.js"), "x").unwrap();
    let got = tokio::time::timeout(Duration::from_millis(1200), ws_read_text(&mut ws)).await;
    assert!(got.is_err(), "no message for node_modules");

    let written = std::time::Instant::now();
    std::fs::write(root.join("app.js"), "console.log(2)").unwrap();
    let first = tokio::time::timeout(Duration::from_secs(5), ws_read_text(&mut ws)).await.expect("message within 5 s");
    // Polling waits DEBOUNCE + POLL_INTERVAL (420 ms) for quiet; file events only DEBOUNCE (120 ms).
    assert!(written.elapsed() >= Duration::from_millis(400), "{:?}: not the polling path", written.elapsed());
    let mut messages = vec![first];
    messages.extend(ws_read_all(&mut ws, Duration::from_millis(1500)).await);
    assert_eq!(messages, ["reload"]);
}

/// Forwards connections to a server and can cut them all, refuse new ones, or switch to another
/// server: what the page sees when the server stops and starts again.
struct Proxy {
    port: u16,
    to: Arc<AtomicU16>,
    down: Arc<AtomicBool>,
    connections: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

impl Proxy {
    async fn start(to: u16) -> Proxy {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy = Proxy {
            port: listener.local_addr().unwrap().port(),
            to: Arc::new(AtomicU16::new(to)),
            down: Arc::new(AtomicBool::new(false)),
            connections: Arc::new(Mutex::new(Vec::new())),
        };
        let (to, down, list) = (proxy.to.clone(), proxy.down.clone(), proxy.connections.clone());
        tokio::spawn(async move {
            while let Ok((mut inbound, _)) = listener.accept().await {
                if down.load(Ordering::SeqCst) {
                    continue;
                }
                let to = to.load(Ordering::SeqCst);
                let task = tokio::spawn(async move {
                    if let Ok(mut outbound) = TcpStream::connect(("127.0.0.1", to)).await {
                        let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
                    }
                });
                list.lock().unwrap().push(task);
            }
        });
        proxy
    }

    /// Closes every open connection.
    fn cut(&self) {
        for task in self.connections.lock().unwrap().drain(..) {
            task.abort();
        }
    }

    /// While down, new connections are closed right away.
    fn set_down(&self, down: bool) {
        self.down.store(down, Ordering::SeqCst);
    }

    /// New connections go to the server on `port`.
    fn retarget(&self, port: u16) {
        self.to.store(port, Ordering::SeqCst);
    }
}

/// Stands in for the browser: `location.reload()` prints "reload" and exits; the page's socket
/// prints "open" and "message <text>", its fetches "fetch <status>" (plus " listing" for a
/// directory listing) or "fetch failed".
const BROWSER: &str = r#"const href = process.argv[2], url = new URL(href);
globalThis.location = { protocol: url.protocol, host: url.host, href, reload() { console.log("reload"); process.exit(0); } };
globalThis.document = { querySelectorAll: () => [] };
const realFetch = fetch;
globalThis.fetch = (...args) => realFetch(...args).then(
  (r) => { console.log("fetch " + r.status + (r.headers.get("x-live-serve") === "listing" ? " listing" : "")); return r; },
  (e) => { console.log("fetch failed"); throw e; });
globalThis.WebSocket = class extends WebSocket {
  constructor(u) {
    super(u);
    this.addEventListener("open", () => console.log("open"));
    this.addEventListener("message", (e) => console.log("message " + e.data));
  }
};
"#;

/// The reload script of a served page, running in Node.js; killed on drop.
struct Page {
    child: Child,
    lines: mpsc::UnboundedReceiver<String>,
}

impl Page {
    /// Runs the script of the page at `path` on `port` as if the page were `http://127.0.0.1:{location_port}{path}`.
    /// Without Node.js 20.10+ this is None and the test passes with a SKIPPED note, except on CI (`CI` set),
    /// where it fails.
    async fn open(port: u16, path: &str, location_port: u16) -> Option<Page> {
        let node = std::env::var("NODE").unwrap_or_else(|_| "node".into());
        let check = Command::new(&node)
            .args(["--experimental-websocket", "-e", "process.exit(typeof WebSocket === 'function' ? 0 : 1)"])
            .output();
        if !check.is_ok_and(|o| o.status.success()) {
            assert!(std::env::var_os("CI").is_none(), "CI needs Node.js 20.10+ for this test: set NODE or put node on PATH");
            // Straight to stderr: the test harness would hide eprintln! output of a passing test.
            let _ = writeln!(std::io::stderr(), "SKIPPED: this test needs Node.js 20.10+; set NODE or put node on PATH");
            return None;
        }
        let (_, html) = get(port, path).await;
        // The injected script, not one of the page's own (a listing has its filter box script).
        let marker = html.find("/__live/ws").expect("script injected");
        let start = html[..marker].rfind("<script>").unwrap() + "<script>".len();
        let end = html[start..].find("</script>").unwrap() + start;
        let file = std::env::temp_dir().join(format!("live-serve-page-{}-{location_port}.cjs", std::process::id()));
        std::fs::write(&file, format!("{BROWSER}{}", &html[start..end])).unwrap();

        let mut child = Command::new(&node)
            .arg("--experimental-websocket")
            .arg(&file)
            .arg(format!("http://127.0.0.1:{location_port}{path}"))
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = mpsc::unbounded_channel();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines().map_while(Result::ok) {
                let _ = tx.send(line);
            }
        });
        Some(Page { child, lines })
    }

    /// Every line up to and including the first that `wanted` accepts, which must come within 5 s
    /// (in all: a page retrying every 500 ms prints a line each time).
    async fn until_line(&mut self, what: &str, wanted: impl Fn(&str) -> bool) -> Vec<String> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let mut seen = Vec::new();
        loop {
            match tokio::time::timeout_at(deadline, self.lines.recv()).await {
                Ok(Some(line)) => {
                    let done = wanted(&line);
                    seen.push(line);
                    if done {
                        return seen;
                    }
                }
                _ => panic!("no {what} within 5 s; got {seen:?}"),
            }
        }
    }

    /// Every line up to and including `expected`.
    async fn until(&mut self, expected: &str) -> Vec<String> {
        self.until_line(&format!("{expected:?}"), |line| line == expected).await
    }

    /// Every line up to and including the next version token.
    async fn until_token(&mut self) -> Vec<String> {
        self.until_line("version token", |line| line.starts_with("message id:")).await
    }

    /// The lines printed so far, without waiting.
    fn printed(&mut self) -> Vec<String> {
        std::iter::from_fn(|| self.lines.try_recv().ok()).collect()
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

/// The line before the last, which comes right before "reload".
fn before_last(lines: &[String]) -> &str {
    lines.len().checked_sub(2).map_or("", |at| &lines[at])
}

#[tokio::test]
async fn reconnecting_without_a_change_keeps_the_page() {
    let port = start(site("reconnect")).await;
    let proxy = Proxy::start(port).await;
    let Some(mut page) = Page::open(port, "/", proxy.port).await else { return };
    page.until_token().await;

    proxy.cut();
    page.until("open").await;
    page.until_token().await;
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(page.printed(), Vec::<String>::new(), "same token: no fetch, no reload");
}

#[tokio::test]
async fn reloads_after_a_restart() {
    let root = site("restart");
    let first = start(root.clone()).await;
    let proxy = Proxy::start(first).await;
    let Some(mut page) = Page::open(first, "/", proxy.port).await else { return };
    page.until_token().await;

    // Another run of the server on the same files, with nothing changed.
    proxy.retarget(start(root).await);
    proxy.cut();
    let lines = page.until("reload").await;
    assert_eq!(before_last(&lines), "fetch 200", "{lines:?}");
}

#[tokio::test]
async fn reloads_after_a_change_made_while_disconnected() {
    let root = site("offline");
    let port = start(root.clone()).await;
    let proxy = Proxy::start(port).await;
    let Some(mut page) = Page::open(port, "/", proxy.port).await else { return };
    page.until_token().await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    proxy.set_down(true);
    proxy.cut();
    std::fs::write(root.join("app.js"), "console.log(2)").unwrap();
    tokio::time::sleep(Duration::from_millis(600)).await;
    let offline = page.printed();
    assert!(!offline.iter().any(|l| l.starts_with("message") || l == "reload"), "it was offline: {offline:?}");

    proxy.set_down(false);
    let lines = page.until("reload").await;
    assert_eq!(before_last(&lines), "fetch 200", "{lines:?}");
}

#[tokio::test]
async fn reloads_only_once_the_page_works_again() {
    let root = site("broken");
    std::fs::write(root.join("page.html"), "<html><body>v1</body></html>").unwrap();
    let port = start(root.clone()).await;
    let Some(mut page) = Page::open(port, "/page.html", port).await else { return };
    page.until_token().await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    // A build that deleted the page and keeps writing: every change sends "reload".
    std::fs::remove_file(root.join("page.html")).unwrap();
    page.until("fetch 404").await;
    for i in 0..3 {
        tokio::time::sleep(Duration::from_millis(300)).await;
        std::fs::write(root.join("app.js"), format!("// {i}")).unwrap();
    }
    tokio::time::sleep(Duration::from_millis(700)).await;
    let waiting = page.printed();
    assert!(!waiting.iter().any(|l| l == "reload"), "never reloads into a 404: {waiting:?}");
    let fetches: Vec<&String> = waiting.iter().filter(|l| l.starts_with("fetch")).collect();
    assert!(fetches.iter().all(|l| *l == "fetch 404"), "{waiting:?}");
    // One retry every 500 ms over 1.6 s; the later reload messages start no second loop.
    assert!((1..=4).contains(&fetches.len()), "{waiting:?}");

    std::fs::write(root.join("page.html"), "<html><body>v2</body></html>").unwrap();
    let lines = page.until("reload").await;
    assert_eq!(before_last(&lines), "fetch 200", "{lines:?}");
}

#[tokio::test]
async fn waits_while_the_page_would_be_a_listing() {
    let root = site("wiped");
    let port = start(root.clone()).await;
    let Some(mut page) = Page::open(port, "/", port).await else { return };
    page.until_token().await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    // A build that wiped its output folder: "/" answers 200, but with a directory listing.
    std::fs::remove_file(root.join("index.html")).unwrap();
    page.until("fetch 200 listing").await;
    for i in 0..2 {
        tokio::time::sleep(Duration::from_millis(300)).await;
        std::fs::write(root.join("app.js"), format!("// {i}")).unwrap();
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    let waiting = page.printed();
    assert!(!waiting.iter().any(|l| l == "reload"), "never reloads into a listing: {waiting:?}");
    assert!(waiting.iter().filter(|l| l.starts_with("fetch")).all(|l| l == "fetch 200 listing"), "{waiting:?}");

    std::fs::write(root.join("index.html"), "<html><body>v2</body></html>").unwrap();
    let lines = page.until("reload").await;
    assert_eq!(before_last(&lines), "fetch 200", "{lines:?}");
}

#[tokio::test]
async fn a_listing_reloads_into_a_listing() {
    let root = site("listed");
    std::fs::create_dir_all(root.join("docs")).unwrap();
    let port = start(root.clone()).await;
    let Some(mut page) = Page::open(port, "/docs/", port).await else { return };
    page.until_token().await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    std::fs::write(root.join("docs").join("new.txt"), "n").unwrap();
    let lines = page.until("reload").await;
    assert_eq!(before_last(&lines), "fetch 200 listing", "{lines:?}");
}
