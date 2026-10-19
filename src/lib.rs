//! live-serve: a static dev server that reloads the page when files change (a Live Server replacement).
//!
//! - Serves the files under `root`, always with `Cache-Control: no-store`, so a fresh build is never cached.
//! - Injects a small script into every HTML page that opens a WebSocket to `/__live/ws`.
//! - Watches `root`: on a change it tells browsers "reload" (whole page) or "css" (swap stylesheets only).
//! - Answers only host names that point at this machine on purpose, hides dotfiles, and sends no CORS
//!   header unless asked, so other websites cannot read the files.

pub mod init;
pub mod listing;

pub use listing::{Entry, directory_listing};

use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use notify::{RecursiveMode, Watcher};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc};
use tokio::time::Instant;
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

/// A change is announced once events stop for this long (saving a file raises several events)...
const DEBOUNCE: Duration = Duration::from_millis(120);

/// ...or at the latest this long after the first event, so a long build still reloads now and then.
const MAX_WAIT: Duration = Duration::from_secs(2);

const RELOAD_SCRIPT: &str = r#"<script>(()=>{const u=(location.protocol==="https:"?"wss://":"ws://")+location.host+"/__live/ws";const c=()=>{const w=new WebSocket(u);w.onmessage=e=>{if(e.data==="css"){for(const l of document.querySelectorAll('link[rel="stylesheet"]')){const h=new URL(l.href);h.searchParams.set("_live",Date.now());l.href=h.href}}else{location.reload()}};w.onclose=()=>setTimeout(c,1000)};c()})();</script>"#;

/// Inserts the reload script before the last `</body>` (any case), or at the end when there is none.
/// Works on bytes, so pages in any ASCII-compatible encoding come through unchanged.
pub fn inject_reload_script(html: &[u8]) -> Vec<u8> {
    let at = html
        .windows(7)
        .rposition(|w| w.eq_ignore_ascii_case(b"</body>"))
        .unwrap_or(html.len());
    let mut out = Vec::with_capacity(html.len() + RELOAD_SCRIPT.len());
    out.extend_from_slice(&html[..at]);
    out.extend_from_slice(RELOAD_SCRIPT.as_bytes());
    out.extend_from_slice(&html[at..]);
    out
}

/// What a URL path names under the served folder.
#[derive(Debug, PartialEq, Eq)]
pub enum Target {
    /// A file to serve (a directory's `index.html` included).
    File(PathBuf),
    /// A directory without `index.html`, shown as a listing.
    Dir(PathBuf),
}

/// URL path → what it names under `real_root`, which must be canonical (see `Path::canonicalize`).
/// None when it leaves the root (`..`, `\`, drive letters, symlinks pointing outside), is hidden
/// (a name starting with "." other than `.well-known`, unless `dotfiles`), or does not exist.
/// Costs one canonicalize and one stat; a directory adds one more of each for its `index.html`.
pub fn locate(real_root: &Path, url_path: &str, dotfiles: bool) -> Option<Target> {
    let path_only = url_path.split(['?', '#']).next().unwrap_or("");
    let decoded = percent_encoding::percent_decode_str(path_only).decode_utf8().ok()?;
    // "\" and drive letters ("C:") have no place in a URL path; refuse them so nothing escapes root on Windows.
    if decoded.contains(['\\', ':', '\0']) {
        return None;
    }

    let mut target = real_root.to_path_buf();
    for segment in decoded.split('/') {
        match segment {
            "" | "." => {}
            ".." => return None,
            s => target.push(s),
        }
    }
    let real = visible(real_root, &target, dotfiles)?;
    let meta = std::fs::metadata(&real).ok()?;
    if meta.is_file() {
        return Some(Target::File(real));
    }
    if !meta.is_dir() {
        return None;
    }
    match visible(real_root, &real.join("index.html"), dotfiles) {
        Some(index) if index.is_file() => Some(Target::File(index)),
        _ => Some(Target::Dir(real)),
    }
}

