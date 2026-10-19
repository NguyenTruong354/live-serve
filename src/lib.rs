//! live-serve: a static dev server that reloads the page when files change (a Live Server replacement).
//!
//! - Serves the files under `root`, always with `Cache-Control: no-store`, so a fresh build is never cached.
//! - Injects a small script into every HTML page that opens a WebSocket to `/__live/ws`.
//! - Watches `root`: on a change it tells browsers "reload" (whole page) or "css" (swap stylesheets only).

pub mod init;

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use notify::{RecursiveMode, Watcher};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc};
use tower::ServiceExt;
use tower_http::services::ServeFile;

/// What browsers are told to do after a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// Reload the whole page.
    Reload,
    /// Re-fetch the stylesheets only.
    Css,
}

impl Change {
    fn as_str(self) -> &'static str {
        match self {
            Change::Reload => "reload",
            Change::Css => "css",
        }
    }
}

/// Directories never watched.
const ALWAYS_IGNORED: [&str; 2] = ["node_modules", ".git"];

/// Changes this close together are one change (saving a file raises several events).
const DEBOUNCE: Duration = Duration::from_millis(120);

const RELOAD_SCRIPT: &str = r#"<script>(()=>{const u=(location.protocol==="https:"?"wss://":"ws://")+location.host+"/__live/ws";const c=()=>{const w=new WebSocket(u);w.onmessage=e=>{if(e.data==="css"){for(const l of document.querySelectorAll('link[rel="stylesheet"]')){const h=new URL(l.href);h.searchParams.set("_live",Date.now());l.href=h.href}}else{location.reload()}};w.onclose=()=>setTimeout(c,1000)};c()})();</script>"#;

/// Inserts the reload script before the last `</body>`, or at the end when there is none.
pub fn inject_reload_script(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    match lower.rfind("</body>") {
        Some(at) => format!("{}{}{}", &html[..at], RELOAD_SCRIPT, &html[at..]),
        None => format!("{html}{RELOAD_SCRIPT}"),
    }
}

/// URL path → file under `root`; None when it leaves `root`, does not exist, or cannot be read.
/// A directory resolves to its `index.html`.
pub fn resolve_path(root: &Path, url_path: &str) -> Option<PathBuf> {
    let mut target = existing_target(root, url_path)?;
    if target.is_dir() {
        target.push("index.html");
    }
    (target.is_file() && inside(root, &target)).then_some(target)
}

/// URL path → a directory under `root` that has no `index.html` (shown as a listing).
pub fn resolve_dir(root: &Path, url_path: &str) -> Option<PathBuf> {
    let target = existing_target(root, url_path)?;
    (target.is_dir() && !target.join("index.html").is_file()).then_some(target)
}

/// The path a URL names under `root`, if it exists and stays inside `root`.
fn existing_target(root: &Path, url_path: &str) -> Option<PathBuf> {
    let path_only = url_path.split(['?', '#']).next().unwrap_or("");
    let decoded = percent_encoding::percent_decode_str(path_only).decode_utf8().ok()?;
    // "\" and drive letters ("C:") have no place in a URL path; refuse them so nothing escapes root on Windows.
    if decoded.contains('\\') || decoded.contains(':') || decoded.contains('\0') {
        return None;
    }

    let mut target = root.to_path_buf();
    for segment in decoded.split('/') {
        match segment {
            "" | "." => {}
            ".." => return None,
            s => target.push(s),
        }
    }
    (target.exists() && inside(root, &target)).then_some(target)
}

/// Guard against symlinks pointing outside root.
fn inside(root: &Path, target: &Path) -> bool {
    match (root.canonicalize(), target.canonicalize()) {
        (Ok(real_root), Ok(real_target)) => real_target.starts_with(real_root),
        _ => false,
    }
}

