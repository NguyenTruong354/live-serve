//! live-serve [DIR] [--port N] [--host H] [--ignore a,b] [--allow-host a,b] [--cors] [--dotfiles] [--poll]
//!            [--https | --cert FILE --key FILE] [--proxy PATH=URL]... [--open[=PAGE]]
//! live-serve init <vscode|zed> [DIR] [--port N]

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use live_serve::init::{Editor, InitOutcome, init, tasks_json, tasks_path};
use live_serve::proxy::Proxy;
use live_serve::{Config, bind, display_url, lan_address, page_url, serve, strip_verbatim, tls};

const HELP: &str = "live-serve: a static dev server that reloads the page when files change

Usage:
  live-serve [DIR] [options]            serve DIR (default: current directory)
  live-serve init <vscode|zed> [DIR]    add a \"live-serve\" task to the editor's project tasks

Options:
  -p, --port N      port (default 5500); if taken, the next free one is used
      --strict-port fail instead of moving to another port
      --host H      address to bind (default 127.0.0.1; 0.0.0.0 for your network)
      --ignore a,b  directory names not to watch, besides node_modules and .git
      --spa         serve /index.html for unknown routes (single-page apps)
      --allow-host a,b  host names to answer besides IP addresses and localhost
                    (\"*\" for any), e.g. a hosts-file name or a tunnel's domain
      --cors        send Access-Control-Allow-Origin: *, so other sites can read the files
      --dotfiles    serve names starting with \".\" (.env, .git); hidden by default
      --no-reload   plain static server: no injected script, no file watching
      --poll        scan for changes every 300 ms (skipping ignored folders) instead of
                    using file system events, for WSL, Docker volumes and network drives
                    where events do not arrive; reloads take about half a second
      --https       serve over HTTPS with a self-signed certificate, made and kept for you
                    (the browser warns once; for one it trusts, use --cert and --key)
      --cert FILE   serve over HTTPS with this certificate (PEM), e.g. made by mkcert
      --key FILE    and its private key (unencrypted PEM)
      --proxy PATH=URL
                    send requests under PATH to another local server, PATH replaced by
                    URL's path: --proxy /api=http://localhost:3000 sends /api/users to
                    http://localhost:3000/users; repeat it for more
      --open        open the browser once the server is up
      --open=PAGE   the same, at PAGE (e.g. --open=docs/ or --open=about.html)
      --exit-on-stdin-close
                    stop when standard input closes: for editors and tools that start
                    live-serve, so it never outlives them
  -V, --version     print the version
  -h, --help        print this help";

struct ServeArgs {
    root: PathBuf,
    host: String,
    port: u16,
    strict_port: bool,
    ignore: Vec<String>,
    spa: bool,
    reload: bool,
    cors: bool,
    dotfiles: bool,
    allow_host: Vec<String>,
    poll: bool,
    /// The page to open in the browser ("" for the root), if any.
    open: Option<String>,
    exit_on_stdin_close: bool,
    https: Https,
    proxies: Vec<Proxy>,
}

/// How to serve HTTPS, if at all.
#[derive(Debug, PartialEq)]
enum Https {
    Off,
    /// `--https`: a self-signed certificate, made and kept in the cache folder.
    SelfSigned,
    /// `--cert` and `--key`.
    Files { cert: PathBuf, key: PathBuf },
}

enum Command {
    Help,
    Version,
    Serve(ServeArgs),
    Init { editor: Editor, root: PathBuf, port: u16 },
}

fn parse(args: Vec<String>) -> Result<Command, String> {
    let mut iter = args.into_iter().peekable();
    let init_editor = if iter.peek().map(String::as_str) == Some("init") {
        iter.next();
        let name = iter.next().ok_or("init needs an editor: vscode or zed")?;
        Some(Editor::parse(&name).ok_or(format!("unknown editor: {name} (use vscode or zed)"))?)
    } else {
        None
    };

    let mut serve = ServeArgs {
        root: std::env::current_dir().map_err(|e| e.to_string())?,
        host: "127.0.0.1".into(),
        port: 5500,
        strict_port: false,
        ignore: vec![],
        spa: false,
        reload: true,
        cors: false,
        dotfiles: false,
        allow_host: vec![],
        poll: false,
        open: None,
        exit_on_stdin_close: false,
        https: Https::Off,
        proxies: vec![],
    };
    let (mut cert, mut key, mut self_signed) = (None, None, false);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(Command::Help),
            "-V" | "--version" => return Ok(Command::Version),
            "-p" | "--port" => {
                let v = iter.next().ok_or("--port needs a number")?;
                serve.port = v.parse().map_err(|_| format!("invalid port: {v}"))?;
            }
            "--host" => serve.host = iter.next().ok_or("--host needs an address")?,
            "--ignore" => {
                let v = iter.next().ok_or("--ignore needs a list")?;
                serve.ignore = list(&v);
            }
            "--allow-host" => {
                let v = iter.next().ok_or("--allow-host needs a list")?;
                serve.allow_host = list(&v);
            }
            "--strict-port" => serve.strict_port = true,
            "--cors" => serve.cors = true,
            "--dotfiles" => serve.dotfiles = true,
            "--spa" => serve.spa = true,
            "--no-reload" => serve.reload = false,
            "--poll" => serve.poll = true,
            "--open" => serve.open = Some(String::new()),
            // Only with "=": a bare word after --open stays the folder to serve.
            s if s.starts_with("--open=") => serve.open = Some(s["--open=".len()..].to_string()),
            "--exit-on-stdin-close" => serve.exit_on_stdin_close = true,
            "--https" => self_signed = true,
            "--proxy" => serve.proxies.push(Proxy::parse(&iter.next().ok_or("--proxy needs PATH=URL")?)?),
            "--cert" => cert = Some(PathBuf::from(iter.next().ok_or("--cert needs a file")?)),
            "--key" => key = Some(PathBuf::from(iter.next().ok_or("--key needs a file")?)),
            s if s.starts_with('-') => return Err(format!("unknown option: {s}")),
            dir => serve.root = PathBuf::from(dir),
        }
    }
    serve.https = match (cert, key) {
        (Some(cert), Some(key)) => Https::Files { cert, key },
        (Some(_), None) => return Err("--cert needs --key too (the certificate's private key)".into()),
        (None, Some(_)) => return Err("--key needs --cert too (the certificate it belongs to)".into()),
        (None, None) if self_signed => Https::SelfSigned,
        (None, None) => Https::Off,
    };
    let root = serve
        .root
        .canonicalize()
        .map_err(|e| format!("cannot open directory {}: {e}", serve.root.display()))?;
    serve.root = strip_verbatim(&root);

    Ok(match init_editor {
        Some(editor) => Command::Init { editor, root: serve.root, port: serve.port },
        None => Command::Serve(serve),
    })
}

