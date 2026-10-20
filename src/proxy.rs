//! `--proxy /api=http://localhost:3000`: requests under a path go to another local server (an API
//! backend), so a static frontend can call it without CORS. Like VS Code's Live Server and the npm
//! `live-server`, the prefix is replaced by the target's path: `/api/users` goes to
//! `http://localhost:3000/users`, or to `/api/users` with `--proxy /api=http://localhost:3000/api`.

use std::time::Duration;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;

/// A connection to a backend that does not answer within this long is a 502.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// One `--proxy` rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proxy {
    /// The path prefix, without a trailing slash ("/api"); "" for everything.
    pub prefix: String,
    /// Where it goes: `http://host:port` and an optional path, without a trailing slash.
    pub target: String,
}

impl Proxy {
    /// `/api=http://localhost:3000` → a rule; the message says what is wrong otherwise.
    pub fn parse(spec: &str) -> Result<Proxy, String> {
        let (prefix, target) = spec
            .split_once('=')
            .ok_or_else(|| format!("--proxy takes PATH=URL, e.g. /api=http://localhost:3000, not '{spec}'"))?;
        let (prefix, target) = (prefix.trim(), target.trim());
        if prefix.contains(['\\', ':']) {
            // Git Bash on Windows turns an argument that starts with "/" into a Windows path.
            return Err(format!(
                "the --proxy path '{prefix}' looks like a Windows path: Git Bash rewrites arguments that start \
with '/'; write the path without it (--proxy api=http://localhost:3000) or set MSYS_NO_PATHCONV=1"
            ));
        }
        // "api" is "/api": the leading slash is optional, and leaving it out keeps Git Bash's hands off.
        let prefix = format!("/{}", prefix.trim_start_matches('/'));
        let uri: Uri = target.parse().map_err(|_| format!("not a URL: '{target}'"))?;
        match uri.scheme_str() {
            Some("http") => {}
            Some("https") => {
                return Err(format!(
                    "--proxy forwards to http:// servers only, not '{target}' (a local backend rarely needs TLS)"
                ));
            }
            _ => return Err(format!("the --proxy target must be an http:// URL, not '{target}'")),
        }
        if uri.authority().is_none() || uri.query().is_some() {
            return Err(format!("the --proxy target must be http://host:port with an optional path, not '{target}'"));
        }
        Ok(Proxy {
            prefix: prefix.trim_end_matches('/').to_string(),
            target: target.trim_end_matches('/').to_string(),
        })
    }

    /// The rest of `path` after the prefix ("/users", or "" for the prefix itself), when the rule covers
    /// it. Whole segments only: `/api` covers `/api` and `/api/users`, not `/apix`.
    fn rest<'a>(&self, path: &'a str) -> Option<&'a str> {
        let rest = path.strip_prefix(&self.prefix)?;
        (rest.is_empty() || rest.starts_with('/')).then_some(rest)
    }
}

/// The rule for `path`: the one with the longest prefix that covers it.
pub fn route<'a>(proxies: &'a [Proxy], path: &str) -> Option<&'a Proxy> {
    proxies.iter().filter(|p| p.rest(path).is_some()).max_by_key(|p| p.prefix.len())
}

/// The URL `path?query` goes to under `proxy`.
fn upstream_url(proxy: &Proxy, path: &str, query: Option<&str>) -> String {
    let rest = proxy.rest(path).unwrap_or("");
    let mut url = format!("{}{rest}", proxy.target);
    if url.matches('/').count() < 3 {
        // http://host:port with nothing after it: the root.
        url.push('/');
    }
    if let Some(query) = query {
        url.push('?');
        url.push_str(query);
    }
    url
}

/// Headers about one hop, never passed on (RFC 9110 §7.6.1), plus any the request's Connection names.
fn drop_hop_by_hop(headers: &mut HeaderMap) {
    let named: Vec<HeaderName> = headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .filter_map(|name| HeaderName::try_from(name.trim()).ok())
        .collect();
    for name in named {
        headers.remove(name);
    }
    for name in [
        header::CONNECTION,
        header::PROXY_AUTHENTICATE,
        header::PROXY_AUTHORIZATION,
        header::TE,
        header::TRAILER,
        header::TRANSFER_ENCODING,
        header::UPGRADE,
    ] {
        headers.remove(name);
    }
    headers.remove("keep-alive");
    headers.remove("proxy-connection");
}

/// Forwards requests to backends; one per server, so connections are reused.
#[derive(Clone)]
pub(crate) struct Forwarder {
    client: Client<HttpConnector, Body>,
}

impl Forwarder {
    pub(crate) fn new() -> Forwarder {
        let mut connector = HttpConnector::new();
        connector.set_connect_timeout(Some(CONNECT_TIMEOUT));
        Forwarder { client: Client::builder(TokioExecutor::new()).build(connector) }
    }

