# Benchmarks

live-serve 0.3.0 compared with [`live-server`](https://www.npmjs.com/package/live-server), the package VS Code's Live Server extension runs (the copy bundled with Live Server 5.7.10, `live-server` 2.2.0).

## Results

Median of six rounds, each round taking the median (p50) of 300 requests per path (60 for the 5 MB file) after 20 warm-up requests.

| | live-serve | live-server | |
|---|---:|---:|---|
| Folder listing, 1,200 entries | **2.2 ms** | 54.9 ms | ~25× faster |
| Folder listing, 7 entries | **0.65 ms** | 1.6 ms | ~2.5× faster |
| HTML page (reload script injected) | **0.54 ms** | 0.86 ms | ~1.6× faster |
| 5 MB JavaScript file | 8.4 ms | 8.7 ms | about the same |
| 4.5 KB stylesheet | 0.53 ms | 0.57 ms | about the same |
| First response after start (the 1,200-entry listing) | **45 ms** | ~1,470 ms | |
| Memory after the run | **10.4 MB** | ~240 MB | ~23× less |
| Listing page size, 1,200 entries | 122 KB | 276 KB | |

On a real project folder (a React app's `node_modules`, 1,116 entries), the listing took 1.4 ms with live-serve and 14.5 ms with live-server.

Static files cost about the same for both servers: reading the file and the loopback connection dominate. live-serve's lead comes from everything around the files: listings, HTML injection, start-up and memory.

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
