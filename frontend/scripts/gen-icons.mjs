// Generates the home-screen icons (public/icon.svg + PNGs) with no dependency:
// the design is described once below and rasterised here with a tiny
// anti-aliased renderer (4x4 supersampling) written straight to PNG.
// Run from frontend/:  node scripts/gen-icons.mjs
//
// Design (512 x 512 units): dark tile, grey 270-degree knob track, accent-blue
// partial arc, white needle. Echoes src/components/Knob.svelte. Original art.
import { deflateSync } from 'node:zlib';
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const OUT = join(dirname(fileURLToPath(import.meta.url)), '..', 'public');
const C = { bg: '#16181d', track: '#3a3f4b', body: '#1e2128', accent: '#3b93e0', needle: '#ffffff' };
const CX = 256, CY = 256, R = 158, TRACK_W = 34, BODY_R = 104, NEEDLE_W = 20;
const START = 135, SWEEP = 270, VALUE = 0.7; // degrees, clockwise from east; 0..1

const rad = (d) => (d * Math.PI) / 180;
const pt = (deg, r) => [CX + r * Math.cos(rad(deg)), CY + r * Math.sin(rad(deg))];
const f = (n) => +n.toFixed(2);
const valueDeg = START + SWEEP * VALUE;

export function svg() {
  const [sx, sy] = pt(START, R), [ex, ey] = pt(START + SWEEP, R), [vx, vy] = pt(valueDeg, R);
  const [n0x, n0y] = pt(valueDeg, 28), [n1x, n1y] = pt(valueDeg, BODY_R - 14);
  const large = SWEEP * VALUE > 180 ? 1 : 0;
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512">
  <rect width="512" height="512" rx="112" fill="${C.bg}"/>
  <path d="M${f(sx)} ${f(sy)}A${R} ${R} 0 1 1 ${f(ex)} ${f(ey)}" fill="none" stroke="${C.track}" stroke-width="${TRACK_W}" stroke-linecap="round"/>
  <path d="M${f(sx)} ${f(sy)}A${R} ${R} 0 ${large} 1 ${f(vx)} ${f(vy)}" fill="none" stroke="${C.accent}" stroke-width="${TRACK_W}" stroke-linecap="round"/>
  <circle cx="${CX}" cy="${CY}" r="${BODY_R}" fill="${C.body}"/>
  <path d="M${f(n0x)} ${f(n0y)}L${f(n1x)} ${f(n1y)}" stroke="${C.needle}" stroke-width="${NEEDLE_W}" stroke-linecap="round"/>
</svg>
`;
}

const hex = (h) => [1, 3, 5].map((i) => parseInt(h.slice(i, i + 2), 16));
const segDist = (x, y, ax, ay, bx, by) => {
  const dx = bx - ax, dy = by - ay;
  const t = Math.max(0, Math.min(1, ((x - ax) * dx + (y - ay) * dy) / (dx * dx + dy * dy)));
  return Math.hypot(x - ax - t * dx, y - ay - t * dy);
};
// Is (x, y) on the round-capped arc from START over `sweep` degrees?
function onArc(x, y, sweep) {
  const d = Math.hypot(x - CX, y - CY);
  let a = (Math.atan2(y - CY, x - CX) * 180) / Math.PI - START;
  a = ((a % 360) + 360) % 360;
  if (a <= sweep) return Math.abs(d - R) <= TRACK_W / 2;
  for (const deg of [START, START + sweep]) {
    const [px, py] = pt(deg, R);
    if (Math.hypot(x - px, y - py) <= TRACK_W / 2) return true;
  }
  return false;
}
const [n0x, n0y] = pt(valueDeg, 28), [n1x, n1y] = pt(valueDeg, BODY_R - 14);

/** Colour at design coordinates (x, y), or null when transparent. */
function sample(x, y, rounded) {
  if (rounded) {
    const q = 112, ix = Math.min(Math.max(x, q), 512 - q), iy = Math.min(Math.max(y, q), 512 - q);
    if (x < 0 || y < 0 || x > 512 || y > 512 || Math.hypot(x - ix, y - iy) > q) return null;
  }
  if (segDist(x, y, n0x, n0y, n1x, n1y) <= NEEDLE_W / 2) return C.needle;
  if (Math.hypot(x - CX, y - CY) <= BODY_R) return C.body;
  if (onArc(x, y, SWEEP * VALUE)) return C.accent;
  if (onArc(x, y, SWEEP)) return C.track;
  return C.bg;
}

function render(size, rounded) {
  const SS = 4, rows = [];
  for (let py = 0; py < size; py++) {
    const row = Buffer.alloc(1 + size * 4);
    for (let px = 0; px < size; px++) {
      let r = 0, g = 0, b = 0, a = 0;
      for (let sy = 0; sy < SS; sy++)
        for (let sx = 0; sx < SS; sx++) {
          const c = sample(((px + (sx + 0.5) / SS) * 512) / size, ((py + (sy + 0.5) / SS) * 512) / size, rounded);
          if (c) { const [cr, cg, cb] = hex(c); r += cr; g += cg; b += cb; a++; }
        }
      const o = 1 + px * 4;
      if (a) { row[o] = Math.round(r / a); row[o + 1] = Math.round(g / a); row[o + 2] = Math.round(b / a); }
      row[o + 3] = Math.round((a / (SS * SS)) * 255);
    }
    rows.push(row);
  }
  return png(size, Buffer.concat(rows));
}

function png(size, raw) {
  const crcT = Array.from({ length: 256 }, (_, n) => { let c = n; for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1; return c >>> 0; });
  const crc = (buf) => { let c = 0xffffffff; for (const b of buf) c = crcT[(c ^ b) & 255] ^ (c >>> 8); return (c ^ 0xffffffff) >>> 0; };
  const chunk = (type, data) => {
    const t = Buffer.from(type), len = Buffer.alloc(4), cr = Buffer.alloc(4);
    len.writeUInt32BE(data.length); cr.writeUInt32BE(crc(Buffer.concat([t, data])));
    return Buffer.concat([len, t, data, cr]);
  };
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0); ihdr.writeUInt32BE(size, 4); ihdr[8] = 8; ihdr[9] = 6;
  return Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk('IHDR', ihdr), chunk('IDAT', deflateSync(raw, { level: 9 })), chunk('IEND', Buffer.alloc(0))]);
}

mkdirSync(OUT, { recursive: true });
writeFileSync(join(OUT, 'icon.svg'), svg());
// apple-touch and maskable are full-bleed squares (iOS / Android apply their own mask).
for (const [name, size, rounded] of [
  ['icon-192.png', 192, true],
  ['icon-512.png', 512, true],
  ['icon-maskable-512.png', 512, false],
  ['apple-touch-icon.png', 180, false],
]) writeFileSync(join(OUT, name), render(size, rounded));
console.log('icons written to', OUT);
