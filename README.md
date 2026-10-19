# live-serve

[![CI](https://github.com/NguyenTruong354/live-serve/actions/workflows/ci.yml/badge.svg)](https://github.com/NguyenTruong354/live-serve/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/live-serve.svg)](https://crates.io/crates/live-serve)

A tiny static dev server that reloads the page when files change. Think VS Code's Live Server, as a single ~1 MB binary with no Node.js. Run it from the command line, from VS Code or from Zed, for any project on Windows, macOS and Linux.

```
$ live-serve
live-serve: starting in /home/me/my-site
live-serve: http://127.0.0.1:5500/
```

## Quick start

1. **Install** with [Rust](https://rustup.rs):
   ```
   cargo install live-serve
   ```
   No Rust? Download the archive for your platform from [Releases](https://github.com/NguyenTruong354/live-serve/releases), unpack it, and put `live-serve` (`live-serve.exe` on Windows) on your `PATH`.
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

Troubleshooting:

- **403 "host name is not allowed":** you opened it under a name other than `localhost` or an IP address. Add `--allow-host <name>`.
- **It started on 5501:** something else, often VS Code's Live Server, already uses 5500.
- **The editor cannot find `live-serve`:** restart the editor so it picks up the new `PATH`.
- **macOS refuses to open it:** the binary is not signed. Run `xattr -d com.apple.quarantine live-serve` once.

## Features

- **Live reload.** Saving a file reloads open pages. Saving only CSS swaps the stylesheets in place, without a full reload.
- **Never stale.** Every response carries `Cache-Control: no-store`, so you always get the latest build.
- **Directory listings** for folders without an `index.html`.
- **Media friendly.** Files stream from disk, with HTTP Range support so audio and video can seek.
- **Single-page apps.** With `--spa`, unknown routes serve `/index.html`; missing assets still return 404.
- **Busy port? It moves on.** If port 5500 is taken, it uses the next free port. `--strict-port` turns this off.
- **Safe by default.** It binds to `127.0.0.1` and refuses paths that leave the served folder (`..`, encoded `..`, `\`, drive letters, symlinks pointing outside). Other websites cannot read your files: there is no CORS header unless you pass `--cors`, requests for unknown host names are refused (this stops DNS rebinding), and the reload WebSocket only accepts local pages. Dotfiles such as `.env` and `.git` are hidden unless you pass `--dotfiles`.
- **Editor tasks in one command** for VS Code and Zed (`live-serve init`).

## Install

**From [crates.io](https://crates.io/crates/live-serve)** (any platform with [Rust](https://rustup.rs)):

```
cargo install live-serve
```

To upgrade later, run the same command again.

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

The server watches the folder recursively. It waits until no file has changed for 120 ms, so a build that writes many files causes one reload, not many; during a build that runs longer than 2 seconds it reloads every 2 seconds. It then sends `css` if only stylesheets changed, or `reload` otherwise. Editor temporary files (`a.css~`, Vim swap files, JetBrains safe-write copies, `*.tmp`) are ignored, so saving a stylesheet still swaps it in place. If the server restarts, the script reconnects on its own.

With `--no-reload`, there is no script and no watcher.

## Building from source

```
cargo test
cargo build --release      # target/release/live-serve(.exe)
```

On Windows, the usual `x86_64-pc-windows-msvc` toolchain works out of the box once Visual Studio Build Tools are installed. With the `x86_64-pc-windows-gnu` toolchain instead, you also need a MinGW-w64 toolchain on `PATH`, because the build calls `dlltool` and `as`.

## License

MIT
