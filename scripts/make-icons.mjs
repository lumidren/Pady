// Renders Pady's app icons (PNG + multi-size ICO) with no dependencies.
//
//   node scripts/make-icons.mjs
//
// The mark: a violet rounded square holding a touchpad outline with three
// finger dots gliding upward. Shapes are signed-distance functions sampled
// 4×4 per pixel for clean anti-aliasing at every size.

import { deflateSync } from "node:zlib";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const OUT = join(dirname(fileURLToPath(import.meta.url)), "..", "crates", "pady-app", "icons");

// --- geometry (unit square, origin top-left) --------------------------------

const roundedRect = (px, py, cx, cy, hw, hh, r) => {
  const qx = Math.abs(px - cx) - hw + r;
  const qy = Math.abs(py - cy) - hh + r;
  return Math.hypot(Math.max(qx, 0), Math.max(qy, 0)) + Math.min(Math.max(qx, qy), 0) - r;
};
const circle = (px, py, cx, cy, r) => Math.hypot(px - cx, py - cy) - r;

const lerp = (a, b, t) => a + (b - a) * t;
const mix = (c1, c2, t) => c1.map((v, i) => lerp(v, c2[i], t));

const INDIGO = [99, 102, 241];
const VIOLET = [168, 85, 247];
const WHITE = [255, 255, 255];

/** Colour (premultiplied RGBA 0..1) of the icon at a point. */
function shade(x, y) {
  // Background tile.
  if (roundedRect(x, y, 0.5, 0.5, 0.47, 0.47, 0.22) > 0) return [0, 0, 0, 0];
  const t = Math.min(1, Math.max(0, (x + y) / 2));
  let rgb = mix(INDIGO, VIOLET, t);
  // Soft top highlight.
  rgb = mix(rgb, WHITE, Math.max(0, 0.18 - y * 0.3));

  // Touchpad outline.
  const pad = Math.abs(roundedRect(x, y, 0.5, 0.53, 0.3, 0.22, 0.07)) - 0.022;
  if (pad < 0) rgb = mix(rgb, WHITE, 0.92);

  // Three fingers with short trails below them (a swipe up).
  for (const fx of [0.36, 0.5, 0.64]) {
    for (let k = 3; k >= 1; k--) {
      if (circle(x, y, fx, 0.5 + k * 0.045, 0.045 - k * 0.008) < 0) rgb = mix(rgb, WHITE, 0.22 * (4 - k) / 3);
    }
    if (circle(x, y, fx, 0.48, 0.055) < 0) rgb = WHITE;
  }
  return [rgb[0] / 255, rgb[1] / 255, rgb[2] / 255, 1];
}

function render(size) {
  const ss = 4;
  const px = Buffer.alloc(size * size * 4);
  for (let j = 0; j < size; j++) {
    for (let i = 0; i < size; i++) {
      let r = 0, g = 0, b = 0, a = 0;
      for (let sj = 0; sj < ss; sj++) {
        for (let si = 0; si < ss; si++) {
          const [cr, cg, cb, ca] = shade((i + (si + 0.5) / ss) / size, (j + (sj + 0.5) / ss) / size);
          r += cr * ca; g += cg * ca; b += cb * ca; a += ca;
        }
      }
      const n = ss * ss, o = (j * size + i) * 4;
      const alpha = a / n;
      px[o] = alpha ? Math.round((r / a) * 255) : 0;
      px[o + 1] = alpha ? Math.round((g / a) * 255) : 0;
      px[o + 2] = alpha ? Math.round((b / a) * 255) : 0;
      px[o + 3] = Math.round(alpha * 255);
    }
  }
  return px;
}

// --- encoders ----------------------------------------------------------------

const CRC_TABLE = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});
const crc32 = (buf) => {
  let c = 0xffffffff;
  for (const byte of buf) c = CRC_TABLE[(c ^ byte) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
};

function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, crc]);
}

function png(size) {
  const rgba = render(size);
  const raw = Buffer.alloc((size * 4 + 1) * size);
  for (let y = 0; y < size; y++) {
    raw[y * (size * 4 + 1)] = 0; // filter: none
    rgba.copy(raw, y * (size * 4 + 1) + 1, y * size * 4, (y + 1) * size * 4);
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr.set([8, 6, 0, 0, 0], 8); // 8-bit RGBA
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", deflateSync(raw, { level: 9 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

/** ICO with PNG-compressed entries (supported since Windows Vista). */
function ico(sizes) {
  const images = sizes.map(png);
  const header = Buffer.alloc(6 + 16 * sizes.length);
  header.writeUInt16LE(0, 0);
  header.writeUInt16LE(1, 2);
  header.writeUInt16LE(sizes.length, 4);
  let offset = header.length;
  sizes.forEach((s, i) => {
    const e = 6 + i * 16;
    header[e] = s >= 256 ? 0 : s;
    header[e + 1] = s >= 256 ? 0 : s;
    header.writeUInt16LE(1, e + 4); // planes
    header.writeUInt16LE(32, e + 6); // bpp
    header.writeUInt32LE(images[i].length, e + 8);
    header.writeUInt32LE(offset, e + 12);
    offset += images[i].length;
  });
  return Buffer.concat([header, ...images]);
}

mkdirSync(OUT, { recursive: true });
const files = {
  "32x32.png": png(32),
  "64x64.png": png(64),
  "128x128.png": png(128),
  "128x128@2x.png": png(256),
  "icon.png": png(512),
  "icon.ico": ico([16, 24, 32, 48, 64, 128, 256]),
};
for (const [name, data] of Object.entries(files)) {
  writeFileSync(join(OUT, name), data);
  console.log(`wrote ${name} (${data.length} bytes)`);
}
