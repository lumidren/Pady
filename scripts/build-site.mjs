// Assembles the website into _site/: the pages in site/, the app's UI as the
// live demo (its built-in mock runs outside the app), and the README images.
//
//   node scripts/build-site.mjs           build _site/
//   node scripts/build-site.mjs --serve   build, then serve it on http://localhost:4173
//
// GitHub Pages publishes _site/ (.github/workflows/pages.yml).

import { createHash } from "node:crypto";
import { cpSync, createReadStream, existsSync, mkdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
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

// The demo's About page shows the version being released.
const version = readFileSync(join(ROOT, "Cargo.toml"), "utf8").match(/^version = "([^"]+)"/m)[1];
const mock = join(OUT, "demo", "mock.js");
writeFileSync(mock, readFileSync(mock, "utf8").replace(/version: '[^']*'/, `version: '${version}'`));

// Cache busting: Pages lets browsers keep files for 10 minutes, so a new page
// could meet an old stylesheet or script. Link each file with a hash of its contents.
const bust = (pageFile, files) => {
  const dir = dirname(pageFile);
  let html = readFileSync(pageFile, "utf8");
  for (const file of files) {
    const hash = createHash("sha256").update(readFileSync(join(dir, file))).digest("hex").slice(0, 10);
    html = html.replaceAll(`"${file}"`, `"${file}?v=${hash}"`);
  }
  writeFileSync(pageFile, html);
};
bust(join(OUT, "index.html"), ["style.css", "site.js"]);
bust(join(OUT, "demo", "index.html"), ["styles.css", "mock.js", "icons.js", "model.js", "app.js"]);
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
