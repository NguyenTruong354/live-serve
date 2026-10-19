'use strict';
// Runs inside VS Code (see scripts/run-vscode-test.mjs): drives the extension the way a user would
// and checks the server it starts. The workspace is a temporary site with openBrowser off.

const vscode = require('vscode');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const http = require('node:http');
const net = require('node:net');
const path = require('node:path');

const get = (url) =>
  new Promise((resolve, reject) => {
    http.get(url, (res) => {
      let body = '';
      res.on('data', (c) => (body += c));
      res.on('end', () => resolve({ status: res.statusCode, body }));
    }).on('error', reject);
  });

const refused = (url) => get(url).then(() => false, () => true);

/** Resolves once `check()` is true; fails after 5 s. */
async function until(check, what) {
  const end = Date.now() + 5000;
  while (!check()) {
    if (Date.now() > end) throw new Error(`still waiting for ${what}`);
    await new Promise((r) => setTimeout(r, 1));
  }
}

/** The page's reload socket, once connected; next() is the next change message (tokens skipped). */
async function socket(port) {
  const ws = new WebSocket(`ws://127.0.0.1:${port}/__live/ws`);
  const queue = [];
  let waiter = null;
  let connected;
  const ready = new Promise((resolve) => (connected = resolve));
  ws.onmessage = (e) => {
    const data = String(e.data);
    if (data.startsWith('id:')) return connected();
    if (waiter) waiter(data);
    else queue.push(data);
    waiter = null;
  };
  await ready;
  return {
    next: () =>
      queue.length
        ? Promise.resolve(queue.shift())
        : Promise.race([
            new Promise((resolve) => (waiter = resolve)),
            new Promise((_, reject) => setTimeout(() => reject(new Error('no change within 5 s')), 5000)),
          ]),
    close: () => ws.close(),
  };
}

async function step(name, fn) {
  try {
    await fn();
    console.log(`ok - ${name}`);
  } catch (error) {
    console.log(`not ok - ${name}`);
    throw error;
  }
}

exports.run = async function run() {
  const ext = vscode.extensions.getExtension('nguyentruong354.live-serve');
  assert.ok(ext, 'extension is installed');
  const api = await ext.activate();
  const opened = [];
  api.setOpener(async (url) => opened.push(url));
  const root = vscode.workspace.workspaceFolders[0].uri.fsPath;

  await step('shows Go Live before starting', async () => {
    assert.match(api.statusText(), /Go Live/);
    assert.equal(api.state(), null);
  });

  let url;
  await step('Go Live starts the bundled server for the folder', async () => {
    await vscode.commands.executeCommand('liveServe.start');
    const state = api.state();
    assert.ok(state && state.url, 'server has an address');
    url = state.url;
    assert.equal(path.resolve(state.root), path.resolve(root));
    assert.match(api.statusText(), new RegExp(`Port: ${new URL(url).port}`));
    const page = await get(url);
    assert.equal(page.status, 200);
    assert.match(page.body, /Test site/);
    assert.match(page.body, /__live\/ws/, 'reload script injected');
    assert.deepEqual(opened, [], 'openBrowser is off in this workspace');
  });

  await step('saving CSS sends "css", saving HTML sends "reload"', async () => {
    const ws = await socket(new URL(url).port);
    try {
      await new Promise((r) => setTimeout(r, 300));
      fs.writeFileSync(path.join(root, 'style.css'), 'h1 { color: red; }');
      assert.equal(await ws.next(), 'css');
      await new Promise((r) => setTimeout(r, 300));
      fs.writeFileSync(path.join(root, 'sub', 'page.html'), '<h1>Changed</h1>');
      assert.equal(await ws.next(), 'reload');
    } finally {
      ws.close();
    }
  });

  await step('Open with live-serve opens the file\'s page', async () => {
    await vscode.commands.executeCommand('liveServe.open', vscode.Uri.file(path.join(root, 'sub', 'page.html')));
    assert.deepEqual(opened, [new URL('sub/page.html', url).href]);
    const page = await get(opened[0]);
    assert.match(page.body, /Changed/);
  });

  await step('Port: N stops the server', async () => {
    await vscode.commands.executeCommand('liveServe.toggle');
    assert.equal(api.state(), null);
    assert.match(api.statusText(), /Go Live/);
    assert.ok(await refused(url), 'nothing answers after stop');
  });

  await step('moves to the next port when the configured one is taken', async () => {
    const blocker = net.createServer();
    await new Promise((r) => blocker.listen(5591, '127.0.0.1', r));
    try {
      await vscode.commands.executeCommand('liveServe.start');
      const state = api.state();
      assert.ok(state && state.url);
      assert.notEqual(new URL(state.url).port, '5591');
      assert.match(api.statusText(), new RegExp(`Port: ${new URL(state.url).port}`));
    } finally {
      await vscode.commands.executeCommand('liveServe.stop');
      blocker.close();
    }
  });

  await step('Open with live-serve starts the server when it is not running', async () => {
    opened.length = 0;
    await vscode.commands.executeCommand('liveServe.open', vscode.Uri.file(path.join(root, 'index.html')));
    assert.ok(api.state(), 'server started');
    assert.equal(opened.length, 1, 'page opened even though openBrowser is off');
    assert.equal(new URL(opened[0]).pathname, '/');
    await vscode.commands.executeCommand('liveServe.stop');
  });

  const run = (...args) => vscode.commands.executeCommand(...args);
  const starting = () => /Starting/.test(api.statusText());

  await step('clicking Go Live again while it starts cancels it without an error', async () => {
    api.takeError();
    const first = run('liveServe.toggle');
    await until(starting, 'Starting…');
    await run('liveServe.toggle');
    assert.ok(!(await first), 'the start was cancelled');
    assert.equal(api.state(), null);
    assert.match(api.statusText(), /Go Live/);
    assert.equal(api.takeError(), null);
  });

  await step('two Go Live at once start one server', async () => {
    await Promise.all([run('liveServe.start'), run('liveServe.start')]);
    assert.ok(api.state()?.url);
    await run('liveServe.stop');
    for (const port of [5591, 5592, 5593]) assert.ok(await refused(`http://127.0.0.1:${port}/`), `nothing left on ${port}`);
  });

  await step('Open with live-serve while it starts opens the page once it listens', async () => {
    opened.length = 0;
    const first = run('liveServe.start');
    await until(starting, 'Starting…');
    await run('liveServe.open', vscode.Uri.file(path.join(root, 'sub', 'page.html')));
    assert.equal(opened.length, 1, 'the page opened once the server listened');
    assert.equal(opened[0], new URL('sub/page.html', api.state().url).href);
    await first;
    await run('liveServe.stop');
  });

  await step('a root folder that does not exist yet is named, not blamed on the binary', async () => {
    const cfg = vscode.workspace.getConfiguration('liveServe');
    await cfg.update('root', 'dist', vscode.ConfigurationTarget.Workspace);
    try {
      api.takeError();
      assert.ok(!(await run('liveServe.start')));
      assert.equal(api.state(), null);
      assert.equal(api.takeError(), `live-serve: folder ${path.join(root, 'dist')} does not exist.`);
    } finally {
      await cfg.update('root', undefined, vscode.ConfigurationTarget.Workspace);
    }
  });
};