/// `path` with symlinks (and on Windows, 8.3 short names) resolved, if it exists, stays inside
/// `real_root` and is not hidden. Checking the resolved path means `GIT~1` or a link cannot reach `.git`.
fn visible(real_root: &Path, path: &Path, dotfiles: bool) -> Option<PathBuf> {
    let real = path.canonicalize().ok()?;
    let inner = real.strip_prefix(real_root).ok()?;
    (dotfiles || !inner.components().any(|c| is_hidden(c.as_os_str()))).then_some(real)
}

/// A name starting with "." (`.env`, `.git`), except `.well-known`.
fn is_hidden(name: &OsStr) -> bool {
    let name = name.to_string_lossy();
    name.starts_with('.') && name != ".well-known"
}

/// Whether the last path segment looks like a file ("app.js"), not a route ("settings").
fn looks_like_asset(url_path: &str) -> bool {
    let last = url_path.split(['?', '#']).next().unwrap_or("").rsplit('/').next().unwrap_or("");
    last.contains('.')
}

/// Content-Type live-serve sets itself for a file extension (with a charset for text);
/// None leaves it to the file server's own guess.
pub fn content_type(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
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
        _ => return None,
    })
}

fn is_html(path: &Path) -> bool {
    content_type(path).is_some_and(|t| t.starts_with("text/html"))
}

/// Whether a change to this file is not worth a reload: it is inside an ignored directory
/// (`node_modules`, `.git`, or one in `extra`), or it is an editor's temporary file.
pub fn is_ignored(path: &Path, root: &Path, extra: &[String]) -> bool {
    let relative = path.strip_prefix(root).unwrap_or(path);
    let in_ignored_dir = relative.components().any(|c| match c {
        Component::Normal(name) => {
            let name = name.to_string_lossy();
            ALWAYS_IGNORED.contains(&name.as_ref()) || extra.iter().any(|e| e == name.as_ref())
        }
        _ => false,
    });
    in_ignored_dir || path.file_name().is_some_and(|n| is_editor_temp(&n.to_string_lossy()))
}

/// Files editors write around a save: backups (`a.css~`), Vim swap files and its `4913` probe,
/// JetBrains safe-write copies, Emacs locks (`.#a.css`), Chrome's `.crswap` and `*.tmp`.
fn is_editor_temp(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name == "4913"
        || name.starts_with(".#")
        || name.ends_with('~')
        || [".swp", ".swx", ".swo", ".tmp", ".crswap", "___jb_tmp___", "___jb_old___"]
            .iter()
            .any(|suffix| name.ends_with(suffix))
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

/// The host name in a `Host` header ("localhost:5500", "[::1]:5500"), without port or brackets, lowercased.
fn host_name(authority: &str) -> String {
    let authority = authority.trim();
    let name = match authority.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or(""),
        None => authority.rsplit_once(':').map_or(authority, |(name, _)| name),
    };
    name.trim_end_matches('.').to_ascii_lowercase()
}

/// Whether to answer a request for this `Host`: IP addresses, `localhost`, `*.localhost`, and the names
/// in `allowed` ("*" allows any). Any other name is likely DNS rebinding: an outside website pointing its
/// own name at this machine to read the files.
pub fn host_allowed(authority: &str, allowed: &[String]) -> bool {
    let name = host_name(authority);
    name == "localhost"
        || name.ends_with(".localhost")
        || name.parse::<std::net::IpAddr>().is_ok()
        || allowed.iter().any(|a| a == "*" || a.eq_ignore_ascii_case(&name))
}

/// Whether a page with this `Origin` ("http://localhost:5500") may open the reload WebSocket.
pub fn origin_allowed(origin: &str, allowed: &[String]) -> bool {
    origin
        .split_once("://")
        .is_some_and(|(_, rest)| host_allowed(rest.split('/').next().unwrap_or(""), allowed))
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
    /// Send `Access-Control-Allow-Origin: *`, so pages on other origins can read the files.
    pub cors: bool,
    /// Serve names starting with "." (`.env`, `.git`). `.well-known` is always served.
    pub dotfiles: bool,
    /// Host names answered besides IP addresses, `localhost` and `*.localhost` ("*" for any).
    pub allowed_hosts: Vec<String>,
}

