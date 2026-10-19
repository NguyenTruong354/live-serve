//! Integration tests: a real server on a random port, spoken to over raw TCP (HTTP and WebSocket).

use std::path::PathBuf;
use std::time::Duration;

use live_serve::{Config, serve};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

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

/// Opens a WebSocket to /__live/ws and returns the stream after the handshake.
async fn ws_connect(port: u16) -> TcpStream {
    let (head, s) = ws_open(port, "").await;
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");
    s
}

/// Reads one short text frame (server frames are not masked).
async fn ws_read_text(s: &mut TcpStream) -> String {
    let mut h = [0u8; 2];
    s.read_exact(&mut h).await.unwrap();
    assert_eq!(h[0] & 0x0f, 1, "text frame");
    let len = (h[1] & 0x7f) as usize;
    let mut payload = vec![0u8; len];
    s.read_exact(&mut payload).await.unwrap();
    String::from_utf8(payload).unwrap()
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
