//! Tests of the `live-serve` binary itself: what it prints and when it stops.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn site(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("live-serve-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("index.html"), "<p>hi</p>").unwrap();
    dir
}

fn cache(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("live-serve-cli-cache-{name}-{}", std::process::id()))
}

/// Runs the binary with `args` to its end; returns its exit code and everything it printed.
fn run(args: &[&str]) -> (Option<i32>, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_live-serve"))
        .args(args)
        .env("LIVE_SERVE_CACHE_DIR", cache("run"))
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    (out.status.code(), text)
}

/// Starts the binary on a free port with `args`, stdin and stdout piped; killed on drop.
struct Server {
    child: Child,
    /// Its output, line by line, read on a thread so a test can wait with a time limit.
    lines: mpsc::Receiver<String>,
}

impl Server {
    fn start(name: &str, args: &[&str]) -> Server {
        let mut child = Command::new(env!("CARGO_BIN_EXE_live-serve"))
            .arg(site(name))
            .args(["--port", "0"])
            .args(args)
            // A self-signed certificate (--https) goes here, not into the user's cache folder.
            .env("LIVE_SERVE_CACHE_DIR", cache(name))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout: ChildStdout = child.stdout.take().unwrap();
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let _ = tx.send(line);
            }
        });
        Server { child, lines }
    }

    /// The next line it prints, without the line break; it must come within 5 s.
    fn line(&mut self) -> String {
        self.lines.recv_timeout(Duration::from_secs(5)).expect("a line within 5 s")
    }

    /// Lines up to and including the first that starts with `prefix`.
    fn until(&mut self, prefix: &str) -> Vec<String> {
        let mut lines = Vec::new();
        loop {
            let line = self.line();
            let done = line.starts_with(prefix);
            lines.push(line);
            if done {
                return lines;
            }
        }
    }

    /// Whether it has exited within `wait`.
    fn exits_within(&mut self, wait: Duration) -> bool {
        let start = Instant::now();
        while start.elapsed() < wait {
            if self.child.try_wait().unwrap().is_some() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn stops_when_stdin_closes_if_asked() {
    // An editor that dies without stopping it closes the pipe; the server must not outlive it.
    let mut server = Server::start("stdin", &["--exit-on-stdin-close"]);
    server.until("live-serve: http");
    drop(server.child.stdin.take());
    assert!(server.exits_within(Duration::from_secs(5)), "still running after stdin closed");
    assert_eq!(server.line(), "live-serve: stopped (standard input closed)");
}

#[test]
fn keeps_running_when_stdin_closes_by_default() {
    // `live-serve &` in a shell, or a service manager, gives it no stdin at all.
    let mut server = Server::start("stdin-default", &[]);
    server.until("live-serve: http");
    drop(server.child.stdin.take());
    assert!(!server.exits_within(Duration::from_millis(1500)), "stopped although not asked to");
}

#[test]
fn on_all_addresses_it_also_prints_the_network_address() {
    let mut server = Server::start("lan", &["--host", "0.0.0.0"]);
    let lines = server.until("live-serve: http");
    let url = lines.last().unwrap().clone();
    assert!(url.starts_with("live-serve: http://localhost:"), "{lines:?}");
    let port = url.trim_end_matches('/').rsplit(':').next().unwrap().to_string();
    match live_serve::lan_address() {
        Some(ip) => assert_eq!(server.line(), format!("live-serve: on your network: http://{ip}:{port}/")),
        None => eprintln!("SKIPPED: no network address on this machine"),
    }
}

#[test]
fn on_loopback_it_prints_only_the_local_address() {
    let mut server = Server::start("loopback", &["--exit-on-stdin-close"]);
    server.until("live-serve: http://127.0.0.1:");
    drop(server.child.stdin.take());
    assert!(server.exits_within(Duration::from_secs(5)));
    assert_eq!(server.line(), "live-serve: stopped (standard input closed)", "no network line in between");
}

#[test]
fn https_with_a_self_signed_certificate_says_so_and_keeps_it() {
    let mut server = Server::start("https", &["--https", "--exit-on-stdin-close"]);
    let lines = server.until("live-serve: http");
    assert!(lines.last().unwrap().starts_with("live-serve: https://127.0.0.1:"), "{lines:?}");
    let note = server.line();
    assert!(note.starts_with("live-serve: the certificate is self-signed"), "{note}");
    let cert = cache("https").join("cert.pem");
    assert!(note.ends_with(&cert.display().to_string()), "{note}");
    assert!(std::fs::read_to_string(&cert).unwrap().starts_with("-----BEGIN CERTIFICATE-----"));
}

#[test]
fn cert_and_key_go_together() {
    for args in [&["--cert", "c.pem"][..], &["--key", "k.pem"][..]] {
        let (code, text) = run(args);
        assert_eq!(code, Some(2), "{text}");
        assert!(text.contains("--cert") && text.contains("--key"), "{text}");
    }
}

#[test]
fn an_unreadable_certificate_is_named() {
    let (code, text) = run(&["--cert", "no-such-cert.pem", "--key", "no-such-key.pem", "--port", "0"]);
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("live-serve: error: cannot read the certificate no-such-cert.pem"), "{text}");
}