    /// Sends `request` on to `proxy`'s target and passes the answer back untouched, but for one-hop
    /// headers and a redirect to the backend's own address, which points back under the prefix.
    pub(crate) async fn forward(&self, proxy: &Proxy, request: Request, https: bool) -> Response {
        let (mut parts, body) = request.into_parts();
        if parts.headers.contains_key(header::UPGRADE) {
            return text(StatusCode::NOT_IMPLEMENTED, "live-serve: --proxy does not carry WebSockets yet".into());
        }
        let url = upstream_url(proxy, parts.uri.path(), parts.uri.query());
        let Ok(uri) = url.parse::<Uri>() else {
            return text(StatusCode::BAD_REQUEST, format!("live-serve: cannot proxy to '{url}'"));
        };
        let original_host = parts.headers.get(header::HOST).cloned();
        let version = parts.version;
        drop_hop_by_hop(&mut parts.headers);
        if let Some(authority) = uri.authority().and_then(|a| HeaderValue::from_str(a.as_str()).ok()) {
            parts.headers.insert(header::HOST, authority);
        }
        if let Some(host) = original_host {
            parts.headers.insert("x-forwarded-host", host);
        }
        parts.headers.insert("x-forwarded-proto", HeaderValue::from_static(if https { "https" } else { "http" }));
        parts.uri = uri;

        match self.client.request(Request::from_parts(parts, body)).await {
            Ok(response) => {
                let (mut parts, body) = response.into_parts();
                // The browser's connection, not the backend's: an HTTP/1.0 backend (python -m http.server)
                // must not turn the answer into HTTP/1.0, which closes the browser's connection each time.
                parts.version = version;
                drop_hop_by_hop(&mut parts.headers);
                if let Some(location) = parts.headers.get(header::LOCATION).and_then(|l| l.to_str().ok())
                    && let Some(rest) = location.strip_prefix(&proxy.target)
                    && (rest.is_empty() || rest.starts_with(['/', '?']))
                {
                    let local = format!("{}{}", proxy.prefix, if rest.is_empty() { "/" } else { rest });
                    if let Ok(value) = HeaderValue::from_str(&local) {
                        parts.headers.insert(header::LOCATION, value);
                    }
                }
                Response::from_parts(parts, Body::new(body))
            }
            Err(e) => {
                let why = std::error::Error::source(&e).map_or_else(|| e.to_string(), |s| s.to_string());
                text(StatusCode::BAD_GATEWAY, format!("live-serve: cannot reach {} for {url}: {why}", proxy.target))
            }
        }
    }
}

fn text(status: StatusCode, message: String) -> Response {
    (status, [(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"))], message).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proxy(spec: &str) -> Proxy {
        Proxy::parse(spec).unwrap()
    }

    #[test]
    fn parses_path_equals_url() {
        assert_eq!(proxy("/api=http://localhost:3000"), Proxy { prefix: "/api".into(), target: "http://localhost:3000".into() });
        assert_eq!(proxy(" /api/ = http://127.0.0.1:8080/v1/ ").target, "http://127.0.0.1:8080/v1");
        assert_eq!(proxy("/api/=http://localhost:3000").prefix, "/api");
        assert_eq!(proxy("/=http://localhost:3000").prefix, "", "everything");
        assert_eq!(proxy("api=http://localhost:3000").prefix, "/api", "the leading slash is optional");
        assert_eq!(proxy("api/v2=http://localhost:3000").prefix, "/api/v2");
    }

    #[test]
    fn says_what_is_wrong_with_a_rule() {
        let error = |spec| Proxy::parse(spec).unwrap_err();
        assert!(error("/api").contains("PATH=URL"));
        // What Git Bash makes of "/api=…".
        assert!(error(r"C:\Program Files\Git\api=http://localhost:3000").contains("MSYS_NO_PATHCONV"));
        assert!(error("/api=localhost:3000").contains("http://"));
        assert!(error("/api=https://example.com").contains("http:// servers only"));
        assert!(error("/api=ftp://example.com").contains("http://"));
        assert!(error("/api=http://localhost:3000/?a=1").contains("optional path"));
    }

    #[test]
    fn covers_whole_segments_and_the_longest_prefix_wins() {
        let rules = [proxy("/api=http://a:1"), proxy("/api/admin=http://b:2")];
        assert_eq!(route(&rules, "/api").unwrap().target, "http://a:1");
        assert_eq!(route(&rules, "/api/users").unwrap().target, "http://a:1");
        assert_eq!(route(&rules, "/api/admin/x").unwrap().target, "http://b:2");
        assert_eq!(route(&rules, "/api/administrators").unwrap().target, "http://a:1");
        assert!(route(&rules, "/apix").is_none());
        assert!(route(&rules, "/").is_none());
        assert!(route(&[proxy("/=http://c:3")], "/anything").is_some());
    }

    #[test]
    fn the_prefix_is_replaced_by_the_targets_path() {
        let root = proxy("/api=http://localhost:3000");
        assert_eq!(upstream_url(&root, "/api/users", Some("x=1")), "http://localhost:3000/users?x=1");
        assert_eq!(upstream_url(&root, "/api", None), "http://localhost:3000/");
        assert_eq!(upstream_url(&root, "/api/", None), "http://localhost:3000/");
        let kept = proxy("/api=http://localhost:3000/api");
        assert_eq!(upstream_url(&kept, "/api/users", None), "http://localhost:3000/api/users");
        assert_eq!(upstream_url(&kept, "/api", None), "http://localhost:3000/api");
    }

    #[test]
    fn one_hop_headers_stay_behind() {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONNECTION, HeaderValue::from_static("keep-alive, x-hop"));
        headers.insert("x-hop", HeaderValue::from_static("1"));
        headers.insert("keep-alive", HeaderValue::from_static("timeout=5"));
        headers.insert(header::TRANSFER_ENCODING, HeaderValue::from_static("chunked"));
        headers.insert(header::ACCEPT, HeaderValue::from_static("application/json"));
        drop_hop_by_hop(&mut headers);
        assert_eq!(headers.len(), 1, "{headers:?}");
        assert!(headers.contains_key(header::ACCEPT));
    }
}
