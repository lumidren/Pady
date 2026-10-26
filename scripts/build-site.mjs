// Assembles the website into _site/: the pages in site/, the app's UI as the
// live demo (its built-in mock runs outside the app), and the README images.
//
//   node scripts/build-site.mjs           build _site/
//   node scripts/build-site.mjs --serve   build, then serve it on http://localhost:4173
//
// GitHub Pages publishes _site/ (.github/workflows/pages.yml).

import { cpSync, createReadStream, existsSync, mkdirSync, rmSync, statSync } from "node:fs";
import { createServer } from "node:http";
import { dirname, extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const OUT = join(ROOT, "_site");

rmSync(OUT, { recursive: true, force: true });
mkdirSync(OUT);
cpSync(join(ROOT, "site"), OUT, { recursive: true });
cpSync(join(ROOT, "crates", "pady-app", "ui"), join(OUT, "demo"), { recursive: true });
cpSync(join(ROOT, "docs"), join(OUT, "docs"), { recursive: true });
console.log("built " + OUT);

if (process.argv.includes("--serve")) {
  const types = { ".html": "text/html", ".css": "text/css", ".js": "text/javascript", ".svg": "image/svg+xml", ".png": "image/png" };
  createServer((req, res) => {
    let path = normalize(decodeURIComponent(new URL(req.url, "http://x").pathname)).replace(/^([/\\])+/, "");
    let file = join(OUT, path);
    if (!file.startsWith(OUT) || !existsSync(file)) return res.writeHead(404).end("not found");
    if (statSync(file).isDirectory()) file = join(file, "index.html");
    res.writeHead(200, { "content-type": types[extname(file)] || "application/octet-stream" });
    createReadStream(file).pipe(res);
  }).listen(4173, () => console.log("serving on http://localhost:4173"));
}
