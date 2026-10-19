# Changelog

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
