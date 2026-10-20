# Benchmarks

live-serve 0.4.0 compared with [`live-server`](https://www.npmjs.com/package/live-server), the package VS Code's Live Server extension runs (the copy bundled with Live Server 5.7.10, `live-server` 2.2.0).

## Results

Median of six rounds, each round taking the median (p50) of 300 requests per path (60 for the 5 MB file) after 20 warm-up requests. The ranges are the fastest and slowest of the six rounds.

| | live-serve 0.4.0 | live-server | |
|---|---:|---:|---|
| Folder listing, 1,200 entries | **1.98 ms** (1.87–2.20) | 47.7 ms (45.6–98.1) | ~24× faster |
| Folder listing, 7 entries | **0.55 ms** (0.52–0.69) | 2.9 ms (1.4–3.6) | 2.5–5× faster |
| HTML page (reload script injected) | **0.50 ms** (0.44–0.64) | 1.46 ms (0.71–1.76) | 1.4–2.9× faster |
| 5 MB JavaScript file | 7.45 ms (7.15–7.61) | 14.3 ms (8.8–20.5) | about the same at best |
| 4.5 KB stylesheet | 0.45 ms (0.41–0.52) | 0.79 ms (0.42–1.54) | about the same at best |
| First response after start (the 1,200-entry listing) | **44 ms** | ~1,360 ms | |
| Memory after the run | **10.6 MB** | ~229 MB | ~22× less |
| Listing page size, 1,200 entries | 122 KB | 276 KB | |

live-server's numbers swing much more than live-serve's from round to round, and more when the machine is busy (this run started with 12–25 % CPU in use). Read its best rounds as the fair comparison: in a quieter run the same day it measured 49 ms, 1.5 ms, 0.72 ms, 9.0 ms and 0.48 ms on the five paths above, against live-serve's 2.2 ms, 0.57 ms, 0.51 ms, 7.6 ms and 0.49 ms. Static files and small stylesheets cost about the same for both: reading the file and the loopback connection dominate. live-serve's lead comes from everything around the files: listings, HTML injection, start-up and memory.

On a real project folder (a React app's `node_modules`, 1,116 entries), the listing took 1.4 ms with live-serve and 14.5 ms with live-server (measured with 0.3.0).

0.4.0 against 0.3.0 (2.2 ms, 0.65 ms, 0.54 ms, 8.4 ms, 0.53 ms, 45 ms, 10.4 MB): the same or a little faster, with HTTPS and the proxy built in. Neither is in the path when it is not turned on, and HTTPS itself is not measured here.

## Machine

- Windows 11 Pro on an Intel Core i9-13900H (20 threads) with 32 GB RAM.
- Node.js 24.21 runs both the client and live-server.
- live-serve is the release build (`cargo build --release`).

Inside VS Code, live-server runs in the extension host, a process the editor already has, so the memory it adds there is harder to separate. The figure above is live-server running on its own.

## Method

- One client for both servers: Node's `http` with keep-alive on a single connection, so every request reuses one socket. `Accept-Encoding: identity`, so neither server compresses.
- The servers run one after the other, never at the same time, and the rounds alternate between them.
- A loaded machine shifts both servers' numbers. Compare rounds, not single runs.
- An earlier PowerShell/.NET client showed spurious p95 spikes on responses over 85 KB (its large-object heap). The Node client does not.

## Running it yourself

```
node bench/make-site.mjs /tmp/bench-site

# live-serve
node bench/compare.mjs live-serve 5798 live-serve /tmp/bench-site --port 5798 --strict-port

# live-server: `npm install live-server` in this folder, or pass the copy inside the VS Code extension
node bench/compare.mjs live-server 5799 node bench/run-live-server.cjs /tmp/bench-site 5799 [path/to/live-server]
```

Set `BENCH_REQUESTS` to change the number of requests, and `BENCH_PATHS` (`label=/path,label=/path`) to measure other paths.