/// "a, b,,c" → ["a", "b", "c"].
fn list(value: &str) -> Vec<String> {
    value.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()
}

/// Print and flush: editors read these lines to tell when the server is up.
fn say(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

fn run_init(editor: Editor, root: &PathBuf, port: u16) -> ExitCode {
    match init(editor, root, port) {
        Ok(InitOutcome::Created(path)) => {
            say(&format!("live-serve: wrote {}", path.display()));
            ExitCode::SUCCESS
        }
        Ok(InitOutcome::AlreadyThere(path)) => {
            say(&format!("live-serve: {} already has a live-serve task", path.display()));
            ExitCode::SUCCESS
        }
        Ok(InitOutcome::Exists(path)) => {
            say(&format!(
                "live-serve: {} has other tasks and was left untouched; add this by hand:\n\n{}",
                path.display(),
                tasks_json(editor, port)
            ));
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("live-serve: error: cannot write {}: {e}", tasks_path(editor, root).display());
            ExitCode::from(1)
        }
    }
}

async fn run_serve(args: ServeArgs) -> ExitCode {
    say(&format!("live-serve: starting in {}", args.root.display()));
    // On all addresses it is meant for other devices, which open it by this machine's network address.
    let everywhere = args.host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_unspecified()) || args.host == "[::]";
    let lan = if everywhere { lan_address() } else { None };
    let (tls, self_signed) = match &args.https {
        Https::Off => (None, None),
        Https::Files { cert, key } => match tls::from_pem_files(cert, key) {
            Ok(config) => (Some(config), None),
            Err(e) => {
                say(&format!("live-serve: error: {e}"));
                return ExitCode::from(1);
            }
        },
        Https::SelfSigned => match tls::self_signed(&tls::cache_dir(), &tls::certificate_names(&args.host, lan)) {
            Ok((config, cert)) => (Some(config), Some(cert)),
            Err(e) => {
                say(&format!("live-serve: error: cannot make a self-signed certificate: {e}"));
                return ExitCode::from(1);
            }
        },
    };
    let https = tls.is_some();

    let listener = match bind(&args.host, args.port, args.strict_port).await {
        Ok(l) => l,
        Err(e) => {
            say(&format!("live-serve: error: cannot listen on {}:{} ({e})", args.host, args.port));
            return ExitCode::from(1);
        }
    };
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(args.port);
    if port != args.port {
        say(&format!("live-serve: port {} is in use, using {port}", args.port));
    }
    let url = display_url(&args.host, port, https);
    say(&format!("live-serve: {url}"));
    // Say which address other devices should open, after the line above, which editors read as the URL.
    if let Some(ip) = lan {
        say(&format!("live-serve: on your network: {}", display_url(&ip.to_string(), port, https)));
    }
    for rule in &args.proxies {
        let path = if rule.prefix.is_empty() { "/" } else { &rule.prefix };
        say(&format!("live-serve: {path} goes to {}", rule.target));
    }
    // A backend that listens on this machine only is reachable from elsewhere through live-serve.
    if !args.proxies.is_empty() && !this_machine_only(&args.host) {
        say("live-serve: note: other devices that reach live-serve reach the proxied servers through it too");
    }
    if let Some(cert) = self_signed {
        say(&format!(
            "live-serve: the certificate is self-signed, so the browser warns once; it is kept in {}",
            cert.display()
        ));
    }
    if let Some(page) = &args.open {
        open_browser(&page_url(&url, page));
    }
    let stdin_closed = stdin_closed(args.exit_on_stdin_close);

    let mut cfg = Config::new(args.root);
    cfg.ignore = args.ignore;
    cfg.spa = args.spa;
    cfg.reload = args.reload;
    cfg.cors = args.cors;
    cfg.dotfiles = args.dotfiles;
    cfg.poll = args.poll;
    cfg.tls = tls;
    cfg.proxies = args.proxies;
    // The name it was started on is answered too, e.g. `--host mybox.lan`.
    cfg.allowed_hosts = args.allow_host;
    cfg.allowed_hosts.push(args.host);
    tokio::select! {
        result = serve(listener, cfg) => match result {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("live-serve: error: {e}");
                ExitCode::from(1)
            }
        },
        _ = tokio::signal::ctrl_c() => {
            say("live-serve: stopped");
            ExitCode::SUCCESS
        }
        _ = stdin_closed => {
            say("live-serve: stopped (standard input closed)");
            ExitCode::SUCCESS
        }
    }
}

