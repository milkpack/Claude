import type { PointerEvent as RPointerEvent } from 'react';
import { useRef, useState } from 'react';
import type { Asset } from '../api/types';
import type { Pt, SketchShape, SketchTool } from '../graph/types';

const W = 1000;

function render(s: SketchShape, h: number, i: number | string) {
  const k = Math.min(W, h) / 400;
  const sw = s.w * k;
  const P = (p: Pt) => [p[0] * W, p[1] * h] as const;
  const common = { stroke: s.c, strokeWidth: sw, fill: 'none', strokeLinecap: 'round' as const, strokeLinejoin: 'round' as const };
  if (s.t === 'pen') return <polyline key={i} points={s.pts.map((p) => P(p).join(',')).join(' ')} {...common} />;
  if (s.t === 'ellipse') {
    const [ax, ay] = P(s.a);
    const [bx, by] = P(s.b);
    return <ellipse key={i} cx={(ax + bx) / 2} cy={(ay + by) / 2} rx={Math.abs(bx - ax) / 2} ry={Math.abs(by - ay) / 2} {...common} />;
  }
  if (s.t === 'arrow') {
    const [ax, ay] = P(s.a);
    const [bx, by] = P(s.b);
    const ang = Math.atan2(by - ay, bx - ax);
    const head = Math.max(8, s.w * 3.5) * k;
    const l = [bx - head * Math.cos(ang - 0.45), by - head * Math.sin(ang - 0.45)];
    const r = [bx - head * Math.cos(ang + 0.45), by - head * Math.sin(ang + 0.45)];
    return <path key={i} d={`M${ax},${ay} L${bx},${by} M${l[0]},${l[1]} L${bx},${by} L${r[0]},${r[1]}`} {...common} />;
  }
  const [x, y] = P(s.at);
  return (
    <text
      key={i}
      x={x}
      y={y}
      fill={s.c}
      stroke="rgba(0,0,0,0.55)"
      strokeWidth={Math.max(2, s.w * 0.8 * k)}
      paintOrder="stroke"
      fontFamily="IBM Plex Sans, sans-serif"
      fontWeight={600}
      fontSize={(10 + s.w * 3) * k}
      dominantBaseline="hanging"
    >
      {s.s}
    </text>
  );
}

/** Draws vector marks over an image; coordinates are stored 0–1 so they survive any size. */
export function SketchPad({
  asset,
  shapes,
  tool,
  color,
  width,
  text,
  onAdd,
}: {
  asset: Asset;
  shapes: SketchShape[];
  tool: SketchTool;
  color: string;
  width: number;
  text: string;
  onAdd: (s: SketchShape) => void;
}) {
  const h = Math.round((W * asset.height) / Math.max(1, asset.width)) || W;
  const [draft, setDraft] = useState<SketchShape | null>(null);
  const ref = useRef<SVGSVGElement>(null);
  const pos = (e: RPointerEvent): Pt => {
    const r = e.currentTarget.getBoundingClientRect();
    return [Math.min(1, Math.max(0, (e.clientX - r.left) / r.width)), Math.min(1, Math.max(0, (e.clientY - r.top) / r.height))];
  };
  const down = (e: RPointerEvent<SVGSVGElement>) => {
    e.stopPropagation();
    const p = pos(e);
    if (tool === 'text') {
      if (text.trim()) onAdd({ t: 'text', at: p, s: text.trim().slice(0, 80), c: color, w: width });
      return;
    }
    e.currentTarget.setPointerCapture(e.pointerId);
    setDraft(tool === 'pen' ? { t: 'pen', pts: [p], c: color, w: width } : { t: tool, a: p, b: p, c: color, w: width });
  };
  const move = (e: RPointerEvent<SVGSVGElement>) => {
    if (!draft) return;
    const p = pos(e);
    if (draft.t === 'pen') {
      const last = draft.pts[draft.pts.length - 1];
      if (last && Math.hypot(p[0] - last[0], p[1] - last[1]) < 0.003) return;
      setDraft({ ...draft, pts: [...draft.pts, p] });
    } else if (draft.t === 'arrow' || draft.t === 'ellipse') setDraft({ ...draft, b: p });
  };
  const up = () => {
    if (!draft) return;
    const ok =
      draft.t === 'pen' ? draft.pts.length > 1 : draft.t === 'text' ? true : Math.hypot(draft.b[0] - draft.a[0], draft.b[1] - draft.a[1]) > 0.01;
    if (ok) onAdd(draft);
    setDraft(null);
  };
  return (
    <div className="sketch-wrap" style={{ aspectRatio: `${asset.width} / ${asset.height}` }}>
      <img src={asset.thumb} alt="" draggable={false} />
      <svg
        ref={ref}
        viewBox={`0 0 ${W} ${h}`}
        className={`sketch-svg nodrag nopan nowheel tool-${tool}`}
        onPointerDown={down}
        onPointerMove={move}
        onPointerUp={up}
        onPointerCancel={up}
        role="img"
        aria-label="Sketch surface"
      >
        {shapes.map((s, i) => render(s, h, i))}
        {draft ? render(draft, h, 'draft') : null}
      </svg>
    </div>
  );
}
