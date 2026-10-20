'use strict';
// Pure helpers, kept apart from extension.js so they can be tested with plain Node.

const fs = require('node:fs');
const path = require('node:path');

/**
 * The address live-serve prints once it listens ("live-serve: http://127.0.0.1:5500/"), or null.
 * Only a finished line counts: output can arrive cut in the middle of the port.
 */
function findUrl(text) {
  const match = /^live-serve: (https?:\/\/\S+)\r?\n/m.exec(text);
  return match ? match[1] : null;
}

/** The port `url` is served on, also when the URL leaves out the scheme's default (80, 443). */
function urlPort(url) {
  const { port, protocol } = new URL(url);
  return port || (protocol === 'https:' ? '443' : '80');
}

/**
 * Command-line arguments that serve `root`. The server exits when its stdin closes, so it does not
 * outlive an extension host that dies without stopping it.
 */
function serveArgs(root, { port, host, extra = [] }) {
  const args = [root, '--port', String(port), '--exit-on-stdin-close'];
  if (host) args.push('--host', host);
  return args.concat(extra);
}

/** Why `root` cannot be served (it is missing or not a folder), or null when it can. */
function folderProblem(root) {
  const stat = fs.statSync(root, { throwIfNoEntry: false });
  if (!stat) return `live-serve: folder ${root} does not exist.`;
  if (!stat.isDirectory()) return `live-serve: ${root} is not a folder.`;
  return null;
}

/**
 * The address of `file` on a server for `root` at `base`, or null when the file is outside `root`.
 * `index.html` maps to its folder, the way the server answers it.
 */
function pageUrl(base, root, file, sep = path.sep, relative = path.relative) {
  const rel = relative(root, file);
  // A drive letter or a leading slash or backslash: on another drive, or on a network share (\\server\share).
  if (rel === '..' || rel.startsWith('..' + sep) || /^([a-zA-Z]:|[\\/])/.test(rel)) return null;
  const parts = rel === '' ? [''] : rel.split(sep).map(encodeURIComponent);
  if (parts[parts.length - 1].toLowerCase() === 'index.html') parts[parts.length - 1] = '';
  return new URL(parts.join('/'), base).href;
}

/** File name of the bundled binary on `platform`. */
function binaryName(platform) {
  return platform === 'win32' ? 'live-serve.exe' : 'live-serve';
}

/**
 * The workspace folder to serve: the target's `own`, else the only one, else one `pick`ed from several.
 * Resolves with { folder } (folder undefined when there is none), or null when the pick was dismissed.
 */
async function chooseFolder(own, folders, pick) {
  if (own || folders.length < 2) return { folder: own || folders[0] };
  const folder = await pick();
  return folder ? { folder } : null;
}

/** Wraps the async `fn` so that a call made while one is still running gets that call's promise. */
function singleFlight(fn) {
  let pending = null;
  return (...args) => {
    if (!pending) pending = fn(...args).finally(() => (pending = null));
    return pending;
  };
}

/**
 * How to start the server: stdin stays an open pipe (it exits once that closes), and it runs in `base`,
 * the workspace folder, so relative paths in liveServe.args (`--cert certs/localhost.pem`) mean what
 * they say there. Without a folder that exists, it runs where the extension host does.
 */
function spawnOptions(base) {
  const options = { stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true };
  if (base && folderProblem(base) === null) options.cwd = base;
  return options;
}

module.exports = {
  findUrl,
  urlPort,
  serveArgs,
  folderProblem,
  pageUrl,
  binaryName,
  chooseFolder,
  singleFlight,
  spawnOptions,
};
