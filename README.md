# live-serve

A tiny static dev server that reloads the page when files change. Think VS Code's Live Server, as a single ~1 MB binary with no Node.js. Run it from the command line, from VS Code or from Zed, for any project on Windows, macOS and Linux.

```
$ live-serve
live-serve: starting in /home/me/my-site
live-serve: http://127.0.0.1:5500/
```

## Features

- **Live reload.** Saving a file reloads open pages. Saving only CSS swaps the stylesheets in place, without a full reload.
- **Never stale.** Every response carries `Cache-Control: no-store`, so you always get the latest build.
- **Directory listings** for folders without an `index.html`.
- **Media friendly.** Files stream from disk, with HTTP Range support so audio and video can seek.
- **Single-page apps.** With `--spa`, unknown routes serve `/index.html`; missing assets still return 404.
- **Busy port? It moves on.** If port 5500 is taken, it uses the next free port. `--strict-port` turns this off.
- **Safe by default.** It binds to `127.0.0.1` and refuses paths that leave the served folder (`..`, encoded `..`, `\`, drive letters, symlinks pointing outside).
- **Editor tasks in one command** for VS Code and Zed (`live-serve init`).

## Install

**From source** (any platform with [Rust](https://rustup.rs)):

```
cargo install live-serve
```

To install from a checkout of this repository instead, run `cargo install --path .`.

**Prebuilt binaries:** download the archive for your platform from the GitHub Releases page, unpack it, and put `live-serve` (or `live-serve.exe`) somewhere on your `PATH`.

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

The server watches the folder recursively and batches events that arrive within 120 ms. It then sends `css` if only stylesheets changed, or `reload` otherwise. If the server restarts, the script reconnects on its own.

With `--no-reload`, there is no script and no watcher.

## Building from source

```
cargo test
cargo build --release      # target/release/live-serve(.exe)
```

On Windows, the usual `x86_64-pc-windows-msvc` toolchain works out of the box once Visual Studio Build Tools are installed. With the `x86_64-pc-windows-gnu` toolchain instead, you also need a MinGW-w64 toolchain on `PATH`, because the build calls `dlltool` and `as`.

## License

MIT
