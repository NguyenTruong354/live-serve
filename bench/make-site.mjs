// Builds the folder the benchmarks serve: a 1,200-entry listing, a small one, an HTML page,
// a 5 MB "bundle" and a small stylesheet.
//
//   node bench/make-site.mjs <dir>
import fs from 'node:fs';
import path from 'node:path';

const dir = process.argv[2];
if (!dir) {
  console.error('usage: node bench/make-site.mjs <dir>');
  process.exit(2);
}
fs.rmSync(dir, { recursive: true, force: true });
fs.mkdirSync(path.join(dir, 'big'), { recursive: true });
fs.mkdirSync(path.join(dir, 'small'), { recursive: true });

for (let i = 0; i < 1200; i++) {
  const name = i % 3 === 0 ? `package-${i}` : `module-${i}.js`;
  const target = path.join(dir, 'big', name);
  if (i % 3 === 0) fs.mkdirSync(target);
  else fs.writeFileSync(target, 'x'.repeat(100 + (i % 900)));
}
for (const name of ['a.html', 'b.css', 'c.js', 'd.png', 'e.json', 'readme.md', 'notes.txt']) {
  fs.writeFileSync(path.join(dir, 'small', name), 'x'.repeat(512));
}
fs.writeFileSync(
  path.join(dir, 'index.html'),
  '<!doctype html><html><head><link rel="stylesheet" href="style.css"></head><body><h1>bench</h1></body></html>',
);
fs.writeFileSync(path.join(dir, 'style.css'), 'h1{color:#0969da}\n'.repeat(250));
fs.writeFileSync(path.join(dir, 'bundle.js'), Buffer.alloc(5 * 1024 * 1024, 'a'));
console.log(`site ready in ${dir}`);