/// Whether the last path segment looks like a file ("app.js"), not a route ("settings").
fn looks_like_asset(url_path: &str) -> bool {
    let last = url_path.split(['?', '#']).next().unwrap_or("").rsplit('/').next().unwrap_or("");
    last.contains('.')
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Content-Type for a file extension.
pub fn content_type(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" | "cjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json; charset=utf-8",
        "txt" | "md" => "text/plain; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "wasm" => "application/wasm",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

/// Whether the file is inside an ignored directory (`node_modules`, `.git`, or one in `extra`).
pub fn is_ignored(path: &Path, root: &Path, extra: &[String]) -> bool {
    let relative = path.strip_prefix(root).unwrap_or(path);
    relative.components().any(|c| match c {
        Component::Normal(name) => {
            let name = name.to_string_lossy();
            ALWAYS_IGNORED.contains(&name.as_ref()) || extra.iter().any(|e| e == name.as_ref())
        }
        _ => false,
    })
}

/// Turns a batch of changed files into one change: only CSS → "css", anything else → "reload";
/// None when every file is ignored.
pub fn classify(paths: &[PathBuf], root: &Path, extra: &[String]) -> Option<Change> {
    let relevant: Vec<&PathBuf> = paths.iter().filter(|p| !is_ignored(p, root, extra)).collect();
    if relevant.is_empty() {
        return None;
    }
    let all_css = relevant.iter().all(|p| {
        p.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("css"))
    });
    Some(if all_css { Change::Css } else { Change::Reload })
}

/// Server settings.
#[derive(Debug, Clone)]
pub struct Config {
    /// Directory served and watched.
    pub root: PathBuf,
    /// Extra directory names not watched (besides `node_modules`, `.git`).
    pub ignore: Vec<String>,
    /// Single-page app: an unknown path without a file extension serves `/index.html`.
    pub spa: bool,
    /// Inject the reload script and watch files. Off = a plain static server.
    pub reload: bool,
}

impl Config {
    /// Serve `root` with the defaults: reload on, no SPA fallback, nothing extra ignored.
    pub fn new(root: PathBuf) -> Config {
        Config { root, ignore: vec![], spa: false, reload: true }
    }
}

/// Characters escaped in a file name used as one URL path segment.
const PATH_SEGMENT: &percent_encoding::AsciiSet = &percent_encoding::CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}')
    .add(b'/')
    .add(b'\\');

/// How many ports past the requested one `bind` tries when it is taken.
const PORT_ATTEMPTS: u16 = 20;

/// Binds `host:port`; if the port is taken and `strict` is false, tries the next ones.
pub async fn bind(host: &str, port: u16, strict: bool) -> std::io::Result<TcpListener> {
    let attempts = if strict || port == 0 { 1 } else { PORT_ATTEMPTS };
    let mut last_error = None;
    for offset in 0..attempts {
        let Some(candidate) = port.checked_add(offset) else { break };
        match TcpListener::bind((host, candidate)).await {
            Ok(listener) => return Ok(listener),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => last_error = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last_error.unwrap_or_else(|| std::io::Error::other("no port available")))
}

/// The URL to show for a bound address ("0.0.0.0" → localhost, IPv6 in brackets).
pub fn display_url(host: &str, port: u16) -> String {
    match host {
        "0.0.0.0" | "::" | "[::]" => format!("http://localhost:{port}/"),
        h if h.contains(':') && !h.starts_with('[') => format!("http://[{h}]:{port}/"),
        h => format!("http://{h}:{port}/"),
    }
}

/// An HTML page listing a directory: `entries` are (name, is_dir), shown dirs first.
pub fn directory_listing(url_path: &str, entries: &[(String, bool)]) -> String {
    let mut sorted: Vec<&(String, bool)> = entries.iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));

    let title = escape_html(url_path);
    let mut items = String::new();
    if url_path.trim_matches('/') != "" {
        items.push_str("<li><a href=\"../\">../</a></li>\n");
    }
    for (name, is_dir) in sorted {
        let href = percent_encoding::utf8_percent_encode(name, PATH_SEGMENT).to_string();
        let slash = if *is_dir { "/" } else { "" };
        items.push_str(&format!(
            "<li><a href=\"{href}{slash}\">{}{slash}</a></li>\n",
            escape_html(name)
        ));
    }
    format!(
        "<!doctype html>\n<html><head><meta charset=\"utf-8\"><title>Index of {title}</title>\
<style>body{{font:15px/1.6 system-ui,sans-serif;margin:2rem}}a{{text-decoration:none}}li{{list-style:none}}</style>\
</head><body><h1>Index of {title}</h1><ul>\n{items}</ul></body></html>\n"
    )
}

#[derive(Clone)]
struct AppState {
    root: Arc<PathBuf>,
    changes: broadcast::Sender<Change>,
    spa: bool,
    reload: bool,
}

/// Routes: `/__live/ws` is the WebSocket (when reloading), everything else a static file.
fn router(state: AppState) -> Router {
    let routes = if state.reload {
        Router::new().route("/__live/ws", get(ws_handler))
    } else {
        Router::new()
    };
    routes.fallback(file_handler).with_state(state)
}

