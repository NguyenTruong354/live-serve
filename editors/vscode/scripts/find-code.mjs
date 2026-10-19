// Finds the VS Code executable from the `code` command, for scripts/run-vscode-test.mjs.
import { existsSync, realpathSync } from 'node:fs';
import path from 'node:path';

/**
 * The VS Code executable behind the `code` command at `cli` on `platform`. The command is a script in
 * the install's bin/ folder, often reached through a symlink (/usr/local/bin/code on macOS,
 * /usr/bin/code on Linux), so the symlink is resolved first.
 */
export function codeExecutable(cli, platform, { realpath = realpathSync, exists = existsSync } = {}) {
  const { dirname, join } = platform === 'win32' ? path.win32 : path.posix;
  const script = realpath(cli);
  const up = (p, n) => (n === 0 ? p : up(dirname(p), n - 1));
  // <install>\bin\code -> <install>\Code.exe
  if (platform === 'win32') return join(up(script, 2), 'Code.exe');
  // <app>/Contents/Resources/app/bin/code -> <app>/Contents/MacOS/Code (Electron before VS Code 1.110)
  if (platform === 'darwin') {
    const macos = join(up(script, 4), 'MacOS');
    return [join(macos, 'Code'), join(macos, 'Electron')].find(exists) || join(macos, 'Code');
  }
  // <install>/bin/code -> <install>/code
  return join(up(script, 2), 'code');
}
