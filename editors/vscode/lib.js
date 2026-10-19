'use strict';
// Pure helpers, kept apart from extension.js so they can be tested with plain Node.

const path = require('node:path');

/**
 * The address live-serve prints once it listens ("live-serve: http://127.0.0.1:5500/"), or null.
 * Only a finished line counts: output can arrive cut in the middle of the port.
 */
function findUrl(text) {
  const match = /^live-serve: (https?:\/\/\S+)\r?\n/m.exec(text);
  return match ? match[1] : null;
}

/** Command-line arguments that serve `root`. */
function serveArgs(root, { port, host, extra = [] }) {
  const args = [root, '--port', String(port)];
  if (host) args.push('--host', host);
  return args.concat(extra);
}

/**
 * The address of `file` on a server for `root` at `base`, or null when the file is outside `root`.
 * `index.html` maps to its folder, the way the server answers it.
 */
function pageUrl(base, root, file, sep = path.sep, relative = path.relative) {
  const rel = relative(root, file);
  if (rel === '..' || rel.startsWith('..' + sep) || /^([a-zA-Z]:|[\\/])/.test(rel)) return null;
  const parts = rel === '' ? [''] : rel.split(sep).map(encodeURIComponent);
  if (parts[parts.length - 1].toLowerCase() === 'index.html') parts[parts.length - 1] = '';
  return new URL(parts.join('/'), base).href;
}

/** File name of the bundled binary on `platform`. */
function binaryName(platform) {
  return platform === 'win32' ? 'live-serve.exe' : 'live-serve';
}

module.exports = { findUrl, serveArgs, pageUrl, binaryName };