async fn file_handler(State(state): State<AppState>, request: Request) -> Response {
    let url_path = request.uri().path().to_string();

    let path = match resolve_path(&state.root, &url_path) {
        Some(path) => path,
        None => {
            if let Some(dir) = resolve_dir(&state.root, &url_path) {
                if !url_path.ends_with('/') {
                    // Relative links in the listing need the trailing slash.
                    return redirect(&format!("{url_path}/"));
                }
                return html_response(&state, list_dir(&url_path, &dir).await);
            }
            match resolve_path(&state.root, "/index.html") {
                Some(index) if state.spa && !looks_like_asset(&url_path) => index,
                _ => return not_found(),
            }
        }
    };

    if content_type(&path).starts_with("text/html") {
        return match tokio::fs::read(&path).await {
            Ok(bytes) => html_response(&state, String::from_utf8_lossy(&bytes).into_owned()),
            Err(_) => not_found(),
        };
    }

    // Everything else streams from disk, with Range (media seeking), HEAD and If-Modified-Since.
    match ServeFile::new(&path).oneshot(request).await {
        Ok(response) => {
            let mut response = response.map(Body::new);
            let headers = response.headers_mut();
            headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
            headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type(&path)));
            response
        }
        Err(_) => not_found(),
    }
}

async fn list_dir(url_path: &str, dir: &Path) -> String {
    let mut entries = Vec::new();
    if let Ok(mut read) = tokio::fs::read_dir(dir).await {
        while let Ok(Some(entry)) = read.next_entry().await {
            let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
            entries.push((entry.file_name().to_string_lossy().into_owned(), is_dir));
        }
    }
    directory_listing(url_path, &entries)
}

fn html_response(state: &AppState, html: String) -> Response {
    let body = if state.reload { inject_reload_script(&html) } else { html };
    (StatusCode::OK, no_store_headers("text/html; charset=utf-8"), body).into_response()
}

fn redirect(location: &str) -> Response {
    let location = HeaderValue::from_str(location).unwrap_or_else(|_| HeaderValue::from_static("/"));
    (StatusCode::MOVED_PERMANENTLY, [(header::LOCATION, location)]).into_response()
}

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, no_store_headers("text/plain; charset=utf-8"), "not found").into_response()
}

