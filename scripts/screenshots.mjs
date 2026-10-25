// Regenerates the README screenshots from the UI's built-in mock backend.
//
//   node scripts/screenshots.mjs [path-to-chrome-or-edge]
//
// Drives a headless Chromium browser over the DevTools protocol (Node's
// built-in WebSocket, no dependencies) and writes 2x PNGs to docs/screenshots.
// Every page but Live uses the mock's ?still switch, so no gesture animation
// is caught mid-flight.

import { spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const UI = join(ROOT, "crates", "pady-app", "ui");
const OUT = join(ROOT, "docs", "screenshots");
const PORT = 9333;

const BROWSERS = [
  process.argv[2],
  "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
  "C:/Program Files/Microsoft/Edge/Application/msedge.exe",
  "C:/Program Files/Google/Chrome/Application/chrome.exe",
  "/usr/bin/google-chrome",
  "/usr/bin/chromium",
  "/usr/bin/chromium-browser",
].filter(Boolean);

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const page = (file, query = "") => pathToFileURL(join(UI, file)).href + query;

/** Runs in the page: click the first visible element whose text matches. */
const CLICK_TEXT = `(text, selector = "button, [role=button], [role=tab], [role=radio], a") => {
  const el = [...document.querySelectorAll(selector)].find(
    (e) => e.getClientRects().length > 0 && e.textContent.trim().replace(/\\s+/g, " ").startsWith(text));
  if (!el) throw new Error("no element with text " + text);
  el.click();
  return true;
}`;

class Cdp {
  constructor(url) {
    this.ws = new WebSocket(url);
    this.id = 0;
    this.pending = new Map();
    this.ws.onmessage = (m) => {
      const msg = JSON.parse(m.data);
      const p = this.pending.get(msg.id);
      if (!p) return;
      this.pending.delete(msg.id);
      msg.error ? p.reject(new Error(msg.error.message)) : p.resolve(msg.result);
    };
  }
  open() {
    return new Promise((resolve, reject) => {
      this.ws.onopen = resolve;
      this.ws.onerror = reject;
    });
  }
  send(method, params = {}) {
    const id = ++this.id;
    this.ws.send(JSON.stringify({ id, method, params }));
    return new Promise((resolve, reject) => this.pending.set(id, { resolve, reject }));
  }
  async eval(expression) {
    const r = await this.send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description || "evaluation failed");
    return r.result.value;
  }
  /** Click the first visible element matching a CSS selector. */
  clickSelector(selector) {
    return this.eval(`(() => { const el = [...document.querySelectorAll(${JSON.stringify(selector)})].find((e) => e.getClientRects().length > 0 && !e.disabled); if (!el) throw new Error("no " + ${JSON.stringify(selector)}); el.click(); return true; })()`);
  }
  /** Resolve once the Live page has shown at least `min` fingers for `heldMs`. */
  waitForFingers(min, heldMs, timeoutMs = 20000) {
    return this.eval(`new Promise((resolve, reject) => {
      const start = performance.now();
      let since = 0;
      const timer = setInterval(() => {
        const el = document.querySelector(".live-stats .stat-value");
        const now = performance.now();
        since = Number(el && el.textContent) >= ${min} ? since || now : 0;
        if (since && now - since >= ${heldMs}) { clearInterval(timer); resolve(true); }
        else if (now - start > ${timeoutMs}) { clearInterval(timer); reject(new Error("no fingers on the Live page")); }
      }, 16);
    })`);
  }
  /** Drop keyboard focus so no focus ring shows in the picture. */
  blur() {
    return this.eval("document.activeElement && document.activeElement.blur(), true");
  }
  click(text, selector) {
    return this.eval(`(${CLICK_TEXT})(${JSON.stringify(text)}${selector ? ", " + JSON.stringify(selector) : ""})`);
  }
  async goto(url, settleMs = 2500) {
    await this.send("Page.navigate", { url });
    await sleep(settleMs);
  }
  async shot(name, clip) {
    const params = { format: "png", captureBeyondViewport: false };
    if (clip) params.clip = { ...clip, scale: 1 };
    const { data } = await this.send("Page.captureScreenshot", params);
    writeFileSync(join(OUT, name), Buffer.from(data, "base64"));
    console.log("wrote docs/screenshots/" + name);
  }
  async viewport(width, height, scheme = "dark") {
    await this.send("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: 2, mobile: false });
    await this.send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: scheme }] });
  }
}

async function main() {
  const browser = BROWSERS.find((b) => existsSync(b));
  if (!browser) throw new Error("no Chrome or Edge found; pass its path as an argument");
  mkdirSync(OUT, { recursive: true });
  const profile = mkdtempSync(join(tmpdir(), "pady-shots-"));
  const proc = spawn(browser, [
    "--headless=new", `--remote-debugging-port=${PORT}`, `--user-data-dir=${profile}`,
    "--hide-scrollbars", "--allow-file-access-from-files", "--no-first-run", "about:blank",
  ], { stdio: "ignore" });

  try {
    let targets;
    for (let i = 0; i < 50 && !targets; i++) {
      await sleep(200);
      targets = await fetch(`http://127.0.0.1:${PORT}/json/list`).then((r) => r.json()).catch(() => null);
    }
    const target = targets?.find((t) => t.type === "page");
    if (!target) throw new Error("the browser did not open a page");
    const cdp = new Cdp(target.webSocketDebuggerUrl);
    await cdp.open();
    await cdp.send("Page.enable");
    await cdp.viewport(1180, 760);

    await cdp.goto(page("index.html", "?noconflicts&still#gestures"), 3200);
    await cdp.click("4 fingers").catch(() => {});
    await cdp.blur();
    await sleep(1600);
    await cdp.shot("gestures.png");

    await cdp.click("Browser").catch(() => {});
    await cdp.blur();
    await sleep(4200); // switching profile replays the staggered card intro (~3.8 s)
    await cdp.shot("profiles.png");

    await cdp.goto(page("index.html", "?noconflicts#live"), 1200);
    // Catch the virtual hand mid-swipe: four fingers down, trails drawn.
    await cdp.waitForFingers(4, 650);
    await cdp.shot("live.png");

    await cdp.goto(page("index.html", "?noconflicts&still#settings"), 2600);
    await cdp.shot("settings.png");

    await cdp.goto(page("index.html", "?onboarding&noconflicts&still"), 2600);
    // Welcome → Your touchpad → OS gestures → Starter set.
    for (let i = 0; i < 3; i++) {
      await cdp.clickSelector(".tour-next").catch(() => {});
      await sleep(1100);
    }
    await cdp.blur();
    await sleep(600);
    await cdp.shot("tutorial.png");

    await cdp.viewport(560, 220);
    await cdp.goto(page("overlay.html", "?static"), 1800);
    // Keep the backdrop, drop the preview annotations (window outline, caption).
    await cdp.eval("document.getElementById('demo-caption').hidden = true; document.querySelector('.stage').style.outline = 'none'; true");
    await sleep(200);
    await cdp.shot("overlay.png");

    cdp.ws.close();
  } finally {
    proc.kill();
    await sleep(500);
    rmSync(profile, { recursive: true, force: true });
  }
}

main().catch((e) => {
  console.error(e.message);
  process.exit(1);
});
