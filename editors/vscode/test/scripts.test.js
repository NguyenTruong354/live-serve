'use strict';
// The manifest's settings and the packaging and test scripts.
const test = require('node:test');
const assert = require('node:assert/strict');
const { execFileSync } = require('node:child_process');
const path = require('node:path');

const here = path.join(__dirname, '..');
const manifest = require('../package.json');
const settings = manifest.contributes.configuration.properties;

test('an untrusted folder cannot change the served folder, the binary, the host or the options', () => {
  const restricted = manifest.capabilities.untrustedWorkspaces.restrictedConfigurations;
  for (const name of ['liveServe.root', 'liveServe.binaryPath', 'liveServe.host', 'liveServe.args']) {
    assert.ok(restricted.includes(name), name);
  }
});

test('settings can differ per folder of a multi-root workspace', () => {
  for (const name of ['port', 'host', 'root', 'openBrowser', 'args']) {
    assert.equal(settings[`liveServe.${name}`].scope, 'resource', name);
  }
  assert.equal(settings['liveServe.binaryPath'].scope, 'machine');
});

test('the port setting only takes whole port numbers', () => {
  const port = settings['liveServe.port'];
  assert.equal(port.type, 'integer');
  assert.equal(port.minimum, 1);
  assert.equal(port.maximum, 65535);
});

test('packages every platform the release builds', () => {
  let usage = '';
  try {
    execFileSync(process.execPath, [path.join(here, 'scripts', 'package.mjs')], { stdio: 'pipe' });
  } catch (error) {
    usage = String(error.stderr);
  }
  for (const target of ['win32-x64', 'win32-arm64', 'darwin-arm64', 'darwin-x64', 'linux-x64', 'linux-arm64']) {
    assert.match(usage, new RegExp(`\\b${target}\\b`), target);
  }
});

test('finds VS Code behind the code command, through symlinks', async () => {
  const { codeExecutable } = await import('../scripts/find-code.mjs');
  const links = {
    '/usr/local/bin/code': '/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code',
    '/usr/bin/code': '/usr/share/code/bin/code',
  };
  const realpath = (p) => links[p] || p;
  const mac = (exists) => codeExecutable('/usr/local/bin/code', 'darwin', { realpath, exists });
  assert.equal(mac((p) => p.endsWith('/Code')), '/Applications/Visual Studio Code.app/Contents/MacOS/Code');
  assert.equal(mac((p) => p.endsWith('/Electron')), '/Applications/Visual Studio Code.app/Contents/MacOS/Electron');
  assert.equal(codeExecutable('/usr/bin/code', 'linux', { realpath }), '/usr/share/code/code');
  assert.equal(codeExecutable('/opt/VSCode/bin/code', 'linux', { realpath }), '/opt/VSCode/code');
  const win = 'C:\\Users\\me\\AppData\\Local\\Programs\\Microsoft VS Code\\bin\\code';
  assert.equal(
    codeExecutable(win, 'win32', { realpath }),
    'C:\\Users\\me\\AppData\\Local\\Programs\\Microsoft VS Code\\Code.exe',
  );
});
