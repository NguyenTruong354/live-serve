'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const path = require('node:path');
const { findUrl, serveArgs, pageUrl, binaryName } = require('../lib');

test('finds the address once the server listens, also when it moved to another port', () => {
  assert.equal(findUrl('live-serve: starting in /site\n'), null);
  assert.equal(findUrl('live-serve: starting in /site\nlive-serve: http://127.0.0.1:5500/\n'), 'http://127.0.0.1:5500/');
  const moved = 'live-serve: starting in C:\\site\r\nlive-serve: port 5500 is in use, using 5501\r\nlive-serve: http://127.0.0.1:5501/\r\n';
  assert.equal(findUrl(moved), 'http://127.0.0.1:5501/');
  assert.equal(findUrl('live-serve: error: cannot listen on 127.0.0.1:80 (denied)\n'), null);
});

test('waits for the whole line when output arrives in pieces', () => {
  assert.equal(findUrl('live-serve: http://127.0.0.1:55'), null);
  assert.equal(findUrl('live-serve: http://127.0.0.1:5500/\n'), 'http://127.0.0.1:5500/');
});

test('builds the arguments from the settings', () => {
  assert.deepEqual(serveArgs('/site', { port: 5500 }), ['/site', '--port', '5500']);
  assert.deepEqual(serveArgs('/site', { port: 8080, host: '0.0.0.0', extra: ['--spa'] }), [
    '/site', '--port', '8080', '--host', '0.0.0.0', '--spa',
  ]);
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
});

test('keeps names that only start with two dots', () => {
  assert.equal(posix('/site', '/site/..notes.html'), 'http://127.0.0.1:5500/..notes.html');
});

test('names the binary per platform', () => {
  assert.equal(binaryName('win32'), 'live-serve.exe');
  assert.equal(binaryName('darwin'), 'live-serve');
  assert.equal(binaryName('linux'), 'live-serve');
});
