// Runs test/vscode/index.js inside a real VS Code, with its own profile and no other extensions,
// on a temporary site. bin/ must hold the binary (scripts/package.mjs puts it there).
//
//   node scripts/run-vscode-test.mjs [path/to/Code executable]
//
// Without an argument it uses the VS Code found next to the `code` command on PATH.
import { execFileSync, spawn } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { binaryName } from '../lib.js';
import { codeExecutable } from './find-code.mjs';

const here = dirname(dirname(fileURLToPath(import.meta.url)));
const bundled = join(here, 'bin', binaryName(process.platform));
if (!existsSync(bundled)) {
  console.error(`${bundled} is missing: run scripts/package.mjs first.`);
  process.exit(2);
}

function findCode() {
  if (process.argv[2]) return process.argv[2];
  const which = process.platform === 'win32' ? 'where' : 'which';
  const cli = execFileSync(which, ['code'], { encoding: 'utf8' }).split(/\r?\n/)[0].trim();
  return codeExecutable(cli, process.platform);
}

const temp = mkdtempSync(join(tmpdir(), 'live-serve-vscode-'));
const site = join(temp, 'site');
mkdirSync(join(site, 'sub'), { recursive: true });
mkdirSync(join(site, '.vscode'));
writeFileSync(join(site, 'index.html'), '<!doctype html><link rel="stylesheet" href="style.css"><h1>Test site</h1>');
writeFileSync(join(site, 'style.css'), 'h1 { color: blue; }');
writeFileSync(join(site, 'sub', 'page.html'), '<h1>Page</h1>');
writeFileSync(join(site, '.vscode', 'settings.json'), JSON.stringify({ 'liveServe.port': 5591, 'liveServe.openBrowser': false }));

const env = { ...process.env };
delete env.ELECTRON_RUN_AS_NODE;
const code = findCode();
const child = spawn(
  code,
  [
    site,
    `--extensionDevelopmentPath=${here}`,
    `--extensionTestsPath=${join(here, 'test', 'vscode', 'index.js')}`,
    `--user-data-dir=${join(temp, 'user')}`,
    `--extensions-dir=${join(temp, 'extensions')}`,
    '--disable-extensions',
    '--disable-workspace-trust',
    '--skip-welcome',
    '--skip-release-notes',
    '--disable-telemetry',
    '--new-window',
  ],
  { env, stdio: ['ignore', 'pipe', 'pipe'] },
);
// VS Code logs a lot; keep the test's own lines and errors.
const keep = (line) => /^(ok|not ok) - |Error|assert|expected|actual|\bat .*index\.js/.test(line);
let rest = '';
const relay = (data) => {
  rest += data;
  const lines = rest.split(/\r?\n/);
  rest = lines.pop();
  for (const line of lines) if (keep(line)) console.log(line);
};
child.stdout.on('data', relay);
child.stderr.on('data', relay);
child.on('exit', (status) => {
  try {
    rmSync(temp, { recursive: true, force: true });
  } catch {}
  console.log(status === 0 ? 'VS Code tests passed' : `VS Code tests failed (exit ${status})`);
  process.exit(status ?? 1);
});
