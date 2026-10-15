import type { NodeProps } from '@xyflow/react';
import { Maximize2, Trophy } from 'lucide-react';
import type { AppNode, CompareData } from '../../graph/types';
import { useGraph } from '../../graph/store';
import { useNodeInputs } from '../../graph/hooks';
import { useUi } from '../../state/ui';
import { gridCols } from '../ResultsGrid';
import { NodeShell } from './NodeShell';

export function CompareNode({ id, data, selected }: NodeProps<Extract<AppNode, { type: 'compare' }>>) {
  const d = data as CompareData;
  const ins = useNodeInputs(id);
  const images = ins.images.images ?? [];
  const branches = new Set(images.map((i) => i.from)).size;
  return (
    <NodeShell id={id} kind="compare" selected={selected} title={d.title} exposed={d.exposed} done={Boolean(d.winner)}>
      {!images.length ? (
        <p className="hint">Wire two or more branches in, then pick the winner.</p>
      ) : (
        <>
          <p className="hint">
            {images.length} images from {branches} branch{branches === 1 ? '' : 'es'} · {d.winner ? 'winner picked' : 'click to pick the winner'}
          </p>
          <div className="results compare" style={{ gridTemplateColumns: `repeat(${gridCols(images.length)}, 1fr)` }}>
            {images.map((img, i) => (
              <div key={img.asset.id} className={`result ${img.asset.id === d.winner ? 'is-winner' : ''}`} style={{ aspectRatio: `${img.asset.width} / ${img.asset.height}` }}>
                <button
                  type="button"
                  className="result-open nodrag"
                  aria-pressed={img.asset.id === d.winner}
                  aria-label={`Pick image ${i + 1} as the winner`}
                  onClick={() => useGraph.getState().updateData(id, { winner: img.asset.id === d.winner ? undefined : img.asset.id })}
                >
                  <img src={img.asset.thumb} alt="" draggable={false} />
                </button>
                {img.asset.id === d.winner ? (
                  <span className="winner-tag">
                    <Trophy size={10} aria-hidden /> winner
                  </span>
                ) : null}
                <button
                  type="button"
                  className="result-pick nodrag"
                  aria-label="Open full size"
                  onClick={() => useUi.getState().openViewer({ assets: images.map((x) => x.asset), index: i, fromNode: id })}
                >
                  <Maximize2 size={10} aria-hidden />
                </button>
              </div>
            ))}
          </div>
        </>
      )}
    </NodeShell>
  );
}
