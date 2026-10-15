import { Check, ChevronLeft, ChevronRight } from 'lucide-react';
import type { GenData } from '../graph/types';
import { useGraph } from '../graph/store';
import { currentRun } from '../graph/evaluate';
import { useUi } from '../state/ui';
import { formatCredits, formatElapsed, timeMs } from '../lib/util';
import { IconButton } from '../components/ui';

function ratioOf(params: Record<string, unknown>): number {
  const m = /^(\d+(?:\.\d+)?):(\d+(?:\.\d+)?)$/.exec(String(params.aspectRatio ?? ''));
  const r = m ? Number(m[1]) / Number(m[2]) : 1;
  return Number.isFinite(r) && r > 0 ? r : 1;
}

export function gridCols(n: number): number {
  if (n <= 1) return 1;
  if (n === 2 || n === 4) return 2;
  if (n === 3 || n <= 6) return 3;
  return 4;
}

export function ResultsGrid({ id, d, running, expected }: { id: string; d: GenData; running: boolean; expected: number }) {
  const run = currentRun(d);
  const index = d.runIndex ?? d.runs.length - 1;
  const picked = new Set(d.picked ?? []);
  if (running) {
    const n = Math.max(1, Math.min(9, expected));
    const r = ratioOf(d.params);
    return (
      <div className="results is-pending" style={{ gridTemplateColumns: `repeat(${gridCols(n)}, 1fr)` }} aria-label="Generating">
        {Array.from({ length: n }, (_, i) => (
          <span key={i} className="result-skeleton" style={{ aspectRatio: String(r) }} />
        ))}
      </div>
    );
  }
  if (!run) return null;
  const assets = run.assets;
  const togglePick = (aid: string) => {
    const next = new Set(picked);
    if (next.has(aid)) next.delete(aid);
    else next.add(aid);
    useGraph.getState().updateData(id, { picked: [...next] });
  };
  const took = run.finishedAt ? timeMs(run.finishedAt) - timeMs(run.createdAt) : undefined;
  return (
    <div className="results-wrap">
      <div className="results" style={{ gridTemplateColumns: `repeat(${gridCols(assets.length)}, 1fr)` }}>
        {assets.map((a, i) => (
          <div key={a.id} className={`result ${picked.has(a.id) ? 'is-picked' : ''}`} style={{ aspectRatio: `${a.width} / ${a.height}` }}>
            <button
              type="button"
              className="result-open nodrag"
              aria-label={`Open image ${i + 1} of ${assets.length}`}
              onClick={() => useUi.getState().openViewer({ assets, index: i, fromNode: id })}
            >
              <img src={a.thumb} alt="" loading="lazy" decoding="async" draggable={false} />
            </button>
            <button
              type="button"
              className="result-pick nodrag"
              aria-pressed={picked.has(a.id)}
              aria-label={picked.has(a.id) ? 'Unpick' : 'Pick for downstream'}
              title={picked.has(a.id) ? 'Picked: flows downstream' : 'Pick: only picked images flow downstream'}
              onClick={() => togglePick(a.id)}
            >
              <Check size={11} aria-hidden />
            </button>
          </div>
        ))}
      </div>
      <div className="results-foot mono">
        {d.runs.length > 1 ? (
          <span className="pager">
            <IconButton icon={ChevronLeft} label="Previous run" size={12} className="nodrag" disabled={index <= 0} onClick={() => useGraph.getState().updateData(id, { runIndex: index - 1, picked: [] })} />
            run {index + 1}/{d.runs.length}
            <IconButton icon={ChevronRight} label="Next run" size={12} className="nodrag" disabled={index >= d.runs.length - 1} onClick={() => useGraph.getState().updateData(id, { runIndex: index + 1, picked: [] })} />
          </span>
        ) : null}
        <span>{assets.length} img</span>
        {run.credits !== undefined ? <span>{formatCredits(run.credits)} cr</span> : null}
        {took !== undefined && took > 0 ? <span>{formatElapsed(took)}</span> : null}
        <span className="spacer" />
        {picked.size ? (
          <button type="button" className="link nodrag" onClick={() => useGraph.getState().updateData(id, { picked: [] })}>
            {picked.size} picked · clear
          </button>
        ) : (
          <span className="muted">all flow on</span>
        )}
      </div>
    </div>
  );
}
