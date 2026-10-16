// Canvas helpers for masks and sketches (all drawn in code, exported as PNG files for upload).
import type { Asset } from '../api/types';
import type { MaskRect, SketchShape } from '../graph/types';

const MAX_EDGE = 2048;

export async function loadImg(url: string): Promise<HTMLImageElement> {
  const img = new Image();
  img.decoding = 'async';
  img.src = url;
  await img.decode();
  return img;
}

function outSize(a: { width: number; height: number }): { w: number; h: number } {
  const w = Number.isFinite(a.width) && a.width > 0 ? a.width : 1024;
  const h = Number.isFinite(a.height) && a.height > 0 ? a.height : 1024;
  const k = Math.min(1, MAX_EDGE / Math.max(w, h));
  return { w: Math.max(1, Math.round(w * k)), h: Math.max(1, Math.round(h * k)) };
}

function canvas(w: number, h: number): [HTMLCanvasElement, CanvasRenderingContext2D] {
  const c = document.createElement('canvas');
  c.width = w;
  c.height = h;
  const ctx = c.getContext('2d');
  if (!ctx) throw new Error('2D canvas is not available in this browser');
  return [c, ctx];
}

export function toFile(c: HTMLCanvasElement, name: string): Promise<File> {
  return new Promise((resolve, reject) =>
    c.toBlob((b) => (b ? resolve(new File([b], name, { type: 'image/png' })) : reject(new Error('Could not encode the PNG'))), 'image/png'),
  );
}

/** Black/white mask at the source's size (capped at 2048 px): white = the area to change. */
export async function renderMask(source: Asset, maskUrl?: string, rect?: MaskRect): Promise<File> {
  const { w, h } = outSize(source);
  const [c, ctx] = canvas(w, h);
  ctx.fillStyle = '#000';
  ctx.fillRect(0, 0, w, h);
  if (maskUrl) {
    const m = await loadImg(maskUrl);
    // The painted layer is translucent white over transparent: threshold it to solid white.
    const [mc, mctx] = canvas(w, h);
    mctx.drawImage(m, 0, 0, w, h);
    const d = mctx.getImageData(0, 0, w, h);
    for (let i = 0; i < d.data.length; i += 4) {
      const on = (d.data[i + 3] ?? 0) > 20;
      d.data[i] = d.data[i + 1] = d.data[i + 2] = on ? 255 : 0;
      d.data[i + 3] = 255;
    }
    mctx.putImageData(d, 0, 0);
    ctx.globalCompositeOperation = 'lighten';
    ctx.drawImage(mc, 0, 0);
    ctx.globalCompositeOperation = 'source-over';
  }
  if (rect) {
    ctx.fillStyle = '#fff';
    ctx.fillRect(rect.x * w, rect.y * h, rect.w * w, rect.h * h);
  }
  return toFile(c, 'mask.png');
}

export function drawShapes(ctx: CanvasRenderingContext2D, shapes: SketchShape[], w: number, h: number, scale = 1): void {
  const k = (Math.min(w, h) / 400) * scale;
  ctx.lineCap = 'round';
  ctx.lineJoin = 'round';
  for (const s of shapes) {
    ctx.strokeStyle = s.c;
    ctx.fillStyle = s.c;
    ctx.lineWidth = s.w * k;
    if (s.t === 'pen') {
      ctx.beginPath();
      s.pts.forEach(([x, y], i) => (i ? ctx.lineTo(x * w, y * h) : ctx.moveTo(x * w, y * h)));
      ctx.stroke();
    } else if (s.t === 'arrow') {
      const [ax, ay] = [s.a[0] * w, s.a[1] * h];
      const [bx, by] = [s.b[0] * w, s.b[1] * h];
      const ang = Math.atan2(by - ay, bx - ax);
      const head = Math.max(8, s.w * 3.5) * k;
      ctx.beginPath();
      ctx.moveTo(ax, ay);
      ctx.lineTo(bx, by);
      ctx.moveTo(bx - head * Math.cos(ang - 0.45), by - head * Math.sin(ang - 0.45));
      ctx.lineTo(bx, by);
      ctx.lineTo(bx - head * Math.cos(ang + 0.45), by - head * Math.sin(ang + 0.45));
      ctx.stroke();
    } else if (s.t === 'ellipse') {
      const cx = ((s.a[0] + s.b[0]) / 2) * w;
      const cy = ((s.a[1] + s.b[1]) / 2) * h;
      ctx.beginPath();
      ctx.ellipse(cx, cy, Math.abs(s.b[0] - s.a[0]) * w * 0.5, Math.abs(s.b[1] - s.a[1]) * h * 0.5, 0, 0, Math.PI * 2);
      ctx.stroke();
    } else {
      ctx.font = `600 ${Math.round((10 + s.w * 3) * k)}px "IBM Plex Sans", sans-serif`;
      ctx.textBaseline = 'top';
      ctx.lineWidth = Math.max(2, s.w * 0.8 * k);
      ctx.strokeStyle = 'rgba(0,0,0,0.55)';
      ctx.strokeText(s.s, s.at[0] * w, s.at[1] * h);
      ctx.fillText(s.s, s.at[0] * w, s.at[1] * h);
    }
  }
}

/** The source with the marks burned in, and the marks alone on transparent. */
export async function flattenSketch(source: Asset, shapes: SketchShape[]): Promise<{ flat: File; overlay: File }> {
  const { w, h } = outSize(source);
  const img = await loadImg(source.file);
  const [fc, fctx] = canvas(w, h);
  fctx.drawImage(img, 0, 0, w, h);
  drawShapes(fctx, shapes, w, h);
  const [oc, octx] = canvas(w, h);
  drawShapes(octx, shapes, w, h);
  return { flat: await toFile(fc, 'sketch.png'), overlay: await toFile(oc, 'sketch-overlay.png') };
}
