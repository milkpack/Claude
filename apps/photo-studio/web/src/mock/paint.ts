// Placeholder photographs drawn in code for the mock API (no image files ship with the app).
// They are abstract but photographic in tone: soft light, depth, grain and vignette, so the UI
// can be judged with something image-like in it.

export type Style = 'portrait' | 'landscape' | 'product' | 'interior' | 'poster';

export type RGB = [number, number, number];

export type Grade = { key: RGB; shadow: RGB; mid: RGB };

export function rng(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

export function seedOf(s: string): number {
  let h = 2166136261;
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return h >>> 0;
}

const css = (c: RGB, a = 1) => `rgba(${c[0] | 0},${c[1] | 0},${c[2] | 0},${a})`;
const mix = (a: RGB, b: RGB, t: number): RGB => [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
const shift = (c: RGB, d: number): RGB => [c[0] + d, c[1] + d, c[2] + d].map((x) => Math.max(0, Math.min(255, x))) as RGB;

const PALETTES: Record<string, Grade> = {
  daylight: { key: [236, 232, 224], shadow: [52, 58, 66], mid: [150, 150, 146] },
  warm: { key: [248, 196, 140], shadow: [40, 52, 58], mid: [168, 122, 92] },
  dusk: { key: [252, 170, 110], shadow: [28, 36, 64], mid: [120, 92, 120] },
  cool: { key: [206, 222, 236], shadow: [24, 32, 44], mid: [92, 112, 130] },
  amber: { key: [255, 196, 120], shadow: [18, 16, 14], mid: [92, 70, 52] },
  blue: { key: [150, 186, 230], shadow: [10, 16, 32], mid: [40, 62, 98] },
};

export function gradeFromWords(text: string): Grade {
  const t = text.toLowerCase();
  if (/dusk|sunset|golden|amber|warm|tungsten|candle/.test(t)) return t.includes('amber') ? PALETTES.amber! : PALETTES.warm!;
  if (/blue|night|jazz|moody|cool|teal/.test(t)) return t.includes('jazz') || t.includes('night') ? PALETTES.blue! : PALETTES.cool!;
  return PALETTES.daylight!;
}

export function styleFromWords(text: string): Style {
  const t = text.toLowerCase();
  if (/poster|headline|typograph|exact text|signage/.test(t)) return 'poster';
  if (/bottle|product|packshot|perfume|watch|sneaker/.test(t)) return 'product';
  if (/interior|room|studio desk|kitchen|office/.test(t)) return 'interior';
  if (/landscape|street|city|coast|mountain|dusk sky/.test(t)) return 'landscape';
  return 'portrait';
}

function makeCanvas(w: number, h: number): [HTMLCanvasElement, CanvasRenderingContext2D] {
  const c = document.createElement('canvas');
  c.width = Math.max(1, Math.round(w));
  c.height = Math.max(1, Math.round(h));
  const ctx = c.getContext('2d');
  if (!ctx) throw new Error('2D canvas is not available');
  return [c, ctx];
}

let grainTile: HTMLCanvasElement | null = null;
function grain(ctx: CanvasRenderingContext2D, w: number, h: number, amount: number) {
  if (!grainTile) {
    const [c, g] = makeCanvas(128, 128);
    const img = g.createImageData(128, 128);
    const r = rng(7);
    for (let i = 0; i < img.data.length; i += 4) {
      const v = 128 + (r() - 0.5) * 255;
      img.data[i] = img.data[i + 1] = img.data[i + 2] = v;
      img.data[i + 3] = 255;
    }
    g.putImageData(img, 0, 0);
    grainTile = c;
  }
  const pat = ctx.createPattern(grainTile, 'repeat');
  if (!pat) return;
  ctx.save();
  ctx.globalCompositeOperation = 'overlay';
  ctx.globalAlpha = amount;
  ctx.fillStyle = pat;
  ctx.fillRect(0, 0, w, h);
  ctx.restore();
}

function vignette(ctx: CanvasRenderingContext2D, w: number, h: number, strength: number) {
  const g = ctx.createRadialGradient(w / 2, h / 2, Math.min(w, h) * 0.35, w / 2, h / 2, Math.hypot(w, h) * 0.62);
  g.addColorStop(0, 'rgba(0,0,0,0)');
  g.addColorStop(1, `rgba(0,0,0,${strength})`);
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, w, h);
}

function glow(ctx: CanvasRenderingContext2D, x: number, y: number, r: number, c: RGB, a: number) {
  const g = ctx.createRadialGradient(x, y, 0, x, y, r);
  g.addColorStop(0, css(c, a));
  g.addColorStop(1, css(c, 0));
  ctx.fillStyle = g;
  ctx.fillRect(x - r, y - r, r * 2, r * 2);
}

function portrait(ctx: CanvasRenderingContext2D, w: number, h: number, p: Grade, r: () => number) {
  const bg = ctx.createLinearGradient(0, 0, w * 0.3, h);
  bg.addColorStop(0, css(mix(p.mid, p.key, 0.25)));
  bg.addColorStop(1, css(mix(p.shadow, p.mid, 0.35)));
  ctx.fillStyle = bg;
  ctx.fillRect(0, 0, w, h);
  const lx = w * (0.12 + r() * 0.2);
  ctx.save();
  ctx.globalCompositeOperation = 'screen';
  glow(ctx, lx, h * 0.3, w * 0.8, p.key, 0.45);
  ctx.restore();
  // out-of-focus backdrop shapes
  ctx.save();
  ctx.filter = `blur(${Math.round(w * 0.03)}px)`;
  for (let i = 0; i < 5; i++) {
    ctx.fillStyle = css(mix(p.mid, i % 2 ? p.key : p.shadow, 0.4), 0.35);
    ctx.beginPath();
    ctx.ellipse(r() * w, r() * h * 0.7, w * (0.1 + r() * 0.2), h * (0.1 + r() * 0.2), 0, 0, Math.PI * 2);
    ctx.fill();
  }
  ctx.restore();
  const cx = w * (0.47 + (r() - 0.5) * 0.12);
  const headY = h * (0.37 + (r() - 0.5) * 0.04);
  const hr = w * (0.12 + r() * 0.02);
  const tilt = (r() - 0.5) * 0.12;
  const skin: RGB = mix([192, 148, 122], p.key, 0.18);
  const hair = mix([44, 33, 28], p.shadow, 0.35);
  const longHair = r() > 0.5;
  // The subject is slightly soft, like a portrait lens focused on the eyes.
  ctx.save();
  ctx.filter = `blur(${Math.max(1, Math.round(w * 0.003))}px)`;
  // hair, back mass
  ctx.fillStyle = css(hair);
  ctx.beginPath();
  ctx.ellipse(cx, headY + (longHair ? hr * 0.45 : -hr * 0.05), hr * 1.08, hr * (longHair ? 1.75 : 1.2), tilt, 0, Math.PI * 2);
  ctx.fill();
  // shoulders and wardrobe
  const wardrobe = mix(p.shadow, [34, 34, 38], 0.5);
  const sg = ctx.createLinearGradient(cx - w * 0.4, 0, cx + w * 0.4, 0);
  sg.addColorStop(0, css(shift(wardrobe, 34)));
  sg.addColorStop(1, css(shift(wardrobe, -12)));
  ctx.fillStyle = sg;
  ctx.beginPath();
  ctx.moveTo(cx - w * 0.44, h);
  ctx.bezierCurveTo(cx - w * 0.42, h * 0.76, cx - w * 0.24, headY + hr * 2.0, cx - hr * 0.3, headY + hr * 1.9);
  ctx.lineTo(cx + hr * 0.3, headY + hr * 1.9);
  ctx.bezierCurveTo(cx + w * 0.24, headY + hr * 2.0, cx + w * 0.42, h * 0.76, cx + w * 0.44, h);
  ctx.closePath();
  ctx.fill();
  // neck with the shadow under the jaw
  const ng = ctx.createLinearGradient(0, headY + hr * 0.7, 0, headY + hr * 2);
  ng.addColorStop(0, css(mix(skin, p.shadow, 0.6)));
  ng.addColorStop(1, css(mix(skin, p.shadow, 0.25)));
  ctx.fillStyle = ng;
  ctx.beginPath();
  ctx.roundRect(cx - hr * 0.4, headY + hr * 0.6, hr * 0.8, hr * 1.45, hr * 0.2);
  ctx.fill();
  // head, lit from the key side
  const hg = ctx.createRadialGradient(cx - hr * 0.45, headY - hr * 0.25, hr * 0.1, cx + hr * 0.1, headY, hr * 1.35);
  hg.addColorStop(0, css(mix(skin, p.key, 0.4)));
  hg.addColorStop(0.55, css(skin));
  hg.addColorStop(1, css(mix(skin, p.shadow, 0.65)));
  ctx.fillStyle = hg;
  ctx.beginPath();
  ctx.ellipse(cx, headY, hr * 0.8, hr * 1.06, tilt, 0, Math.PI * 2);
  ctx.fill();
  // soft modelling: eye sockets, nose and mouth shadows
  ctx.fillStyle = css(mix(skin, p.shadow, 0.55), 0.55);
  for (const dx of [-0.32, 0.32]) {
    ctx.beginPath();
    ctx.ellipse(cx + hr * dx, headY - hr * 0.05, hr * 0.17, hr * 0.09, tilt, 0, Math.PI * 2);
    ctx.fill();
  }
  ctx.fillStyle = css(mix(skin, p.shadow, 0.5), 0.4);
  ctx.beginPath();
  ctx.ellipse(cx + hr * 0.08, headY + hr * 0.3, hr * 0.08, hr * 0.16, tilt, 0, Math.PI * 2);
  ctx.fill();
  ctx.fillStyle = css(mix([150, 86, 80], p.shadow, 0.3), 0.55);
  ctx.beginPath();
  ctx.ellipse(cx, headY + hr * 0.62, hr * 0.2, hr * 0.05, tilt, 0, Math.PI * 2);
  ctx.fill();
  // hair, front: a side-parted fringe that follows the skull
  ctx.save();
  ctx.beginPath();
  ctx.ellipse(cx, headY - hr * 0.08, hr * 0.9, hr * 1.12, tilt, 0, Math.PI * 2);
  ctx.clip();
  ctx.fillStyle = css(hair);
  ctx.beginPath();
  ctx.moveTo(cx - hr * 1.1, headY - hr * 0.1);
  ctx.quadraticCurveTo(cx - hr * 0.7, headY - hr * 0.75, cx + hr * 0.15, headY - hr * 0.62);
  ctx.quadraticCurveTo(cx + hr * 0.75, headY - hr * 0.5, cx + hr * 1.1, headY + hr * 0.05);
  ctx.lineTo(cx + hr * 1.1, headY - hr * 1.4);
  ctx.lineTo(cx - hr * 1.1, headY - hr * 1.4);
  ctx.closePath();
  ctx.fill();
  ctx.restore();
  ctx.restore();
  // rim light on the far side
  ctx.save();
  ctx.globalCompositeOperation = 'screen';
  ctx.strokeStyle = css(p.key, 0.3);
  ctx.lineWidth = w * 0.006;
  ctx.filter = `blur(${Math.round(w * 0.005)}px)`;
  ctx.beginPath();
  ctx.ellipse(cx, headY, hr * 0.8, hr * 1.06, tilt, -Math.PI * 0.45, Math.PI * 0.3);
  ctx.stroke();
  ctx.restore();
}

function landscape(ctx: CanvasRenderingContext2D, w: number, h: number, p: Grade, r: () => number) {
  const horizon = h * (0.55 + (r() - 0.5) * 0.1);
  const sky = ctx.createLinearGradient(0, 0, 0, horizon);
  sky.addColorStop(0, css(mix(p.shadow, p.mid, 0.4)));
  sky.addColorStop(0.7, css(mix(p.mid, p.key, 0.5)));
  sky.addColorStop(1, css(p.key));
  ctx.fillStyle = sky;
  ctx.fillRect(0, 0, w, horizon + 2);
  ctx.save();
  ctx.globalCompositeOperation = 'screen';
  glow(ctx, w * (0.3 + r() * 0.4), horizon, w * 0.5, p.key, 0.7);
  ctx.restore();
  for (let layer = 0; layer < 4; layer++) {
    const t = layer / 3;
    const base = horizon + layer * h * 0.06;
    ctx.fillStyle = css(mix(mix(p.mid, p.key, 0.3), p.shadow, 0.35 + t * 0.6));
    ctx.beginPath();
    ctx.moveTo(0, h);
    const f1 = 1 + r() * 2;
    const f2 = 3 + r() * 4;
    const ph = r() * 6;
    for (let x = 0; x <= w; x += w / 60) {
      const y = base - (Math.sin((x / w) * f1 * Math.PI + ph) * 0.5 + 0.5) * h * (0.06 + 0.04 * (1 - t)) - Math.sin((x / w) * f2 * Math.PI + ph * 2) * h * 0.012;
      ctx.lineTo(x, y);
    }
    ctx.lineTo(w, h);
    ctx.closePath();
    ctx.fill();
  }
}

function product(ctx: CanvasRenderingContext2D, w: number, h: number, p: Grade, r: () => number) {
  const bg = ctx.createLinearGradient(0, 0, 0, h);
  bg.addColorStop(0, css(shift(p.shadow, 6)));
  bg.addColorStop(0.62, css(mix(p.shadow, p.mid, 0.5)));
  bg.addColorStop(1, css(mix(p.shadow, p.mid, 0.25)));
  ctx.fillStyle = bg;
  ctx.fillRect(0, 0, w, h);
  const cx = w * (0.5 + (r() - 0.5) * 0.16);
  const floor = h * 0.78;
  ctx.save();
  ctx.globalCompositeOperation = 'screen';
  glow(ctx, cx, h * 0.42, Math.min(w, h) * 0.55, p.key, 0.5);
  ctx.restore();
  // shadow on the floor
  ctx.save();
  ctx.filter = `blur(${Math.round(w * 0.012)}px)`;
  ctx.fillStyle = 'rgba(0,0,0,0.55)';
  ctx.beginPath();
  ctx.ellipse(cx, floor, w * 0.16, h * 0.02, 0, 0, Math.PI * 2);
  ctx.fill();
  ctx.restore();
  const bw = Math.min(w * 0.2, h * 0.16);
  const bh = h * 0.36;
  const top = floor - bh;
  const glass = ctx.createLinearGradient(cx - bw, 0, cx + bw, 0);
  glass.addColorStop(0, css(mix(p.key, p.shadow, 0.85)));
  glass.addColorStop(0.35, css(mix(p.key, p.shadow, 0.35)));
  glass.addColorStop(0.5, css(p.key));
  glass.addColorStop(0.7, css(mix(p.key, p.shadow, 0.5)));
  glass.addColorStop(1, css(mix(p.key, p.shadow, 0.9)));
  ctx.fillStyle = glass;
  ctx.beginPath();
  ctx.roundRect(cx - bw / 2, top, bw, bh, bw * 0.12);
  ctx.fill();
  ctx.beginPath();
  ctx.moveTo(cx - bw / 2, top + bw * 0.25);
  ctx.quadraticCurveTo(cx - bw / 2, top - bh * 0.08, cx - bw * 0.16, top - bh * 0.12);
  ctx.lineTo(cx + bw * 0.16, top - bh * 0.12);
  ctx.quadraticCurveTo(cx + bw / 2, top - bh * 0.08, cx + bw / 2, top + bw * 0.25);
  ctx.fill();
  ctx.fillRect(cx - bw * 0.16, top - bh * 0.3, bw * 0.32, bh * 0.2);
  ctx.fillStyle = css(shift(p.shadow, 18));
  ctx.fillRect(cx - bw * 0.19, top - bh * 0.4, bw * 0.38, bh * 0.12);
  // label and highlight
  ctx.fillStyle = css(mix([226, 220, 206], p.key, 0.2), 0.92);
  ctx.fillRect(cx - bw * 0.38, top + bh * 0.38, bw * 0.76, bh * 0.3);
  ctx.fillStyle = css(p.shadow, 0.55);
  for (let i = 0; i < 3; i++) ctx.fillRect(cx - bw * 0.24, top + bh * (0.45 + i * 0.07), bw * (0.48 - i * 0.12), Math.max(1, bh * 0.012));
  ctx.save();
  ctx.globalCompositeOperation = 'screen';
  ctx.filter = `blur(${Math.max(1, Math.round(bw * 0.03))}px)`;
  ctx.fillStyle = 'rgba(255,255,255,0.55)';
  ctx.fillRect(cx - bw * 0.3, top - bh * 0.05, bw * 0.06, bh * 0.98);
  ctx.restore();
}

function interior(ctx: CanvasRenderingContext2D, w: number, h: number, p: Grade, r: () => number) {
  const wall = ctx.createLinearGradient(0, 0, w, 0);
  wall.addColorStop(0, css(mix(p.mid, p.key, 0.3)));
  wall.addColorStop(1, css(mix(p.shadow, p.mid, 0.4)));
  ctx.fillStyle = wall;
  ctx.fillRect(0, 0, w, h);
  const wx = w * (0.12 + r() * 0.15);
  ctx.save();
  ctx.filter = `blur(${Math.round(w * 0.01)}px)`;
  ctx.fillStyle = css(shift(p.key, 10));
  ctx.fillRect(wx, h * 0.14, w * 0.26, h * 0.42);
  ctx.restore();
  ctx.save();
  ctx.globalCompositeOperation = 'screen';
  ctx.fillStyle = css(p.key, 0.16);
  ctx.beginPath();
  ctx.moveTo(wx, h * 0.14);
  ctx.lineTo(wx + w * 0.26, h * 0.14);
  ctx.lineTo(w * 0.95, h);
  ctx.lineTo(w * 0.35, h);
  ctx.closePath();
  ctx.fill();
  ctx.restore();
  ctx.fillStyle = css(mix(p.shadow, p.mid, 0.3));
  ctx.fillRect(0, h * 0.74, w, h * 0.26);
  ctx.fillStyle = css(shift(p.shadow, 10));
  ctx.fillRect(w * 0.5, h * 0.6, w * 0.4, h * 0.05);
  ctx.fillRect(w * 0.53, h * 0.64, w * 0.02, h * 0.2);
  ctx.fillRect(w * 0.85, h * 0.64, w * 0.02, h * 0.2);
}

function poster(ctx: CanvasRenderingContext2D, w: number, h: number, p: Grade, r: () => number, headline: string, sub: string) {
  landscape(ctx, w, h, p, r);
  ctx.save();
  ctx.fillStyle = css(shift(p.shadow, -6), 0.92);
  const cx = w * 0.58;
  ctx.beginPath();
  ctx.ellipse(cx, h * 0.5, w * 0.07, w * 0.09, 0, 0, Math.PI * 2);
  ctx.fill();
  ctx.beginPath();
  ctx.moveTo(cx - w * 0.2, h);
  ctx.quadraticCurveTo(cx - w * 0.16, h * 0.6, cx, h * 0.6);
  ctx.quadraticCurveTo(cx + w * 0.16, h * 0.6, cx + w * 0.2, h);
  ctx.fill();
  ctx.restore();
  ctx.fillStyle = 'rgba(240,238,232,0.95)';
  ctx.textAlign = 'center';
  let size = w * 0.11;
  ctx.font = `600 ${size}px "IBM Plex Sans", sans-serif`;
  while (ctx.measureText(headline).width > w * 0.86 && size > 8) {
    size *= 0.92;
    ctx.font = `600 ${size}px "IBM Plex Sans", sans-serif`;
  }
  ctx.fillText(headline, w / 2, h * 0.16);
  if (sub) {
    ctx.font = `400 ${w * 0.035}px "IBM Plex Mono", monospace`;
    ctx.fillStyle = 'rgba(240,238,232,0.8)';
    ctx.fillText(sub, w / 2, h * 0.16 + size * 0.7);
  }
}

export type PaintOptions = {
  seed: number;
  width: number;
  height: number;
  style: Style;
  grade: Grade;
  text?: { headline: string; sub: string };
};

export function paint(o: PaintOptions): HTMLCanvasElement {
  const [c, ctx] = makeCanvas(o.width, o.height);
  const r = rng(o.seed);
  const g: Grade = {
    key: shift(o.grade.key, (r() - 0.5) * 16),
    shadow: shift(o.grade.shadow, (r() - 0.5) * 10),
    mid: shift(o.grade.mid, (r() - 0.5) * 20),
  };
  const { width: w, height: h } = c;
  if (o.style === 'portrait') portrait(ctx, w, h, g, r);
  else if (o.style === 'landscape') landscape(ctx, w, h, g, r);
  else if (o.style === 'product') product(ctx, w, h, g, r);
  else if (o.style === 'interior') interior(ctx, w, h, g, r);
  else poster(ctx, w, h, g, r, o.text?.headline || 'UNTITLED', o.text?.sub ?? '');
  vignette(ctx, w, h, 0.42);
  grain(ctx, w, h, 0.07);
  return c;
}

/** An "edit" of a source image: re-crop slightly and re-grade toward a mood palette. */
export function regrade(src: CanvasImageSource & { width: number; height: number }, o: { seed: number; width: number; height: number; grade: Grade; strength: number }): HTMLCanvasElement {
  const [c, ctx] = makeCanvas(o.width, o.height);
  const r = rng(o.seed);
  const { width: w, height: h } = c;
  const zoom = 1.02 + r() * 0.08;
  const sw = src.width;
  const sh = src.height;
  const scale = Math.max(w / sw, h / sh) * zoom;
  const dw = sw * scale;
  const dh = sh * scale;
  // Re-crop within the overscan so the frame is always covered.
  const ox = (w - dw) / 2 + (r() - 0.5) * (dw - w);
  const oy = (h - dh) / 2 + (r() - 0.5) * (dh - h);
  ctx.filter = `brightness(${(0.9 + r() * 0.18).toFixed(3)}) contrast(${(1.02 + r() * 0.14).toFixed(3)}) saturate(${(0.85 + r() * 0.35).toFixed(3)})`;
  ctx.drawImage(src, ox, oy, dw, dh);
  ctx.filter = 'none';
  const s = o.strength * (0.7 + r() * 0.6);
  ctx.save();
  ctx.globalCompositeOperation = 'soft-light';
  const side = r() > 0.4;
  const lg = ctx.createLinearGradient(side ? 0 : w, h * 0.1, side ? w : 0, h * 0.6);
  lg.addColorStop(0, css(o.grade.key, Math.min(1, s)));
  lg.addColorStop(1, css(o.grade.shadow, Math.min(1, s)));
  ctx.fillStyle = lg;
  ctx.fillRect(0, 0, w, h);
  ctx.globalCompositeOperation = 'color';
  ctx.fillStyle = css(o.grade.mid, 0.18 * s);
  ctx.fillRect(0, 0, w, h);
  ctx.globalCompositeOperation = 'screen';
  glow(ctx, side ? w * 0.05 : w * 0.95, h * (0.2 + r() * 0.2), w * 0.75, o.grade.key, 0.32 * s);
  ctx.globalCompositeOperation = 'multiply';
  const sh2 = ctx.createLinearGradient(side ? w : 0, 0, side ? w * 0.4 : w * 0.6, 0);
  sh2.addColorStop(0, css(o.grade.shadow, 0.35 * s));
  sh2.addColorStop(1, css(o.grade.shadow, 0));
  ctx.fillStyle = sh2;
  ctx.fillRect(0, 0, w, h);
  ctx.restore();
  vignette(ctx, w, h, 0.3);
  grain(ctx, w, h, 0.05);
  return c;
}

/** Four corner colours of an image, sorted dark to light: a crude palette for grading. */
export function sampleGrade(src: CanvasImageSource): Grade {
  const [, ctx] = makeCanvas(3, 3);
  ctx.drawImage(src, 0, 0, 3, 3);
  const d = ctx.getImageData(0, 0, 3, 3).data;
  const px: RGB[] = [];
  for (let i = 0; i < d.length; i += 4) px.push([d[i] ?? 0, d[i + 1] ?? 0, d[i + 2] ?? 0]);
  const lum = (c: RGB) => c[0] * 0.3 + c[1] * 0.59 + c[2] * 0.11;
  px.sort((a, b) => lum(a) - lum(b));
  const first = px[0] ?? [30, 30, 30];
  const last = px[px.length - 1] ?? [220, 220, 220];
  const middle = px[Math.floor(px.length / 2)] ?? [128, 128, 128];
  return { shadow: first, key: last, mid: middle };
}

export function scaled(src: HTMLCanvasElement | HTMLImageElement, longEdge: number): HTMLCanvasElement {
  const sw = src.width;
  const sh = src.height;
  const k = longEdge / Math.max(sw, sh, 1);
  const [c, ctx] = makeCanvas(Math.max(1, sw * k), Math.max(1, sh * k));
  ctx.imageSmoothingQuality = 'high';
  ctx.drawImage(src, 0, 0, c.width, c.height);
  return c;
}

export function toBlobUrl(c: HTMLCanvasElement, type = 'image/jpeg', quality = 0.9): Promise<string> {
  return new Promise((resolve, reject) => {
    c.toBlob((b) => (b ? resolve(URL.createObjectURL(b)) : reject(new Error('encode failed'))), type, quality);
  });
}

export async function loadImage(url: string): Promise<HTMLImageElement> {
  const img = new Image();
  img.decoding = 'async';
  img.src = url;
  await img.decode();
  return img;
}

export const GRADES = PALETTES;
