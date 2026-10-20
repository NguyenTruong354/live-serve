# Changelog

## 0.4.2 (2026-10-20)

- Renamed to `nguyentruong354.live-serve-rs`, as the name `live-serve-native` can no longer be used on the VS Code Marketplace. If you installed `nguyentruong354.live-serve-native` (or `nguyentruong354.live-serve`) from Open VSX, uninstall it and install this one; your `liveServe.*` settings carry over.

## 0.4.1 (2026-10-20)

- Renamed to `nguyentruong354.live-serve-native`, so it can be on the VS Code Marketplace, which already has an extension named `live-serve`. If you installed `nguyentruong354.live-serve` from Open VSX, uninstall it and install this one; your `liveServe.*` settings carry over.
- On the VS Code Marketplace and Open VSX.

## 0.4.0 (2026-10-20)

- The server runs in the workspace folder, so relative paths in `liveServe.args` work: `["--cert", ".certs/localhost.pem", "--key", ".certs/localhost-key.pem"]`.
- Packages for Linux on Arm and Windows on Arm. The Linux server is now a static binary, so it also runs over Remote-SSH on older distributions (Ubuntu 20.04, Debian 11, RHEL 8).
- A `liveServe.root` folder that does not exist yet (such as `dist` before a build) is reported as such, instead of as a missing binary.
- Clicking **Go Live** again while it starts cancels it, without an error; clicking twice quickly no longer starts two servers.
- **Open with live-serve** while the server starts opens the page once it listens.
- The server stops when VS Code goes away without stopping it, so it no longer holds the port.
- **Port: 80** (and 443) shows the port.
- Dismissing the **Folder to serve** pick in a multi-root workspace cancels quietly.
- Port, host, open-browser and options settings work per folder in multi-root workspaces; the port only takes 1 to 65535.
- In an untrusted folder, the folder's own settings cannot change the served folder either.

## 0.3.3

First release of the extension, with live-serve 0.3.3 inside:

- **Go Live** / **Port: N** in the status bar starts and stops the server for the open folder.
- **Open with live-serve** on HTML files.
- Settings for the port, host, served folder, extra options and the binary.