impl Config {
    /// Serve `root` with the defaults: reload on, no SPA fallback, nothing extra ignored,
    /// no CORS, dotfiles hidden, only local host names.
    pub fn new(root: PathBuf) -> Config {
        Config {
            root,
            ignore: vec![],
            spa: false,
            reload: true,
            cors: false,
            dotfiles: false,
            allowed_hosts: vec![],
        }
    }
}

/// `path` without Windows' verbatim prefix, as `canonicalize` returns it:
/// `\\?\C:\x` → `C:\x`, `\\?\UNC\server\share\x` → `\\server\share\x`. Other paths are unchanged.
pub fn strip_verbatim(path: &Path) -> PathBuf {
    match path.to_str() {
        Some(s) if s.starts_with(r"\\?\UNC\") => PathBuf::from(format!(r"\\{}", &s[8..])),
        Some(s) if s.starts_with(r"\\?\") => PathBuf::from(&s[4..]),
        _ => path.to_path_buf(),
    }
}

/// Files up to this size, of a known type and without a Range request, are read in one trip to the
/// blocking pool into a buffer of their exact size. Streaming a 5 MB bundle in tower-http's 64 KB
/// steps took ~80 trips (17 ms); in one read it is about as fast as Node's live-server.
const WHOLE_FILE_MAX: u64 = 16 * 1024 * 1024;

/// Bytes read per step when streaming bigger files or byte ranges (media seeking).
const FILE_CHUNK: usize = 256 * 1024;

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

#[derive(Clone)]
struct AppState {
    /// The served folder, canonical, so `locate` needs no canonicalize of its own for it.
    real_root: Arc<PathBuf>,
    changes: broadcast::Sender<Change>,
    spa: bool,
    reload: bool,
    cors: bool,
    dotfiles: bool,
    allowed_hosts: Arc<Vec<String>>,
}

/// Routes: `/__live/ws` is the WebSocket (when reloading), everything else a static file;
/// every response passes through `guard`.
fn router(state: AppState) -> Router {
    let routes = if state.reload {
        Router::new().route("/__live/ws", get(ws_handler))
    } else {
        Router::new()
    };
    routes
        .fallback(file_handler)
        .layer(middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state)
}

/// Refuses unknown host names, and gives every response `no-store` (plus CORS when asked).
async fn guard(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .or_else(|| request.uri().authority().map(|a| a.as_str()));
    // Browsers always send a Host; a request without one is not from a web page.
    let host_ok = host.is_none_or(|h| host_allowed(h, &state.allowed_hosts));

    let mut response = if host_ok { next.run(request).await } else { forbidden() };
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if state.cors {
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    }
    response
}

/// `locate` on the blocking pool: it makes file system calls.
async fn locate_blocking(state: &AppState, url_path: &str) -> Option<Target> {
    let (root, url_path, dotfiles) = (state.real_root.clone(), url_path.to_string(), state.dotfiles);
    tokio::task::spawn_blocking(move || locate(&root, &url_path, dotfiles))
        .await
        .ok()
        .flatten()
}

async fn file_handler(State(state): State<AppState>, request: Request) -> Response {
    let url_path = request.uri().path().to_string();

    let path = match locate_blocking(&state, &url_path).await {
        Some(Target::File(path)) => path,
        Some(Target::Dir(dir)) => {
            if !url_path.ends_with('/') {
                // Relative links in the listing need the trailing slash. The redirect is relative too,
                // so "//name" cannot turn into a link to another host.
                let last = url_path.rsplit('/').next().unwrap_or("");
                return redirect(&format!("./{last}/"));
            }
            return html_response(&state, list_dir(&url_path, &dir, state.dotfiles).await.into_bytes());
        }
        None if state.spa && !looks_like_asset(&url_path) => match locate_blocking(&state, "/index.html").await {
            Some(Target::File(index)) => index,
            _ => return not_found(),
        },
        None => return not_found(),
    };

    if is_html(&path) {
        return match tokio::fs::read(&path).await {
            Ok(bytes) => html_response(&state, bytes),
            Err(_) => not_found(),
        };
    }

    if let Some(kind) = content_type(&path)
        && !request.headers().contains_key(header::RANGE)
        && let Some(bytes) = read_whole(&path).await
    {
        return (StatusCode::OK, [(header::CONTENT_TYPE, HeaderValue::from_static(kind))], bytes).into_response();
    }

    // Big files, ranges (media seeking) and unknown types stream from disk via tower-http.
    match ServeFile::new(&path).with_buf_chunk_size(FILE_CHUNK).oneshot(request).await {
        Ok(response) => {
            let mut response = response.map(Body::new);
            if let Some(kind) = content_type(&path) {
                response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static(kind));
            }
            response
        }
        Err(_) => not_found(),
    }
}

/// The whole file in one blocking trip, if it is at most WHOLE_FILE_MAX bytes.
async fn read_whole(path: &Path) -> Option<Vec<u8>> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        use std::io::Read;
        let mut file = std::fs::File::open(&path).ok()?;
        let len = file.metadata().ok()?.len();
        if len > WHOLE_FILE_MAX {
            return None;
        }
        let mut bytes = Vec::with_capacity(len as usize);
        file.read_to_end(&mut bytes).ok()?;
        Some(bytes)
    })
    .await
    .ok()
    .flatten()
}

