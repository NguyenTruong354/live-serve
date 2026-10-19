//! live-serve [DIR] [--port N] [--host H] [--ignore a,b] [--allow-host a,b] [--cors] [--dotfiles] [--poll] [--open[=PAGE]]
//! live-serve init <vscode|zed> [DIR] [--port N]

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use live_serve::init::{Editor, InitOutcome, init, tasks_json, tasks_path};
use live_serve::{Config, bind, display_url, lan_address, page_url, serve, strip_verbatim};

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
    };
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
            s if s.starts_with('-') => return Err(format!("unknown option: {s}")),
            dir => serve.root = PathBuf::from(dir),
        }
    }
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
    let url = display_url(&args.host, port);
    say(&format!("live-serve: {url}"));
    // On all addresses it is meant for other devices: say which address they should open.
    // After the line above, which editors read as the server's URL.
    let everywhere = args.host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_unspecified()) || args.host == "[::]";
    if everywhere && let Some(ip) = lan_address() {
        say(&format!("live-serve: on your network: {}", display_url(&ip.to_string(), port)));
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
