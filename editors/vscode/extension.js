'use strict';
// A status bar button that runs the live-serve binary for the open folder: Go Live, then Port: N to stop.
// The server itself is the native binary in bin/ (one per platform-specific package); this file only
// starts it, reads the address it prints, and opens the browser.

const vscode = require('vscode');
const { spawn } = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');
const { findUrl, serveArgs, pageUrl, binaryName } = require('./lib');

/** The running server: { child, root, url }; url is null until it listens. */
let server = null;
let status;
let output;
let extensionPath;
/** Opens a URL; tests replace it so no browser opens. */
let opener = async (url) => {
  const external = await vscode.env.asExternalUri(vscode.Uri.parse(url));
  await vscode.env.openExternal(external);
};

function activate(context) {
  extensionPath = context.extensionPath;
  output = vscode.window.createOutputChannel('live-serve');
  status = vscode.window.createStatusBarItem('liveServe.status', vscode.StatusBarAlignment.Right, 100);
  status.name = 'live-serve';
  context.subscriptions.push(
    output,
    status,
    vscode.commands.registerCommand('liveServe.start', () => start()),
    vscode.commands.registerCommand('liveServe.stop', () => stop()),
    vscode.commands.registerCommand('liveServe.toggle', () => (server ? stop() : start())),
    vscode.commands.registerCommand('liveServe.open', (uri) => open(uri)),
  );
  showStatus();
  status.show();
  // For the extension's own tests.
  return {
    state: () => server && { url: server.url, root: server.root, pid: server.child.pid },
    statusText: () => status.text,
    setOpener: (fn) => (opener = fn),
  };
}

function deactivate() {
  return stop();
}

function showStatus() {
  if (!server) {
    status.text = '$(broadcast) Go Live';
    status.tooltip = 'Start live-serve for this folder';
  } else if (!server.url) {
    status.text = '$(loading~spin) Starting…';
    status.tooltip = 'live-serve is starting';
  } else {
    status.text = `$(circle-slash) Port: ${new URL(server.url).port}`;
    status.tooltip = `live-serve at ${server.url} (${server.root})\nClick to stop`;
  }
  status.command = 'liveServe.toggle';
}

/** The folder to serve: the workspace folder of `uri` or of the active editor, plus the `root` setting. */
async function pickRoot(uri) {
  const target = uri || vscode.window.activeTextEditor?.document.uri;
  let folder = target && vscode.workspace.getWorkspaceFolder(target);
  const folders = vscode.workspace.workspaceFolders || [];
  if (!folder && folders.length === 1) folder = folders[0];
  if (!folder && folders.length > 1) folder = await vscode.window.showWorkspaceFolderPick({ placeHolder: 'Folder to serve' });
  if (!folder) {
    // A single file opened without a folder: serve the folder it is in.
    if (target?.scheme === 'file') return path.dirname(target.fsPath);
    vscode.window.showErrorMessage('live-serve: open a folder first.');
    return null;
  }
  const sub = vscode.workspace.getConfiguration('liveServe', folder.uri).get('root') || '';
  return path.resolve(folder.uri.fsPath, sub);
}

/** The binary to run: the `binaryPath` setting, else the bundled one, else `live-serve` on PATH. */
function binary() {
  const custom = vscode.workspace.getConfiguration('liveServe').get('binaryPath');
  if (custom) return custom;
  const bundled = path.join(extensionPath, 'bin', binaryName(process.platform));
  if (!fs.existsSync(bundled)) return 'live-serve';
  if (process.platform !== 'win32') {
    // Packages do not always keep the executable bit.
    try {
      fs.accessSync(bundled, fs.constants.X_OK);
    } catch {
      fs.chmodSync(bundled, 0o755);
    }
  }
  return bundled;
}

/**
 * Starts the server (if it is not running) and resolves with it once it listens, or null on failure.
 * Then opens the browser at `uri`'s page, or the root, when `browser` (the openBrowser setting) is on.
 */
async function start(uri, browser) {
  if (server) return server;
  const root = await pickRoot(uri);
  if (!root) return null;
  const cfg = vscode.workspace.getConfiguration('liveServe', vscode.Uri.file(root));
  if (browser === undefined) browser = cfg.get('openBrowser');
  const exe = binary();
  const args = serveArgs(root, { port: cfg.get('port'), host: cfg.get('host'), extra: cfg.get('args') });
  output.appendLine(`> ${exe} ${args.join(' ')}`);

  const child = spawn(exe, args, { cwd: root, windowsHide: true });
  const mine = { child, root, url: null };
  server = mine;
  showStatus();

  return new Promise((resolve) => {
    let printed = '';
    let done = false;
    const settleOnce = (value) => {
      if (!done) resolve(value);
      done = true;
    };
    child.stdout.on('data', (data) => {
      const text = data.toString();
      output.append(text);
      if (mine.url) return;
      printed += text;
      const url = findUrl(printed);
      if (!url) return;
      mine.url = url;
      if (server === mine) showStatus();
      settleOnce(mine);
      if (browser) openUrl(uri ? pageUrl(url, root, uri.fsPath) || url : url);
    });
    child.stderr.on('data', (data) => output.append(data.toString()));
    child.on('error', (error) => {
      output.appendLine(`live-serve: ${error.message}`);
      if (server === mine) server = null;
      showStatus();
      const message =
        error.code === 'ENOENT'
          ? 'live-serve: the server binary was not found. Reinstall the extension, or set liveServe.binaryPath.'
          : `live-serve could not start: ${error.message}`;
      vscode.window.showErrorMessage(message);
      settleOnce(null);
    });
    child.on('exit', (code, signal) => {
      output.appendLine(`live-serve stopped (${signal || `exit code ${code}`})`);
      if (server === mine) server = null;
      showStatus();
      if (!mine.url) {
        vscode.window.showErrorMessage('live-serve stopped before it was ready.', 'Show Output').then((choice) => {
          if (choice) output.show();
        });
        settleOnce(null);
      }
    });
  });
}

/** Stops the server; resolves once the process has exited. */
function stop() {
  if (!server) return Promise.resolve();
  const { child } = server;
  server = null;
  showStatus();
  return new Promise((resolve) => {
    if (child.exitCode !== null || child.signalCode !== null) return resolve();
    child.once('exit', () => resolve());
    child.kill();
  });
}

/** Opens `uri` (or the active file) in the browser, starting the server first when needed. */
async function open(uri) {
  const file = uri || vscode.window.activeTextEditor?.document.uri;
  if (!file || file.scheme !== 'file') return;
  if (!server) {
    await start(file, true); // asked for this page, so it opens whatever openBrowser says
    return;
  }
  if (!server.url) return;
  const url = pageUrl(server.url, server.root, file.fsPath);
  if (!url) {
    vscode.window.showWarningMessage(`live-serve: ${path.basename(file.fsPath)} is outside the served folder (${server.root}).`);
    return;
  }
  await openUrl(url);
}

async function openUrl(url) {
  try {
    await opener(url);
  } catch (error) {
    output.appendLine(`live-serve: could not open the browser: ${error.message}`);
  }
}

module.exports = { activate, deactivate };