/// Reads and renders a listing in one go on the blocking pool: one task for the whole folder
/// instead of one per entry. On Windows the size and date come with the directory read itself.
async fn list_dir(url_path: &str, dir: &Path, dotfiles: bool) -> String {
    let (url_path, dir) = (url_path.to_string(), dir.to_path_buf());
    tokio::task::spawn_blocking(move || directory_listing(&url_path, &read_entries(&dir, dotfiles)))
        .await
        .unwrap_or_default()
}

fn read_entries(dir: &Path, dotfiles: bool) -> Vec<Entry> {
    let Ok(read) = std::fs::read_dir(dir) else { return vec![] };
    read.filter_map(Result::ok)
        .filter(|entry| dotfiles || !is_hidden(&entry.file_name()))
        .map(|entry| {
            let meta = entry.metadata().ok();
            Entry {
                name: entry.file_name().to_string_lossy().into_owned(),
                is_dir: meta.as_ref().is_some_and(|m| m.is_dir()),
                size: meta.as_ref().map_or(0, |m| m.len()),
                modified: meta
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs()),
            }
        })
        .collect()
}

fn html_response(state: &AppState, html: Vec<u8>) -> Response {
    // Claim UTF-8 only when it is; otherwise the browser goes by the page's own <meta charset>.
    let kind = if std::str::from_utf8(&html).is_ok() { "text/html; charset=utf-8" } else { "text/html" };
    let body = if state.reload { inject_reload_script(&html) } else { html };
    (StatusCode::OK, [(header::CONTENT_TYPE, HeaderValue::from_static(kind))], body).into_response()
}

fn redirect(location: &str) -> Response {
    let location = HeaderValue::from_str(location).unwrap_or_else(|_| HeaderValue::from_static("/"));
    (StatusCode::MOVED_PERMANENTLY, [(header::LOCATION, location)]).into_response()
}

fn not_found() -> Response {
    text_response(StatusCode::NOT_FOUND, "not found")
}

fn forbidden() -> Response {
    text_response(
        StatusCode::FORBIDDEN,
        "live-serve: this host name is not allowed; start live-serve with --allow-host <name> to use it",
    )
}

