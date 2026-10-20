'use strict';
// A status bar button that runs the live-serve binary for the open folder: Go Live, then Port: N to stop.
// The server itself is the native binary in bin/ (one per platform-specific package); this file only
// starts it, reads the address it prints, and opens the browser.

const vscode = require('vscode');
const { spawn } = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');
const {
  findUrl,
  urlPort,
  serveArgs,
  folderProblem,
  pageUrl,
  binaryName,
  singleFlight,
  chooseFolder,
  spawnOptions,
} = require('./lib');

/**
 * The running server: { child, root, url, ready, stopping }. url is null until it listens; ready
 * resolves with the server once it listens, or null if it never does; stopping is set by stop().
 */
let server = null;
/** The last error shown, for the extension's own tests. */
let lastError = null;
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
    takeError: () => {
      const error = lastError;
      lastError = null;
      return error;
    },
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
    status.text = `$(circle-slash) Port: ${urlPort(server.url)}`;
    status.tooltip = `live-serve at ${server.url} (${server.root})\nClick to stop`;
  }
  status.command = 'liveServe.toggle';
}

/**
 * The folder to serve, { root, scope, base }: the workspace folder of `uri` or of the active editor,
 * plus the `root` setting; scope is the URI its settings are read for, and base the folder that relative
 * paths in them start from. Null when there is none, or when the folder pick was dismissed.
 */
async function pickRoot(uri) {
  const target = uri || vscode.window.activeTextEditor?.document.uri;
  const own = target && vscode.workspace.getWorkspaceFolder(target);
  const pick = () => vscode.window.showWorkspaceFolderPick({ placeHolder: 'Folder to serve' });
  const chosen = await chooseFolder(own, vscode.workspace.workspaceFolders || [], pick);
  if (!chosen) return null;
  const { folder } = chosen;
  if (!folder) {
    // A single file opened without a folder: serve the folder it is in.
    if (target?.scheme === 'file') {
      const dir = path.dirname(target.fsPath);
      return { root: dir, scope: target, base: dir };
    }
    showError('live-serve: open a folder first.');
    return null;
  }
  const sub = vscode.workspace.getConfiguration('liveServe', folder.uri).get('root') || '';
  return { root: path.resolve(folder.uri.fsPath, sub), scope: folder.uri, base: folder.uri.fsPath };
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
 * Then opens the browser at its root when `browser` (the openBrowser setting) is on. Calls made while
 * it starts share that start, so two clicks never start two servers.
 */
function start(uri, browser) {
  return server ? server.ready : launchOnce(uri, browser);
}

/** Picks the folder and runs the server for it; see start(). */
async function launch(uri, browser) {
  const picked = await pickRoot(uri);
  if (!picked) return null;
  const { root, scope, base } = picked;
  const problem = folderProblem(root);
  if (problem) {
    showError(problem);
    return null;
  }
  const cfg = vscode.workspace.getConfiguration('liveServe', scope);
  if (browser === undefined) browser = cfg.get('openBrowser');
  const exe = binary();
  const args = serveArgs(root, { port: cfg.get('port'), host: cfg.get('host'), extra: cfg.get('args') });
  output.appendLine(`> ${exe} ${args.join(' ')}`);

  // stdin stays an open pipe while the server runs: it exits once that closes (--exit-on-stdin-close),
  // also when the extension host dies without stopping it. It runs in the workspace folder.
  const child = spawn(exe, args, spawnOptions(base));
  child.stdin.on('error', () => {});
  const mine = { child, root, url: null, ready: null, stopping: false };
  mine.ready = watch(mine, browser);
  server = mine;
  showStatus();
  return mine.ready;
}

const launchOnce = singleFlight(launch);

/** Follows the server's output and exit; resolves with it once it listens, or null. */
function watch(mine, browser) {
  const { child } = mine;
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
      if (mine.url || mine.stopping) return;
      printed += text;
      const url = findUrl(printed);
      if (!url) return;
      mine.url = url;
      if (server === mine) showStatus();
      settleOnce(mine);
      if (browser) openUrl(url);
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
      if (!mine.stopping) showError(message);
      settleOnce(null);
    });
    child.on('exit', (code, signal) => {
      output.appendLine(`live-serve stopped (${signal || `exit code ${code}`})`);
      if (server === mine) server = null;
      showStatus();
      if (!mine.url && !mine.stopping) {
        showError('live-serve stopped before it was ready.', 'Show Output').then((choice) => {
          if (choice) output.show();
        });
      }
      settleOnce(null);
    });
  });
}

/** Stops the server; resolves once the process has exited. */
function stop() {
  if (!server) return Promise.resolve();
  const { child } = server;
  server.stopping = true; // so its exit, also before it listens, is not reported as a failure
  server = null;
  showStatus();
  return new Promise((resolve) => {
    if (child.exitCode !== null || child.signalCode !== null) return resolve();
    child.once('exit', () => resolve());
    child.kill();
  });
}

/**
 * Opens `uri` (or the active file) in the browser, whatever openBrowser says, once the server listens;
 * starts it first when needed. A server started here for a file outside its folder opens at its root.
 */
async function open(uri) {
  const file = uri || vscode.window.activeTextEditor?.document.uri;
  if (!file || file.scheme !== 'file') return;
  const running = server !== null;
  const ready = await start(file, false);
  if (!ready) return;
  const url = pageUrl(ready.url, ready.root, file.fsPath);
  if (!url && running) {
    vscode.window.showWarningMessage(`live-serve: ${path.basename(file.fsPath)} is outside the served folder (${ready.root}).`);
    return;
  }
  await openUrl(url || ready.url);
}

/** Shows an error message, and keeps it for the extension's own tests. */
function showError(message, ...items) {
  lastError = message;
  return vscode.window.showErrorMessage(message, ...items);
}

async function openUrl(url) {
  try {
    await opener(url);
  } catch (error) {
    output.appendLine(`live-serve: could not open the browser: ${error.message}`);
  }
}

module.exports = { activate, deactivate };
