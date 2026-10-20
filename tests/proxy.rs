//! `--proxy`: a real backend behind live-serve, spoken to over raw TCP.

use std::path::PathBuf;
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::response::IntoResponse;
use live_serve::proxy::Proxy;
use live_serve::{Config, serve};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn site(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("live-serve-proxy-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("api")).unwrap();
    std::fs::write(dir.join("index.html"), "<html><body>static</body></html>").unwrap();
    // A file where a proxied path is: the backend still gets the request.
    std::fs::write(dir.join("api").join("users"), "static users file").unwrap();
    std::fs::write(dir.join("apix"), "static apix").unwrap();
    dir
}

/// An API that answers with what it was sent: method, path and query, a few headers, and the body.
/// `/created` answers 201 with a header of its own; `/redirect` sends to the backend's own `/login` in
/// full, `/relative` from its root (`/login`), as most frameworks do.
async fn backend(name: &'static str) -> u16 {
    let app = Router::new().fallback(move |method: Method, uri: Uri, headers: HeaderMap, body: Bytes| async move {
        let port = headers.get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("").to_string();
        match uri.path() {
            "/created" | "/v1/created" => (StatusCode::CREATED, [("x-backend", name)], "made").into_response(),
            "/redirect" => {
                let to = format!("http://{port}/login?next=1");
                (StatusCode::FOUND, [(header::LOCATION, to)]).into_response()
            }
            "/relative" => (StatusCode::FOUND, [(header::LOCATION, "/login?next=1")]).into_response(),
            _ => {
                let seen = |h: &str| headers.get(h).and_then(|v| v.to_str().ok()).unwrap_or("-").to_string();
                format!(
                    "{name} {method} {uri} host={} xfh={} xfp={} hop={} body={}",
                    seen("host"),
                    seen("x-forwarded-host"),
                    seen("x-forwarded-proto"),
                    seen("x-hop"),
                    String::from_utf8_lossy(&body)
                )
                .into_response()
            }
        }
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    port
}

async fn start(root: PathBuf, rules: &[String]) -> u16 {
    let mut cfg = Config::new(root);
    cfg.proxies = rules.iter().map(|r| Proxy::parse(r).unwrap()).collect();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { serve(listener, cfg).await.unwrap() });
    tokio::time::sleep(Duration::from_millis(200)).await;
    port
}

/// Sends a raw request (`head` without the final blank line) and returns the lowercased head and body.
async fn send(port: u16, head: &str, body: &str) -> (String, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let request = format!("{head}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    s.write_all(request.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), s.read_to_end(&mut buf)).await.unwrap().unwrap();
    let text = String::from_utf8_lossy(&buf).to_string();
    let (h, b) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    (h.to_ascii_lowercase(), b.to_string())
}

async fn get(port: u16, path: &str) -> (String, String) {
    send(port, &format!("GET {path} HTTP/1.1\r\nHost: localhost:{port}"), "").await
}

#[tokio::test]
async fn requests_under_the_prefix_go_to_the_backend_without_it() {
    let api = backend("api").await;
    let port = start(site("get"), &[format!("/api=http://127.0.0.1:{api}")]).await;
    let (head, body) = get(port, "/api/users?page=2").await;
    assert!(head.starts_with("http/1.1 200"), "{head}");
    assert!(body.starts_with("api GET /users?page=2 "), "{body}");
    assert!(head.contains("cache-control: no-store"), "a dev server's answers are never cached: {head}");
    // The prefix alone is the backend's root.
    let (_, body) = get(port, "/api").await;
    assert!(body.starts_with("api GET / "), "{body}");
}

#[tokio::test]
async fn a_path_on_the_target_replaces_the_prefix() {
    let api = backend("v1").await;
    let port = start(site("path"), &[format!("/api=http://127.0.0.1:{api}/v1")]).await;
    let (_, body) = get(port, "/api/users").await;
    assert!(body.starts_with("v1 GET /v1/users "), "{body}");
    let (head, body) = get(port, "/api/created").await;
    assert!(head.starts_with("http/1.1 201"), "status passes through: {head}");
    assert!(head.contains("x-backend: v1"), "headers pass through: {head}");
    assert_eq!(body, "made");
}

#[tokio::test]
async fn methods_bodies_and_forwarding_headers_pass_through() {
    let api = backend("api").await;
    let port = start(site("post"), &[format!("/api=http://127.0.0.1:{api}")]).await;
    for method in ["POST", "PUT", "PATCH", "DELETE"] {
        let head = format!(
            "{method} /api/items HTTP/1.1\r\nHost: localhost:{port}\r\nContent-Type: application/json\r\nX-Hop: secret\r\nConnection: x-hop"
        );
        let (status, body) = send(port, &head, r#"{"name":"a"}"#).await;
        assert!(status.starts_with("http/1.1 200"), "{method}: {status}");
        assert!(body.starts_with(&format!("api {method} /items ")), "{body}");
        assert!(body.ends_with(r#"body={"name":"a"}"#), "{body}");
        assert!(body.contains(&format!("host=127.0.0.1:{api} ")), "Host names the backend: {body}");
        assert!(body.contains(&format!("xfh=localhost:{port} ")), "the original host: {body}");
        assert!(body.contains("xfp=http "), "{body}");
        assert!(body.contains("hop=-"), "a header the request's Connection names stays behind: {body}");
    }
}

#[tokio::test]
async fn whole_segments_only_and_the_longest_prefix_wins() {
    let (api, admin) = (backend("api").await, backend("admin").await);
    let rules = [format!("/api=http://127.0.0.1:{api}"), format!("/api/admin=http://127.0.0.1:{admin}")];
    let port = start(site("routes"), &rules).await;
    let (_, body) = get(port, "/api/admin/users").await;
    assert!(body.starts_with("admin GET /users "), "{body}");
    let (_, body) = get(port, "/api/users").await;
    assert!(body.starts_with("api GET /users "), "the backend wins over a file of that name: {body}");
    let (_, body) = get(port, "/apix").await;
    assert_eq!(body, "static apix", "/apix is not under /api");
    let (_, body) = get(port, "/").await;
    assert!(body.contains("static"), "everything else is still the folder: {body}");
}

#[tokio::test]
async fn a_backends_redirect_to_itself_points_back_under_the_prefix() {
    let api = backend("api").await;
    let port = start(site("redirect"), &[format!("/api=http://127.0.0.1:{api}")]).await;
    let (head, _) = get(port, "/api/redirect").await;
    assert!(head.starts_with("http/1.1 302"), "{head}");
    assert!(head.contains("location: /api/login?next=1\r\n"), "{head}");
    // `res.redirect("/login")` and the like: from the backend's root, so under the prefix too.
    let (head, _) = get(port, "/api/relative").await;
    assert!(head.starts_with("http/1.1 302"), "{head}");
    assert!(head.contains("location: /api/login?next=1\r\n"), "{head}");
}

#[tokio::test]
async fn a_backend_that_is_down_is_a_502_that_says_so() {
    let free = TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap().port();
    let port = start(site("down"), &[format!("/api=http://127.0.0.1:{free}")]).await;
    let (head, body) = get(port, "/api/users").await;
    assert!(head.starts_with("http/1.1 502"), "{head}");
    assert!(body.contains(&format!("cannot reach http://127.0.0.1:{free} for http://127.0.0.1:{free}/users")), "{body}");
}

#[tokio::test]
async fn websockets_are_refused_plainly_and_hosts_still_checked() {
    let api = backend("api").await;
    let port = start(site("ws"), &[format!("/api=http://127.0.0.1:{api}")]).await;
    let upgrade = format!(
        "GET /api/socket HTTP/1.1\r\nHost: localhost:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13"
    );
    let (head, body) = send(port, &upgrade, "").await;
    assert!(head.starts_with("http/1.1 501"), "{head}");
    assert!(body.contains("WebSockets"), "{body}");
    // Another upgrade (curl --http2 over http://) is only an offer: the request goes on as HTTP/1.1.
    let h2c = format!(
        "GET /api/users HTTP/1.1\r\nHost: localhost:{port}\r\nUpgrade: h2c\r\nConnection: Upgrade, HTTP2-Settings\r\n\
HTTP2-Settings: AAMAAABkAARAAAAAAAIAAAAA"
    );
    let (head, body) = send(port, &h2c, "").await;
    assert!(head.starts_with("http/1.1 200"), "{head}");
    assert!(body.starts_with("api GET /users "), "{body}");
    // An outside site's name pointed at this machine cannot reach the backend through it either.
    let (head, _) = send(port, "GET /api/users HTTP/1.1\r\nHost: evil.example", "").await;
    assert!(head.starts_with("http/1.1 403"), "{head}");
}

#[tokio::test]
async fn an_http_1_0_backend_still_answers_the_browser_in_http_1_1() {
    // Like python -m http.server: answers in HTTP/1.0 and closes.
    let raw = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api = raw.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = raw.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf).await;
                let _ = s.write_all(b"HTTP/1.0 200 OK\r\nContent-Type: application/json\r\n\r\n{\"ok\":true}").await;
            });
        }
    });
    let port = start(site("http10"), &[format!("/api=http://127.0.0.1:{api}")]).await;
    let (head, body) = get(port, "/api/status").await;
    assert!(head.starts_with("http/1.1 200"), "{head}");
    // Without a length from the backend, the body goes chunked, as HTTP/1.1 does.
    assert!(head.contains("transfer-encoding: chunked"), "{head}");
    assert!(body.contains(r#"{"ok":true}"#), "{body}");
}
