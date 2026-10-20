'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const {
  findUrl,
  urlPort,
  serveArgs,
  folderProblem,
  pageUrl,
  binaryName,
  chooseFolder,
  singleFlight,
  spawnOptions,
} = require('../lib');

test('runs the server in the workspace folder, so relative paths in its arguments work', (t) => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'live-serve-cwd-'));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const options = spawnOptions(dir);
  assert.equal(options.cwd, dir);
  assert.deepEqual(options.stdio, ['pipe', 'pipe', 'pipe'], 'stdin stays open for --exit-on-stdin-close');
  assert.equal(options.windowsHide, true);
  // A folder that is gone would make spawn fail as if the binary were missing: no cwd then.
  assert.equal(spawnOptions(path.join(dir, 'gone')).cwd, undefined);
  assert.equal(spawnOptions(undefined).cwd, undefined);
});

test('finds the address once the server listens, also when it moved to another port', () => {
  assert.equal(findUrl('live-serve: starting in /site\n'), null);
  assert.equal(findUrl('live-serve: starting in /site\nlive-serve: http://127.0.0.1:5500/\n'), 'http://127.0.0.1:5500/');
  const moved = 'live-serve: starting in C:\\site\r\nlive-serve: port 5500 is in use, using 5501\r\nlive-serve: http://127.0.0.1:5501/\r\n';
  assert.equal(findUrl(moved), 'http://127.0.0.1:5501/');
  assert.equal(findUrl('live-serve: error: cannot listen on 127.0.0.1:80 (denied)\n'), null);
});

test('keeps the local address when the network address follows it', () => {
  const lan = 'live-serve: http://localhost:5500/\nlive-serve: on your network: http://192.168.1.20:5500/\n';
  assert.equal(findUrl(lan), 'http://localhost:5500/');
  assert.equal(findUrl('live-serve: on your network: http://192.168.1.20:5500/\n'), null);
});

test('reads the port, also when the URL leaves out the default one', () => {
  assert.equal(urlPort('http://127.0.0.1:5500/'), '5500');
  assert.equal(urlPort('http://127.0.0.1/'), '80');
  assert.equal(urlPort('http://127.0.0.1:80/'), '80');
  assert.equal(urlPort('https://localhost/'), '443');
  assert.equal(urlPort('https://localhost:8443/'), '8443');
});

test('waits for the whole line when output arrives in pieces', () => {
  assert.equal(findUrl('live-serve: http://127.0.0.1:55'), null);
  assert.equal(findUrl('live-serve: http://127.0.0.1:5500/\n'), 'http://127.0.0.1:5500/');
});

test('builds the arguments from the settings', () => {
  assert.deepEqual(serveArgs('/site', { port: 5500 }), ['/site', '--port', '5500', '--exit-on-stdin-close']);
  assert.deepEqual(serveArgs('/site', { port: 8080, host: '0.0.0.0', extra: ['--spa'] }), [
    '/site', '--port', '8080', '--exit-on-stdin-close', '--host', '0.0.0.0', '--spa',
  ]);
});

test('says when the folder to serve is missing or is a file', (t) => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'live-serve-lib-'));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  fs.writeFileSync(path.join(dir, 'page.html'), '');
  assert.equal(folderProblem(dir), null);
  const dist = path.join(dir, 'dist');
  assert.equal(folderProblem(dist), `live-serve: folder ${dist} does not exist.`);
  assert.match(folderProblem(path.join(dir, 'page.html')), /is not a folder/);
});

test("serves the target's folder, the only folder, or the one picked", async () => {
  const picked = [];
  const pick = (folder) => async () => (picked.push(folder), folder);
  assert.deepEqual(await chooseFolder('a', ['a', 'b'], pick('b')), { folder: 'a' });
  assert.deepEqual(await chooseFolder(undefined, ['a'], pick('b')), { folder: 'a' });
  assert.deepEqual(await chooseFolder(undefined, [], pick('b')), { folder: undefined });
  assert.deepEqual(picked, [], 'no pick without a choice to make');
  assert.deepEqual(await chooseFolder(undefined, ['a', 'b'], pick('b')), { folder: 'b' });
});

test('stops quietly when the folder pick is dismissed', async () => {
  assert.equal(await chooseFolder(undefined, ['a', 'b'], async () => undefined), null);
});

test('shares a start that is still running instead of starting again', async () => {
  let runs = 0;
  let finish;
  const once = singleFlight(() => (runs++, new Promise((resolve) => (finish = resolve))));
  const first = once('a');
  const second = once('b');
  assert.equal(first, second);
  assert.equal(runs, 1);
  finish('server');
  assert.equal(await second, 'server');
  once('c');
  assert.equal(runs, 2, 'a later call starts again');
});

const base = 'http://127.0.0.1:5500/';
const win = (root, file) => pageUrl(base, root, file, '\\', path.win32.relative);
const posix = (root, file) => pageUrl(base, root, file, '/', path.posix.relative);

test('maps files under the root to their page', () => {
  assert.equal(posix('/site', '/site/about.html'), 'http://127.0.0.1:5500/about.html');
  assert.equal(posix('/site', '/site/docs/a b.html'), 'http://127.0.0.1:5500/docs/a%20b.html');
  assert.equal(win('C:\\site', 'C:\\site\\docs\\page.html'), 'http://127.0.0.1:5500/docs/page.html');
  assert.equal(posix('/site', '/site/docs/index.html'), 'http://127.0.0.1:5500/docs/');
  assert.equal(posix('/site', '/site/Index.HTML'), 'http://127.0.0.1:5500/');
  assert.equal(posix('/site', '/site'), 'http://127.0.0.1:5500/');
  assert.equal(posix('/site', '/site/q?#.html'), 'http://127.0.0.1:5500/q%3F%23.html');
});

test('refuses files outside the root', () => {
  assert.equal(posix('/site/dist', '/site/index.html'), null);
  assert.equal(posix('/site', '/other/index.html'), null);
  assert.equal(win('C:\\site', 'D:\\site\\index.html'), null);
  assert.equal(win('C:\\site', 'C:\\site2\\index.html'), null);
  // On a network share: path.relative gives "\\server\share\a.html", which must not become http://server/.
  assert.equal(win('C:\\site', '\\\\server\\share\\a.html'), null);
  assert.equal(win('\\\\server\\share\\site', 'C:\\site\\a.html'), null);
});

test('keeps names that only start with two dots', () => {
  assert.equal(posix('/site', '/site/..notes.html'), 'http://127.0.0.1:5500/..notes.html');
});

test('names the binary per platform', () => {
  assert.equal(binaryName('win32'), 'live-serve.exe');
  assert.equal(binaryName('darwin'), 'live-serve');
  assert.equal(binaryName('linux'), 'live-serve');
});
