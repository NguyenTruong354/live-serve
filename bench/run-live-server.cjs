// Starts the `live-server` npm package (what VS Code's Live Server extension runs) without a browser.
//
//   node bench/run-live-server.cjs <dir> <port> [path to the live-server package]
//
// Without a path it uses `require('live-server')`, so `npm install live-server` first, or point it
// at the copy inside the extension (…/.vscode/extensions/ritwickdey.liveserver-*/node_modules/live-server).
const [root, port, modulePath] = process.argv.slice(2);
const liveServer = require(modulePath || 'live-server');
liveServer.start({ root, port: Number(port), host: '127.0.0.1', open: false, logLevel: 0, wait: 100 });
