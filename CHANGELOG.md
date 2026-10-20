# Changelog

## 0.4.0 (2026-10-20)

- `--proxy PATH=URL` sends requests under `PATH` to another local server, so a static frontend can call its API without CORS: `--proxy /api=http://localhost:3000` sends `/api/users` to `http://localhost:3000/users`. Every method and body passes through, `PATH` is replaced by the target's path as in VS Code's Live Server, the longest `PATH` wins, a backend redirect to itself (`http://localhost:3000/login` or `/login`) comes back under `PATH`, and a backend that is down gives a `502` that says why. WebSockets are not passed on yet. With `--host 0.0.0.0`, live-serve notes at start that other devices reach the proxied servers through it.
- Release builds check that the tag matches the version in `Cargo.toml`, the extension and `CHANGELOG.md` before building.
- Saving a stylesheet swaps the page's own stylesheets only. Ones from another origin (a CDN, Google Fonts) are left alone instead of being fetched again on every save.
- HTTPS. `--https` serves with a self-signed certificate that live-serve makes for `localhost`, `127.0.0.1`, `::1` and, with `--host 0.0.0.0`, the network address, keeps in the cache folder, one for each address, and renews a week before its 30 days run out; the browser warns once for each. `--cert` and `--key` take a certificate the browser trusts, such as one from mkcert. Live reload goes over `wss://`, and a plain `http://` request to the HTTPS port is redirected. Built on rustls with ring; the binary grows by about 1.2 MB.
- Live reload keeps working when a build deletes the served folder and makes it again (`rm -rf dist`). Until now the watch died with the old folder, and nothing reloaded after the first rebuild.
- A port is in use also when another program listens on it at the other kind of address: on `0.0.0.0` while live-serve asks for `127.0.0.1`, or the other way round. Windows and macOS let both binds coexist, so live-serve used to take localhost away from that program, or print a localhost address that the other program answered.
- `--host 0.0.0.0` also prints the address to open from a phone or another computer: `live-serve: on your network: http://192.168.1.20:5500/`.
- `--open=PAGE` opens the browser at a page: `--open=docs/`, `--open=about.html`.
- `--exit-on-stdin-close` stops the server when its standard input closes, so an editor that crashes leaves no server behind. The VS Code extension uses it.
- Media and other known types read in one go now say `Accept-Ranges: bytes`, as bigger files did, so players let you seek; `HEAD` requests no longer read the file.
- `--allow-host` names may carry a port (`myapp.test:8080`).
- Release archives and VS Code packages for Linux on Arm (`aarch64-unknown-linux-musl`) and Windows on Arm (`aarch64-pc-windows-msvc`). The Linux binaries are now static (musl), so they run on older distributions such as Ubuntu 20.04, Debian 11 and RHEL 8; the Linux x86_64 archive is now named `x86_64-unknown-linux-musl`.
- VS Code extension: see [its changelog](editors/vscode/CHANGELOG.md).

## 0.3.3 (2026-10-19)

- Saving a stylesheet through a temporary file renamed over it (`a.css.tmp.4242.…`, `a.css.2948423`), as write-file-atomic and many tools do, swaps the CSS again instead of reloading the page.
- VS Code extension in `editors/vscode`: **Go Live** / **Port: N** in the status bar, **Open with live-serve** on HTML files, the binary bundled per platform. Release builds attach a `.vsix` for each platform.

## 0.3.2 (2026-10-19)

- `--poll` scans the folder every 300 ms instead of waiting for file system events, which never arrive from WSL, Docker volumes or network drives. Ignored folders such as `node_modules` are never entered.
- A reload waits until the page works again: the script fetches it first and retries every 500 ms, so a build that answers 404 or 500 for a moment, or shows a folder listing in place of `index.html`, no longer replaces the page you are looking at.
- Reconnecting after a dropped connection reloads only if something changed meanwhile or the server restarted; otherwise the page, and what you typed into it, stays.
- Install with Homebrew (`brew install nguyentruong354/tap/live-serve`) or Scoop (the `nguyentruong354` bucket).

## 0.3.1 (2026-10-19)

- `cargo binstall live-serve` installs the prebuilt release binary.
- README: screenshot, comparison with live-server, [benchmarks](BENCHMARKS.md) and the scripts to reproduce them (`bench/`).

## 0.3.0 (2026-10-19)

- New folder listing:
  - clickable path, icons by file type, and size and last-modified columns (in the viewer's local time);
  - filter box (`/` to focus, `Esc` to clear);
  - light and dark themes and a phone layout.
- Listings render faster: about 1.4 ms for a 1,000-entry folder, down from 2.2 ms.
- Files up to 16 MB are read in one go. A 5 MB bundle went from 17 ms to 7.5 ms.

## 0.2.0 (2026-10-19)

- Refuses requests for host names other than IPs, `localhost` and `*.localhost`, which stops DNS rebinding. `--allow-host` adds names.
- The reload WebSocket accepts only local pages.
- No CORS header unless `--cors`.
- Hides dotfiles (`.env`, `.git`) unless `--dotfiles`; `.well-known` is always served.
- Reloads once per burst of changes, and at most every 2 s during a long build.
- Ignores editor temporary files, so saving a stylesheet still swaps it in place.
- HTML that is not UTF-8 keeps its bytes.
- Relative redirects for folders.

## 0.1.0 (2026-10-19)

First release:

- static server with live reload, and CSS swapped in place;
- `Cache-Control: no-store` on every response;
- folder listings;
- HTTP Range;
- `--spa`, `--no-reload`, `--open`;
- moves to the next free port when the requested one is taken (`--strict-port` to fail instead);
- `live-serve init vscode|zed`;
- prebuilt binaries for Windows, macOS and Linux.
