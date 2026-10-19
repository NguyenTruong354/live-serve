// Compares dev servers on the same folder with the same client (Node, keep-alive, one connection).
//
//   node bench/compare.mjs <label> <port> <command> [args...]
//
// The server is started with <command> [args...] and must listen on 127.0.0.1:<port>.
// Paths are relative to the served folder; override them with BENCH_PATHS="label=/path,label=/path".
// See BENCHMARKS.md for the commands used for live-serve and live-server.
import { spawn, execFileSync } from 'node:child_process';
import http from 'node:http';

const [label, port, cmd, ...args] = process.argv.slice(2);
if (!cmd) {
  console.error('usage: node bench/compare.mjs <label> <port> <command> [args...]');
  process.exit(2);
}
const REQUESTS = Number(process.env.BENCH_REQUESTS || 300);
const PATHS = (process.env.BENCH_PATHS ||
  'big listing=/big/,small listing=/small/,HTML page=/index.html,5 MB file=/bundle.js,small CSS=/style.css')
  .split(',')
  .map((pair) => pair.split('='));

const agent = new http.Agent({ keepAlive: true, maxSockets: 1 });
const get = (path) =>
  new Promise((resolve, reject) => {
    const start = process.hrtime.bigint();
    http
      .get({ host: '127.0.0.1', port, path, agent, headers: { 'Accept-Encoding': 'identity' } }, (res) => {
        let bytes = 0;
        res.on('data', (c) => (bytes += c.length));
        res.on('end', () => resolve({ ms: Number(process.hrtime.bigint() - start) / 1e6, bytes, status: res.statusCode }));
      })
      .on('error', reject);
  });

/** Resident memory of the server process in MB (Windows, macOS, Linux). */
function rss(pid) {
  try {
    if (process.platform === 'win32') {
      const out = execFileSync('powershell', ['-NoProfile', '-Command', `(Get-Process -Id ${pid}).WorkingSet64`]);
      return (Number(String(out).trim()) / 1048576).toFixed(1);
    }
    const out = execFileSync('ps', ['-o', 'rss=', '-p', String(pid)]);
    return (Number(String(out).trim()) / 1024).toFixed(1);
  } catch {
    return '?';
  }
}

const t0 = process.hrtime.bigint();
const server = spawn(cmd, args, { stdio: 'ignore' });
let startupMs = null;
for (let i = 0; i < 500 && startupMs === null; i++) {
  try {
    await get(PATHS[0][1]);
    startupMs = Number(process.hrtime.bigint() - t0) / 1e6;
  } catch {
    await new Promise((r) => setTimeout(r, 10));
  }
}

console.log(`== ${label}: first response after ${startupMs?.toFixed(0)} ms, memory idle ${rss(server.pid)} MB`);
try {
  for (const [name, path] of PATHS) {
    const n = /\.(js|mp4|zip)$/.test(path) ? Math.max(30, REQUESTS / 5) : REQUESTS;
    for (let i = 0; i < 20; i++) await get(path);
    const times = [];
    let last;
    for (let i = 0; i < n; i++) {
      last = await get(path);
      times.push(last.ms);
    }
    times.sort((a, b) => a - b);
    const p = (q) => times[Math.min(times.length - 1, Math.floor(times.length * q))].toFixed(2);
    console.log(`  ${name.padEnd(16)} p50 ${p(0.5).padStart(7)} ms  p95 ${p(0.95).padStart(7)} ms  ${String(last.bytes).padStart(9)} B  [${last.status}]`);
  }
  console.log(`  memory after load ${rss(server.pid)} MB`);
} finally {
  agent.destroy();
  server.kill();
}