fn text_response(status: StatusCode, text: &'static str) -> Response {
    (status, [(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"))], text).into_response()
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>, headers: HeaderMap) -> Response {
    // Pages on other websites may not listen in.
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    if origin.is_some_and(|o| !origin_allowed(o, &state.allowed_hosts)) {
        return forbidden();
    }
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

/// Watches `root` and announces one change per burst of events: once they stop for DEBOUNCE,
/// or MAX_WAIT after the first. Ignored files are dropped right away, so they neither start
/// nor stretch a burst.
fn watch(cfg: &Config, changes: broadcast::Sender<Change>) -> notify::Result<notify::RecommendedWatcher> {
    let (tx, mut rx) = mpsc::unbounded_channel::<Vec<PathBuf>>();
    let (root, ignore) = (cfg.root.clone(), cfg.ignore.clone());
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let Ok(event) = event else { return };
        if matches!(event.kind, notify::EventKind::Access(_)) {
            return;
        }
        let paths: Vec<PathBuf> = event.paths.into_iter().filter(|p| !is_ignored(p, &root, &ignore)).collect();
        if !paths.is_empty() {
            let _ = tx.send(paths);
        }
    })?;
    watcher.watch(&cfg.root, RecursiveMode::Recursive)?;

    let root = cfg.root.clone();
    let ignore = cfg.ignore.clone();
    tokio::spawn(async move {
        while let Some(first) = rx.recv().await {
            let mut batch = first;
            let give_up = Instant::now() + MAX_WAIT;
            while Instant::now() < give_up {
                let quiet = (Instant::now() + DEBOUNCE).min(give_up);
                match tokio::time::timeout_at(quiet, rx.recv()).await {
                    Ok(Some(paths)) => batch.extend(paths),
                    Ok(None) | Err(_) => break,
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
        real_root: Arc::new(cfg.root.canonicalize()?),
        changes,
        spa: cfg.spa,
        reload: cfg.reload,
        cors: cfg.cors,
        dotfiles: cfg.dotfiles,
        allowed_hosts: Arc::new(cfg.allowed_hosts),
    };
    axum::serve(listener, router(state)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A canonical temp folder: index.html, sub/index.html, docs/ (no index), "a b.js" and dotfiles.
    fn temp_root() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("live-serve-test-{}", std::process::id()));
        for sub in ["sub", "docs", ".git", ".well-known"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        std::fs::write(dir.join("index.html"), "<html></html>").unwrap();
        std::fs::write(dir.join("sub").join("index.html"), "<p>sub</p>").unwrap();
        std::fs::write(dir.join("a b.js"), "x").unwrap();
        std::fs::write(dir.join(".env"), "SECRET=1").unwrap();
        std::fs::write(dir.join(".git").join("config"), "[core]").unwrap();
        std::fs::write(dir.join(".well-known").join("security.txt"), "x").unwrap();
        dir.canonicalize().unwrap()
    }

    fn file(path: PathBuf) -> Option<Target> {
        Some(Target::File(path))
    }

    #[test]
    fn injects_before_closing_body_case_insensitive() {
        let out = String::from_utf8(inject_reload_script(b"<html><BODY><p>x</p></BODY></html>")).unwrap();
        let script_at = out.find("/__live/ws").expect("script injected");
        let body_close = out.find("</BODY>").unwrap();
        assert!(script_at < body_close);
        assert_eq!(out.matches("<script").count(), 1);
    }

    #[test]
    fn injects_at_end_without_body() {
        let out = String::from_utf8(inject_reload_script(b"<p>fragment</p>")).unwrap();
        assert!(out.starts_with("<p>fragment</p>"));
        assert!(out.contains("/__live/ws"));
    }

    #[test]
    fn injection_keeps_bytes_that_are_not_utf8() {
        let latin1 = b"<html><body>caf\xe9</body></html>";
        let out = inject_reload_script(latin1);
        assert!(out.starts_with(b"<html><body>caf\xe9<script>"), "the \\xe9 byte is untouched");
        assert!(out.ends_with(b"</script></body></html>"));
    }

    #[test]
    fn locates_files_dirs_and_encoded_names() {
        let root = temp_root();
        assert_eq!(locate(&root, "/", false), file(root.join("index.html")));
        assert_eq!(locate(&root, "/sub/", false), file(root.join("sub").join("index.html")));
        assert_eq!(locate(&root, "/sub", false), file(root.join("sub").join("index.html")));
        assert_eq!(locate(&root, "/a%20b.js", false), file(root.join("a b.js")));
        assert_eq!(locate(&root, "/index.html?v=3", false), file(root.join("index.html")));
        assert_eq!(locate(&root, "/docs/", false), Some(Target::Dir(root.join("docs"))));
    }

    #[test]
    fn refuses_paths_outside_root_and_missing_files() {
        let root = temp_root();
        assert_eq!(locate(&root, "/../secret.txt", false), None);
        assert_eq!(locate(&root, "/sub/../../secret.txt", false), None);
        assert_eq!(locate(&root, "/%2e%2e/secret.txt", false), None);
        assert_eq!(locate(&root, "/..%5csecret.txt", false), None);
        assert_eq!(locate(&root, "/C:/Windows/win.ini", false), None);
        assert_eq!(locate(&root, "/missing.html", false), None);
    }

    #[test]
    fn hides_dotfiles_unless_asked() {
        let root = temp_root();
        assert_eq!(locate(&root, "/.env", false), None);
        assert_eq!(locate(&root, "/%2eenv", false), None);
        assert_eq!(locate(&root, "/.git/config", false), None);
        assert_eq!(locate(&root, "/.git/", false), None);
        assert_eq!(
            locate(&root, "/.well-known/security.txt", false),
            file(root.join(".well-known").join("security.txt"))
        );
        assert_eq!(locate(&root, "/.env", true), file(root.join(".env")));
    }

    #[test]
    fn content_types() {
        assert_eq!(content_type(Path::new("a.html")), Some("text/html; charset=utf-8"));
        assert_eq!(content_type(Path::new("a.JS")), Some("text/javascript; charset=utf-8"));
        assert_eq!(content_type(Path::new("a.mjs")), Some("text/javascript; charset=utf-8"));
        assert_eq!(content_type(Path::new("a.css")), Some("text/css; charset=utf-8"));
        assert_eq!(content_type(Path::new("a.svg")), Some("image/svg+xml"));
        assert_eq!(content_type(Path::new("a.woff2")), Some("font/woff2"));
        assert_eq!(content_type(Path::new("a.xml")), None, "left to the file server's guess");
        assert_eq!(content_type(Path::new("README")), None);
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
    fn allows_local_hosts_only() {
        let none: &[String] = &[];
        for ok in ["localhost:5500", "LOCALHOST", "localhost.", "127.0.0.1:5500", "[::1]:5500", "192.168.1.20:5500", "app.localhost:5500"] {
            assert!(host_allowed(ok, none), "{ok}");
        }
        for bad in ["evil.example:5500", "localhost.evil.example", "example.com", ""] {
            assert!(!host_allowed(bad, none), "{bad}");
        }
        let extra = vec!["myapp.test".to_string()];
        assert!(host_allowed("MyApp.test:5500", &extra));
        assert!(!host_allowed("other.test", &extra));
        assert!(host_allowed("anything.example", &["*".to_string()]));
    }

    #[test]
    fn allows_local_origins_only() {
        let none: &[String] = &[];
        assert!(origin_allowed("http://localhost:5500", none));
        assert!(origin_allowed("http://127.0.0.1:5500", none));
        assert!(origin_allowed("http://[::1]:5500", none));
        assert!(!origin_allowed("https://evil.example", none));
        assert!(!origin_allowed("null", none));
    }

    #[test]
    fn strips_verbatim_prefixes() {
        assert_eq!(strip_verbatim(Path::new(r"\\?\C:\site")), PathBuf::from(r"C:\site"));
        assert_eq!(strip_verbatim(Path::new(r"\\?\UNC\server\share\site")), PathBuf::from(r"\\server\share\site"));
        assert_eq!(strip_verbatim(Path::new("/home/me/site")), PathBuf::from("/home/me/site"));
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
    fn ignores_editor_temp_files() {
        let root = Path::new("proj");
        for temp in ["a.css~", ".a.css.swp", "4913", "a.css___jb_tmp___", "a.css___jb_old___", ".#a.css", "a.TMP", "a.css.crswap"] {
            assert!(is_ignored(&root.join(temp), root, &[]), "{temp}");
        }
        for real in ["a.css", "swp.js", "temp.html", "49130.txt"] {
            assert!(!is_ignored(&root.join(real), root, &[]), "{real}");
        }
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

    #[test]
    fn an_editor_save_of_css_is_still_css() {
        let root = Path::new("proj");
        let vim_save = vec![root.join("4913"), root.join("a.css~"), root.join("a.css")];
        assert_eq!(classify(&vim_save, root, &[]), Some(Change::Css));
    }
}
