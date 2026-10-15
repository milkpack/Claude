import type { NodeProps } from '@xyflow/react';
import { Wrench, SquarePen } from 'lucide-react';
import type { Asset, ReviewResponse, Verdict } from '../../api/types';
import type { AppNode, ReviewData } from '../../graph/types';
import { useGraph } from '../../graph/store';
import { useNodeInputs, useRuntime } from '../../graph/hooks';
import { runNode } from '../../graph/run';
import { applyFix, editReviewedImage } from '../../graph/actions';
import { useMe } from '../../api/queries';
import { useUi } from '../../state/ui';
import { Button, SectionLabel } from '../../components/ui';
import { NodeShell } from './NodeShell';

const SCORE_LABELS: [keyof ReviewResponse['scores'], string][] = [
  ['realism', 'Realism'],
  ['skin', 'Skin'],
  ['light', 'Light'],
  ['anatomy', 'Anatomy'],
  ['brief', 'Brief'],
  ['identity', 'Identity'],
  ['preserve', 'Preserve'],
];

export function VerdictBadge({ v }: { v: Verdict }) {
  return <span className={`verdict verdict-${v}`}>{v}</span>;
}

function Scores({ r }: { r: ReviewResponse }) {
  return (
    <div className="scores">
      {SCORE_LABELS.map(([k, label]) => {
        const v = r.scores[k];
        if (typeof v !== 'number') return null;
        const n = Math.max(0, Math.min(5, Math.round(v)));
        return (
          <div className="score" key={k}>
            <span className="score-label">{label}</span>
            <span className="score-bar" aria-hidden>
              {Array.from({ length: 5 }, (_, i) => (
                <span key={i} className={i < n ? `on s${n}` : ''} />
              ))}
            </span>
            <span className="mono score-n" aria-label={`${label} ${v} of 5`}>
              {v}
            </span>
          </div>
        );
      })}
    </div>
  );
}

export function ReviewNode({ id, data, selected }: NodeProps<Extract<AppNode, { type: 'review' }>>) {
  const d = data as ReviewData;
  const ins = useNodeInputs(id);
  const rt = useRuntime(id);
  const me = useMe();
  const images = ins.images.image ?? [];
  const assetsById = new Map<string, Asset>(images.map((i) => [i.asset.id, i.asset]));
  const cur = d.current && d.reviews[d.current] ? d.current : d.order[0];
  const r = cur ? d.reviews[cur] : undefined;
  const curAsset = cur ? assetsById.get(cur) : undefined;
  const counts = d.order.reduce<Record<string, number>>((acc, aid) => {
    const v = d.reviews[aid]?.verdict;
    if (v) acc[v] = (acc[v] ?? 0) + 1;
    return acc;
  }, {});

  return (
    <NodeShell
      id={id}
      kind="review"
      selected={selected}
      title={d.title}
      exposed={d.exposed}
      error={d.error}
      run="node"
      done={d.order.length > 0}
      phase={rt?.total ? `${(rt.done ?? 0) + 1} / ${rt.total}` : undefined}
    >
      {me.data && !me.data.claude.configured ? <p className="hint warn">Claude is not configured on the server, so Review cannot run.</p> : null}
      {!d.order.length ? (
        <>
          <p className="hint">
            {images.length ? `${images.length} image${images.length === 1 ? '' : 's'} to review${ins.images.source?.length ? ' against the source' : ''}.` : 'Connect generated images (and the source for edits).'}
          </p>
          <Button variant="primary" size="sm" className="nodrag full" busy={Boolean(rt)} disabled={Boolean(rt) || !images.length} onClick={() => void runNode(id)}>
            {rt ? 'Reviewing' : 'Review'}
          </Button>
        </>
      ) : (
        <>
          <div className="review-summary mono">
            {(['accept', 'retry', 'rethink'] as const).map((v) => (counts[v] ? <span key={v} className={`verdict-count verdict-${v}`}>{counts[v]} {v}</span> : null))}
          </div>
          <div className="review-strip">
            {d.order.map((aid, i) => {
              const a = assetsById.get(aid);
              const v = d.reviews[aid]?.verdict;
              return (
                <button
                  key={aid}
                  type="button"
                  className={`review-thumb nodrag ${aid === cur ? 'is-on' : ''}`}
                  onClick={() => useGraph.getState().updateData(id, { current: aid }, 'none')}
                  aria-label={`Image ${i + 1}: ${v ?? 'not reviewed'}`}
                  aria-pressed={aid === cur}
                >
                  {a ? <img src={a.thumb} alt="" draggable={false} /> : <span className="mono">{i + 1}</span>}
                  {v ? <span className={`review-dot verdict-${v}`} /> : null}
                </button>
              );
            })}
          </div>
          {r ? (
            <>
              <div className="review-head">
                <VerdictBadge v={r.verdict} />
                {curAsset ? (
                  <button type="button" className="link nodrag" onClick={() => useUi.getState().openViewer({ assets: d.order.map((x) => assetsById.get(x)).filter((x): x is Asset => Boolean(x)), index: Math.max(0, d.order.indexOf(cur ?? '')), fromNode: id })}>
                    view image
                  </button>
                ) : null}
              </div>
              <Scores r={r} />
              {r.defects.length ? (
                <>
                  <SectionLabel>Defects</SectionLabel>
                  <ul className="defects">
                    {r.defects.map((df, i) => (
                      <li key={i}>
                        <span className={`sev sev-${df.severity}`}>{df.severity}</span>
                        <span>
                          <strong>{df.area}</strong> {df.problem}
                        </span>
                      </li>
                    ))}
                  </ul>
                </>
              ) : (
                <p className="hint">No defects found.</p>
              )}
              {r.fix ? (
                <div className="fix">
                  <SectionLabel>Fix</SectionLabel>
                  <p className="fix-prompt">{r.fix.prompt}</p>
                  <p className="hint">{r.fix.why}</p>
                  <div className="fix-actions">
                    <Button size="sm" variant="primary" icon={Wrench} className="nodrag" onClick={() => applyFix(id, cur)}>
                      Apply fix
                    </Button>
                    {curAsset ? (
                      <Button size="sm" icon={SquarePen} className="nodrag" onClick={() => editReviewedImage(id, curAsset)} title="Edit this image with the fix instead of re-running">
                        Edit this image
                      </Button>
                    ) : null}
                  </div>
                </div>
              ) : null}
            </>
          ) : null}
        </>
      )}
    </NodeShell>
  );
}
