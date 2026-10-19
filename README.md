# live-serve

[![CI](https://github.com/NguyenTruong354/live-serve/actions/workflows/ci.yml/badge.svg)](https://github.com/NguyenTruong354/live-serve/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/live-serve.svg)](https://crates.io/crates/live-serve)

A tiny static dev server that reloads the page when files change. Think VS Code's Live Server, as a single ~1 MB binary with no Node.js. Run it from the command line, from VS Code or from Zed, for any project on Windows, macOS and Linux.

```
$ live-serve
live-serve: starting in /home/me/my-site
live-serve: http://127.0.0.1:5500/
```

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/NguyenTruong354/live-serve/main/docs/images/listing-dark.png">
  <img alt="A folder listing served by live-serve: clickable path, icons by file type, size and modified columns, and a filter box" src="https://raw.githubusercontent.com/NguyenTruong354/live-serve/main/docs/images/listing-light.png" width="720">
</picture>

## Why live-serve

Compared with [`live-server`](https://www.npmjs.com/package/live-server), the package VS Code's Live Server extension runs, on the same folder and the same machine ([how it was measured](BENCHMARKS.md)):

| | live-serve | live-server |
|---|---:|---:|
| Folder listing, 1,200 entries | **2.2 ms** | 54.9 ms |
| HTML page with the reload script | **0.54 ms** | 0.86 ms |
| First response after start | **45 ms** | ~1.5 s |
| Memory | **10 MB** | ~240 MB |
| Install | one ~1 MB binary | Node.js + packages |

Static files cost the same for both; the difference is everything around them. It also does a few things live-server does not:

- Refuses requests from other websites: DNS rebinding, cross-origin reads.
- Hides `.env` and `.git`.
- Swaps CSS without a reload, even when your editor writes temporary files.
- Sets up VS Code and Zed tasks with one command.

## Quick start

1. **Install** with your package manager:
   ```
   brew install nguyentruong354/tap/live-serve        # macOS, Linux
   scoop bucket add nguyentruong354 https://github.com/NguyenTruong354/scoop-bucket
   scoop install live-serve                           # Windows
   cargo install live-serve                           # anywhere with Rust
   ```
   More ways, including prebuilt archives that need nothing installed, are under [Install](#install).
2. **Run it in your site's folder:**
   ```
   live-serve --open
   ```
   Edit a file and the page reloads. Edit CSS and only the stylesheets swap. Stop with `Ctrl+C`.
3. **Optional: add an editor task** to the project, then start the server from the editor:
   ```
   live-serve init vscode     # Terminal → Run Task → live-serve (5500)
   live-serve init zed        # task: spawn → live-serve (5500)
   ```

Common cases:

| You want to | Run |
|---|---|
| serve a built single-page app | `live-serve ./dist --spa` |
| open it on your phone over Wi-Fi | `live-serve --host 0.0.0.0`, then `http://<your-ip>:5500` |
| use a custom name (hosts file, tunnel) | `live-serve --allow-host myapp.test` |
| let pages on other origins `fetch` from it | `live-serve --cors` |
| stop reloads from a busy build folder | `live-serve --ignore dist,coverage` |
| reload from WSL, Docker or a network drive | `live-serve --poll` |

Troubleshooting:

- **403 "host name is not allowed":** you opened it under a name other than `localhost` or an IP address. Add `--allow-host <name>`.
- **It started on 5501:** something else, often VS Code's Live Server, already uses 5500.
- **Saving does not reload** a folder in WSL, a Docker volume or on a network drive: the system sends no file events there. Add `--poll`.
- **The editor cannot find `live-serve`:** restart the editor so it picks up the new `PATH`.
- **macOS refuses to open it:** the binary is not signed. Run `xattr -d com.apple.quarantine live-serve` once.

## Features

- **Live reload.** Saving a file reloads open pages. Saving only CSS swaps the stylesheets in place, without a full reload.
- **Never stale.** Every response carries `Cache-Control: no-store`, so you always get the latest build.
- **Directory listings** for folders without an `index.html`: a clickable path, icons by file type, size and last-modified columns (in your local time), and a filter box (press `/`). They follow your system's light or dark theme, work on phones, and stay light: a folder of 1,200 entries renders in about 2 ms.
- **Media friendly.** Files stream from disk, with HTTP Range support so audio and video can seek.
- **Single-page apps.** With `--spa`, unknown routes serve `/index.html`; missing assets still return 404.
- **Busy port? It moves on.** If port 5500 is taken, it uses the next free port. `--strict-port` turns this off.
- **Safe by default.** It binds to `127.0.0.1` and refuses paths that leave the served folder (`..`, encoded `..`, `\`, drive letters, symlinks pointing outside). Other websites cannot read your files: there is no CORS header unless you pass `--cors`, requests for unknown host names are refused (this stops DNS rebinding), and the reload WebSocket only accepts local pages. Dotfiles such as `.env` and `.git` are hidden unless you pass `--dotfiles`.
- **Editor tasks in one command** for VS Code and Zed (`live-serve init`).

## Install

**[Homebrew](https://brew.sh)** (macOS on Apple Silicon or Intel, Linux on x86_64), from the [tap](https://github.com/NguyenTruong354/homebrew-tap):

```
brew install nguyentruong354/tap/live-serve
```

Upgrade with `brew upgrade live-serve`.

**[Scoop](https://scoop.sh)** (Windows), from the [bucket](https://github.com/NguyenTruong354/scoop-bucket):

```
scoop bucket add nguyentruong354 https://github.com/NguyenTruong354/scoop-bucket
scoop install live-serve
```

Upgrade with `scoop update live-serve`.

**winget** (Windows): the package `NguyenTruong354.LiveServe` is [waiting for review](https://github.com/microsoft/winget-pkgs/pull/455749). Once it is accepted:

```
winget install NguyenTruong354.LiveServe
```

**From [crates.io](https://crates.io/crates/live-serve)** (any platform with [Rust](https://rustup.rs)):

```
cargo install live-serve
```

To upgrade later, run the same command again.

**Prebuilt, through cargo** with [cargo-binstall](https://github.com/cargo-bins/cargo-binstall): it fetches the release binary for your platform, so nothing compiles.

```
cargo binstall live-serve
```

**Latest unreleased code**, straight from GitHub:

```
cargo install --git https://github.com/NguyenTruong354/live-serve
```

From a checkout of this repository, run `cargo install --path .`.

**Prebuilt binaries:** download the archive for your platform from [Releases](https://github.com/NguyenTruong354/live-serve/releases), unpack it, and put `live-serve` (or `live-serve.exe`) somewhere on your `PATH`.

## Command line

```
live-serve [DIR] [options]
live-serve init <vscode|zed> [DIR] [--port N]
```

| Option | Default | |
|---|---|---|
| `DIR` | current directory | folder to serve and watch |
| `-p, --port N` | `5500` | moves to the next free port if taken |
| `--strict-port` | off | fail instead of moving to another port |
| `--host H` | `127.0.0.1` | `0.0.0.0` to reach it from other devices on your network |
| `--ignore a,b` | | folder names not to watch, besides `node_modules` and `.git` |
| `--spa` | off | serve `/index.html` for unknown routes |
| `--allow-host a,b` | | host names to answer besides IP addresses, `localhost` and `*.localhost` (`*` for any), such as a name from your hosts file or a tunnel's domain |
| `--cors` | off | send `Access-Control-Allow-Origin: *`, so pages on other origins can read the files |
| `--dotfiles` | off | serve files and folders whose names start with `.` (`.well-known` is always served) |
| `--no-reload` | off | plain static server: no injected script, no file watching |
| `--poll` | off | scan for changes every 300 ms (skipping ignored folders) instead of using file system events; for WSL, Docker volumes and network drives, where events do not arrive. Reloads take about half a second instead of about 130 ms |
| `--open` | off | open the default browser once the server is up |
| `-V, --version` | | |
| `-h, --help` | | |

Examples:

```
live-serve                          # serve the current folder on 5500
live-serve ./dist --spa --open      # a built single-page app, in the browser
live-serve . --port 8080 --ignore target,coverage
live-serve --host 0.0.0.0           # test on your phone over Wi-Fi
```

Stop it with `Ctrl+C`.

## VS Code

From the project folder, run:

```
live-serve init vscode
```

This writes `.vscode/tasks.json` with a **live-serve (5500)** task. Run it from **Terminal → Run Task…** (or `Tasks: Run Task` in the command palette). It is a background task that VS Code considers started once the server prints its URL, so it can also be a `preLaunchTask` of a browser debug configuration.

To bind a key, add this to `keybindings.json`:

```json
{ "key": "alt+l", "command": "workbench.action.tasks.runTask", "args": "live-serve (5500)" }
```

## Zed

From the project folder, run:

```
live-serve init zed
```

This writes `.zed/tasks.json` with two tasks: **live-serve (5500)** and **live-serve (5500, open browser)**. Run them with `task: spawn`. To bind a key, add this to Zed's `keymap.json`:

```json
[
  { "bindings": { "alt-l": ["task::Spawn", { "task_name": "live-serve (5500)" }] } }
]
```

## Notes on `init`

- It never overwrites anything. If the task file already exists, it either says the live-serve task is already there or prints a snippet for you to paste in.
- `--port N` writes a task for another port.
- Task files belong to the project. To keep them out of your repository, add `/.vscode/` or `/.zed/` to `.gitignore`, or to `.git/info/exclude` to keep the change local.
- Editors read your `PATH` when they start. If a task cannot find `live-serve` right after installing, restart the editor.

## How it works

Each HTML page, including directory listings, gets a small script just before `</body>`. The script opens a WebSocket to `/__live/ws` on the same server.

The server watches the folder recursively. It waits until no file has changed for 120 ms, so a build that writes many files causes one reload, not many; during a build that runs longer than 2 seconds it reloads every 2 seconds. It then sends `css` if only stylesheets changed, or `reload` otherwise. Editor temporary files (`a.css~`, Vim swap files, JetBrains safe-write copies, `*.tmp`) are ignored, so saving a stylesheet still swaps it in place.

Before a reload, the script fetches the page itself and reloads only once that succeeds, retrying every 500 ms. A build that is halfway through never replaces the page you are looking at: not while the page answers 404 or 500, and not while its folder answers with a directory listing because `index.html` is gone for a moment.

If the connection drops (the server restarts, the laptop sleeps), the script reconnects every second. The server tells each new connection a version that changes with every change it announces and every time it starts; the page reloads only if that version differs from the one it had, so a reconnect with nothing new keeps what you typed into the page.

With `--poll`, the server scans the folder every 300 ms instead of waiting for the system's file events, which never arrive from some places: a Windows folder seen from WSL, a Docker volume, a network drive. The scan never enters `node_modules`, `.git` or `--ignore` folders, so their size costs nothing. A burst then ends after 420 ms of quiet instead of 120 ms, since each scan reports only what changed since the last, so a save takes about half a second to show (0.4–0.8 s) instead of about 130 ms.

With `--no-reload`, there is no script and no watcher.

## Building from source

```
cargo test
cargo build --release      # target/release/live-serve(.exe)
```

On Windows, the usual `x86_64-pc-windows-msvc` toolchain works out of the box once Visual Studio Build Tools are installed. With the `x86_64-pc-windows-gnu` toolchain instead, you also need a MinGW-w64 toolchain on `PATH`, because the build calls `dlltool` and `as`.

## License

MIT
