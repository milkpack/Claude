import type { PointerEvent as RPointerEvent } from 'react';
import { useEffect, useRef, useState } from 'react';
import type { Asset } from '../api/types';
import type { MaskRect } from '../graph/types';

const RES = 512;

/** Paint a mask (or drag a rectangle) over an image inside a node. White/alpha = change. */
export function MaskPainter({
  asset,
  mode,
  brush,
  maskUrl,
  rect,
  onMask,
  onRect,
}: {
  asset: Asset;
  mode: 'paint' | 'rect';
  brush: number;
  maskUrl?: string;
  rect?: MaskRect;
  onMask: (url: string | undefined) => void;
  onRect: (r: MaskRect | undefined) => void;
}) {
  const ratio = asset.width > 0 && asset.height > 0 ? asset.width / asset.height : 1;
  const w = RES;
  const h = Math.max(1, Math.round(RES / ratio));
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const drawing = useRef<{ last: [number, number] } | null>(null);
  const [drag, setDrag] = useState<{ a: [number, number]; b: [number, number] } | null>(null);

  // Show the saved mask.
  useEffect(() => {
    const c = canvasRef.current;
    const ctx = c?.getContext('2d');
    if (!c || !ctx) return;
    ctx.clearRect(0, 0, c.width, c.height);
    if (!maskUrl) return;
    const img = new Image();
    img.onload = () => {
      ctx.clearRect(0, 0, c.width, c.height);
      ctx.drawImage(img, 0, 0, c.width, c.height);
    };
    img.src = maskUrl;
  }, [maskUrl, w, h]);

  const pos = (e: RPointerEvent): [number, number] => {
    const r = e.currentTarget.getBoundingClientRect();
    return [Math.min(1, Math.max(0, (e.clientX - r.left) / r.width)), Math.min(1, Math.max(0, (e.clientY - r.top) / r.height))];
  };

  const stroke = (a: [number, number], b: [number, number]) => {
    const ctx = canvasRef.current?.getContext('2d');
    if (!ctx) return;
    ctx.strokeStyle = '#e5484d'; // quick-mask red; the export keys on alpha
    ctx.lineCap = 'round';
    ctx.lineJoin = 'round';
    ctx.lineWidth = brush;
    ctx.beginPath();
    ctx.moveTo(a[0] * w, a[1] * h);
    ctx.lineTo(b[0] * w + 0.01, b[1] * h);
    ctx.stroke();
  };

  const down = (e: RPointerEvent) => {
    e.stopPropagation();
    e.currentTarget.setPointerCapture(e.pointerId);
    const p = pos(e);
    if (mode === 'paint') {
      drawing.current = { last: p };
      stroke(p, p);
    } else setDrag({ a: p, b: p });
  };
  const move = (e: RPointerEvent) => {
    const p = pos(e);
    if (mode === 'paint' && drawing.current) {
      stroke(drawing.current.last, p);
      drawing.current.last = p;
    } else if (drag) setDrag({ ...drag, b: p });
  };
  const up = () => {
    if (mode === 'paint' && drawing.current) {
      drawing.current = null;
      onMask(canvasRef.current?.toDataURL('image/png'));
    } else if (drag) {
      const x = Math.min(drag.a[0], drag.b[0]);
      const y = Math.min(drag.a[1], drag.b[1]);
      const rw = Math.abs(drag.b[0] - drag.a[0]);
      const rh = Math.abs(drag.b[1] - drag.a[1]);
      setDrag(null);
      onRect(rw > 0.01 && rh > 0.01 ? { x, y, w: rw, h: rh } : undefined);
    }
  };

  const shownRect = drag
    ? { x: Math.min(drag.a[0], drag.b[0]), y: Math.min(drag.a[1], drag.b[1]), w: Math.abs(drag.b[0] - drag.a[0]), h: Math.abs(drag.b[1] - drag.a[1]) }
    : rect;

  return (
    <div className="mask-wrap" style={{ aspectRatio: `${asset.width} / ${asset.height}` }}>
      <img src={asset.thumb} alt="" draggable={false} />
      <canvas ref={canvasRef} width={w} height={h} className={`mask-canvas ${mode === 'paint' ? '' : 'is-dim'}`} aria-hidden />
      {mode === 'rect' && shownRect ? (
        <span className="mask-rect" style={{ left: `${shownRect.x * 100}%`, top: `${shownRect.y * 100}%`, width: `${shownRect.w * 100}%`, height: `${shownRect.h * 100}%` }} />
      ) : null}
      <div
        className={`mask-surface nodrag nopan nowheel mode-${mode}`}
        onPointerDown={down}
        onPointerMove={move}
        onPointerUp={up}
        onPointerCancel={up}
        role="img"
        aria-label={mode === 'paint' ? 'Paint the area to change' : 'Drag a rectangle around the area to change'}
      />
    </div>
  );
}
