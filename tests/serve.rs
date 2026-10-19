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
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let req = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n{headers}Connection: close\r\n\r\n");
    s.write_all(req.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).await.unwrap();
    let text = String::from_utf8_lossy(&buf).to_string();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    (head.to_ascii_lowercase(), body.to_string())
}

#[tokio::test]
async fn serves_html_with_reload_script_and_no_cache() {
    let port = start(site("html")).await;
    let (head, body) = get(port, "/").await;
    assert!(head.starts_with("http/1.1 200"), "{head}");
    assert!(head.contains("content-type: text/html"));
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

/// Opens a WebSocket to /__live/ws and returns the stream after the handshake.
async fn ws_connect(port: u16) -> TcpStream {
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let req = "GET /__live/ws HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n";
    s.write_all(req.as_bytes()).await.unwrap();
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        s.read_exact(&mut byte).await.unwrap();
        head.push(byte[0]);
    }
    assert!(String::from_utf8_lossy(&head).starts_with("HTTP/1.1 101"));
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