/// Resolves once standard input reaches its end, when `enabled`; never otherwise. An editor that starts
/// live-serve with a pipe on stdin keeps it open while it runs; if the editor dies, so does the pipe.
/// A plain thread, not the runtime's blocking pool, which would wait for it at exit.
async fn stdin_closed(enabled: bool) {
    if !enabled {
        return std::future::pending().await;
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        use std::io::Read;
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 256];
        while matches!(stdin.read(&mut buf), Ok(n) if n > 0) {}
        let _ = tx.send(());
    });
    let _ = rx.await;
}

/// Whether a server on `host` is reachable from this machine only: `localhost`, `*.localhost` or a
/// loopback address.
fn this_machine_only(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
    host == "localhost"
        || host.ends_with(".localhost")
        || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

fn open_browser(url: &str) {
    #[cfg(windows)]
    let result = std::process::Command::new("cmd").args(["/C", "start", "", url]).spawn();
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(url).spawn();
    if let Err(e) = result {
        eprintln!("live-serve: could not open the browser: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serve_args(args: &[&str]) -> ServeArgs {
        match parse(args.iter().map(|a| a.to_string()).collect()) {
            Ok(Command::Serve(args)) => args,
            _ => panic!("not a serve command: {args:?}"),
        }
    }

    #[test]
    fn open_takes_a_page_only_after_an_equals_sign() {
        assert_eq!(serve_args(&[]).open, None);
        assert_eq!(serve_args(&["--open"]).open.as_deref(), Some(""));
        assert_eq!(serve_args(&["--open=docs/"]).open.as_deref(), Some("docs/"));
        // A word after a bare --open is still the folder to serve.
        let args = serve_args(&["--open", "src"]);
        assert_eq!(args.open.as_deref(), Some(""));
        assert!(args.root.ends_with("src"), "{}", args.root.display());
    }

    #[test]
    fn https_comes_from_files_or_a_self_signed_certificate() {
        assert_eq!(serve_args(&[]).https, Https::Off);
        assert_eq!(serve_args(&["--https"]).https, Https::SelfSigned);
        let files = Https::Files { cert: "c.pem".into(), key: "k.pem".into() };
        assert_eq!(serve_args(&["--cert", "c.pem", "--key", "k.pem"]).https, files);
        // Files win over a self-signed one.
        assert_eq!(serve_args(&["--https", "--key", "k.pem", "--cert", "c.pem"]).https, files);
        for half in [&["--cert", "c.pem"][..], &["--key", "k.pem"][..]] {
            let error = parse(half.iter().map(|a| a.to_string()).collect()).err().unwrap();
            assert!(error.contains("--cert") && error.contains("--key"), "{error}");
        }
        assert!(parse(vec!["--cert".into()]).is_err(), "a file is needed");
    }

    #[test]
    fn only_loopback_hosts_keep_proxied_servers_on_this_machine() {
        for host in ["127.0.0.1", "127.0.0.2", "localhost", "LocalHost", "app.localhost", "::1", "[::1]"] {
            assert!(this_machine_only(host), "{host}");
        }
        for host in ["0.0.0.0", "::", "[::]", "192.168.1.20", "mybox.lan", "localhost.example"] {
            assert!(!this_machine_only(host), "{host}");
        }
    }

    #[test]
    fn proxies_add_up_and_bad_ones_are_named() {
        assert!(serve_args(&[]).proxies.is_empty());
        let args = serve_args(&["--proxy", "/api=http://localhost:3000", "--proxy", "/auth=http://localhost:4000/v1"]);
        let rules: Vec<_> = args.proxies.iter().map(|p| (p.prefix.as_str(), p.target.as_str())).collect();
        assert_eq!(rules, [("/api", "http://localhost:3000"), ("/auth", "http://localhost:4000/v1")]);
        let error = parse(vec!["--proxy".into(), "/api=localhost:3000".into()]).err().unwrap();
        assert!(error.contains("http://"), "{error}");
        assert!(parse(vec!["--proxy".into()]).is_err(), "a rule is needed");
    }

    #[test]
    fn exit_on_stdin_close_is_opt_in() {
        assert!(!serve_args(&[]).exit_on_stdin_close);
        assert!(serve_args(&["--exit-on-stdin-close"]).exit_on_stdin_close);
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    match parse(std::env::args().skip(1).collect()) {
        Ok(Command::Help) => {
            println!("{HELP}");
            ExitCode::SUCCESS
        }
        Ok(Command::Version) => {
            println!("live-serve {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Ok(Command::Init { editor, root, port }) => run_init(editor, &root, port),
        Ok(Command::Serve(args)) => run_serve(args).await,
        Err(e) => {
            eprintln!("live-serve: error: {e}\n\n{HELP}");
            ExitCode::from(2)
        }
    }
}