fn no_store_headers(kind: &'static str) -> [(header::HeaderName, HeaderValue); 3] {
    [
        (header::CONTENT_TYPE, HeaderValue::from_static(kind)),
        (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        (header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*")),
    ]
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    let changes = state.changes.subscribe();
    ws.on_upgrade(move |socket| client(socket, changes))
}

async fn client(mut socket: WebSocket, mut changes: broadcast::Receiver<Change>) {
    loop {
        tokio::select! {
            change = changes.recv() => match change {
                Ok(change) => {
                    if socket.send(Message::Text(change.as_str().into())).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => break,
            },
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
        }
    }
}

/// Watches `root`, batching events for DEBOUNCE before announcing one change.
fn watch(cfg: &Config, changes: broadcast::Sender<Change>) -> notify::Result<notify::RecommendedWatcher> {
    let (tx, mut rx) = mpsc::unbounded_channel::<Vec<PathBuf>>();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if let Ok(event) = event {
            if !matches!(event.kind, notify::EventKind::Access(_)) {
                let _ = tx.send(event.paths);
            }
        }
    })?;
    watcher.watch(&cfg.root, RecursiveMode::Recursive)?;

    let root = cfg.root.clone();
    let ignore = cfg.ignore.clone();
    tokio::spawn(async move {
        while let Some(first) = rx.recv().await {
            let mut batch = first;
            let deadline = tokio::time::sleep(DEBOUNCE);
            tokio::pin!(deadline);
            loop {
                tokio::select! {
                    more = rx.recv() => match more {
                        Some(paths) => batch.extend(paths),
                        None => break,
                    },
                    _ = &mut deadline => break,
                }
            }
            if let Some(change) = classify(&batch, &root, &ignore) {
                let _ = changes.send(change);
            }
        }
    });
    Ok(watcher)
}

/// Serves on `listener` until an error or the process stops.
pub async fn serve(listener: TcpListener, cfg: Config) -> std::io::Result<()> {
    let (changes, _) = broadcast::channel(16);
    let _watcher = if cfg.reload {
        Some(watch(&cfg, changes.clone()).map_err(std::io::Error::other)?)
    } else {
        None
    };
    let state = AppState {
        root: Arc::new(cfg.root.clone()),
        changes,
        spa: cfg.spa,
        reload: cfg.reload,
    };
    axum::serve(listener, router(state)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("live-serve-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("index.html"), "<html></html>").unwrap();
        std::fs::write(dir.join("sub").join("index.html"), "<p>sub</p>").unwrap();
        std::fs::write(dir.join("a b.js"), "x").unwrap();
        dir
    }

    #[test]
    fn injects_before_closing_body_case_insensitive() {
        let out = inject_reload_script("<html><BODY><p>x</p></BODY></html>");
        let script_at = out.find("/__live/ws").expect("script injected");
        let body_close = out.find("</BODY>").unwrap();
        assert!(script_at < body_close);
        assert_eq!(out.matches("<script").count(), 1);
    }

    #[test]
    fn injects_at_end_without_body() {
        let out = inject_reload_script("<p>fragment</p>");
        assert!(out.starts_with("<p>fragment</p>"));
        assert!(out.contains("/__live/ws"));
    }

    #[test]
    fn resolves_files_dirs_and_encoded_names() {
        let root = temp_root();
        assert_eq!(resolve_path(&root, "/"), Some(root.join("index.html")));
        assert_eq!(resolve_path(&root, "/sub/"), Some(root.join("sub").join("index.html")));
        assert_eq!(resolve_path(&root, "/sub"), Some(root.join("sub").join("index.html")));
        assert_eq!(resolve_path(&root, "/a%20b.js"), Some(root.join("a b.js")));
        assert_eq!(resolve_path(&root, "/index.html?v=3"), Some(root.join("index.html")));
    }

    #[test]
    fn refuses_paths_outside_root_and_missing_files() {
        let root = temp_root();
        assert_eq!(resolve_path(&root, "/../secret.txt"), None);
        assert_eq!(resolve_path(&root, "/sub/../../secret.txt"), None);
        assert_eq!(resolve_path(&root, "/%2e%2e/secret.txt"), None);
        assert_eq!(resolve_path(&root, "/..%5csecret.txt"), None);
        assert_eq!(resolve_path(&root, "/C:/Windows/win.ini"), None);
        assert_eq!(resolve_path(&root, "/missing.html"), None);
    }

    #[test]
    fn content_types() {
        assert_eq!(content_type(Path::new("a.html")), "text/html; charset=utf-8");
        assert_eq!(content_type(Path::new("a.JS")), "text/javascript; charset=utf-8");
        assert_eq!(content_type(Path::new("a.mjs")), "text/javascript; charset=utf-8");
        assert_eq!(content_type(Path::new("a.css")), "text/css; charset=utf-8");
        assert_eq!(content_type(Path::new("a.svg")), "image/svg+xml");
        assert_eq!(content_type(Path::new("a.woff2")), "font/woff2");
        assert_eq!(content_type(Path::new("a.unknown")), "application/octet-stream");
    }

    #[test]
    fn display_urls() {
        assert_eq!(display_url("127.0.0.1", 5500), "http://127.0.0.1:5500/");
        assert_eq!(display_url("0.0.0.0", 8080), "http://localhost:8080/");
        assert_eq!(display_url("::", 8080), "http://localhost:8080/");
        assert_eq!(display_url("::1", 5500), "http://[::1]:5500/");
        assert_eq!(display_url("localhost", 3000), "http://localhost:3000/");
    }

    #[test]
    fn lists_directories_safely() {
        let html = directory_listing(
            "/docs/",
            &[("b.txt".into(), false), ("<x>.html".into(), false), ("sub".into(), true)],
        );
        assert!(html.contains("Index of /docs/"));
        assert!(html.contains(r#"href="../""#), "link to parent");
        assert!(html.contains(r#"href="sub/""#));
        assert!(html.contains("&lt;x&gt;.html"), "names are escaped");
        assert!(!html.contains("<x>"));
        assert!(html.find("sub/").unwrap() < html.find("b.txt").unwrap(), "dirs first");
        let root = directory_listing("/", &[]);
        assert!(!root.contains(r#"href="../""#), "no parent link at the root");
    }

    #[tokio::test]
    async fn binds_the_next_port_unless_strict() {
        let taken = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = taken.local_addr().unwrap().port();
        let next = bind("127.0.0.1", port, false).await.unwrap();
        assert_ne!(next.local_addr().unwrap().port(), port);
        assert!(bind("127.0.0.1", port, true).await.is_err());
    }

    #[test]
    fn ignores_vcs_deps_and_extra_dirs() {
        let root = Path::new("proj");
        let extra = vec!["dist".to_string()];
        assert!(is_ignored(&root.join("node_modules/x/y.js"), root, &extra));
        assert!(is_ignored(&root.join(".git/HEAD"), root, &extra));
        assert!(is_ignored(&root.join("dist/a.js"), root, &extra));
        assert!(!is_ignored(&root.join("out/script-build/a.js"), root, &extra));
        assert!(!is_ignored(&root.join("src/node_modules_like.ts"), root, &extra));
    }

    #[test]
    fn classifies_css_only_as_css_otherwise_reload() {
        let root = Path::new("proj");
        let css = vec![root.join("a.css"), root.join("b/c.CSS")];
        assert_eq!(classify(&css, root, &[]), Some(Change::Css));
        let mixed = vec![root.join("a.css"), root.join("a.js")];
        assert_eq!(classify(&mixed, root, &[]), Some(Change::Reload));
        let ignored = vec![root.join("node_modules/a.js"), root.join(".git/index")];
        assert_eq!(classify(&ignored, root, &[]), None);
        assert_eq!(classify(&[], root, &[]), None);
    }
}
