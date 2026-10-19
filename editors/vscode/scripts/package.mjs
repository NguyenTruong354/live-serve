// Builds the platform-specific package: copies the live-serve binary into bin/ and runs vsce.
//
//   node scripts/package.mjs <vscode-target> <path/to/live-serve[.exe]>
//
// vscode-target is one of win32-x64, darwin-arm64, darwin-x64, linux-x64. The package's version must
// match the crate's, so the extension always ships the server of the same release.
import { execFileSync } from 'node:child_process';
import { chmodSync, copyFileSync, mkdirSync, readFileSync, rmSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const TARGETS = ['win32-x64', 'darwin-arm64', 'darwin-x64', 'linux-x64'];
const here = dirname(dirname(fileURLToPath(import.meta.url)));
const [target, binary] = process.argv.slice(2);
if (!TARGETS.includes(target) || !binary) {
  console.error(`usage: node scripts/package.mjs <${TARGETS.join('|')}> <path/to/live-serve>`);
  process.exit(2);
}

const version = JSON.parse(readFileSync(join(here, 'package.json'), 'utf8')).version;
const crate = /^version\s*=\s*"([^"]+)"/m.exec(readFileSync(join(here, '..', '..', 'Cargo.toml'), 'utf8'))[1];
if (version !== crate) {
  console.error(`package.json is ${version} but Cargo.toml is ${crate}; bump both together.`);
  process.exit(1);
}

const bin = join(here, 'bin');
rmSync(bin, { recursive: true, force: true });
mkdirSync(bin);
const name = target.startsWith('win32') ? 'live-serve.exe' : 'live-serve';
copyFileSync(binary, join(bin, name));
if (!target.startsWith('win32')) chmodSync(join(bin, name), 0o755);
copyFileSync(join(here, '..', '..', 'LICENSE'), join(here, 'LICENSE'));

const out = `live-serve-${target}-${version}.vsix`;
const vsce = join(here, 'node_modules', '@vscode', 'vsce', 'vsce');
execFileSync(process.execPath, [vsce, 'package', '--target', target, '--out', out], { cwd: here, stdio: 'inherit' });
console.log(out);
