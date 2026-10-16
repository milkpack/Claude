import type { DragEvent, ReactNode } from 'react';
import { useState } from 'react';
import type { Asset } from '../api/types';
import { uploadResults } from '../graph/handoff';
import { useUi } from '../state/ui';
import { gridCols } from './ResultsGrid';

/** Wraps a node body so result images dropped from the OS go to the node's task. */
export function ResultDrop({ id, children }: { id: string; children: ReactNode }) {
  const [over, setOver] = useState(false);
  const onDrop = (e: DragEvent) => {
    const files = Array.from(e.dataTransfer.files);
    if (!files.length) return;
    e.preventDefault();
    e.stopPropagation();
    setOver(false);
    void uploadResults(id, files);
  };
  return (
    <div
      className={`result-drop ${over ? 'is-over' : ''}`}
      onDragOver={(e) => {
        if (e.dataTransfer.types.includes('Files')) {
          e.preventDefault();
          e.stopPropagation();
          setOver(true);
        }
      }}
      onDragLeave={(e) => {
        if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setOver(false);
      }}
      onDrop={onDrop}
    >
      {children}
      {over ? <div className="result-drop-hint">Drop to return the results</div> : null}
    </div>
  );
}

export function ResultThumbs({ id, assets, label = 'Results' }: { id: string; assets: Asset[]; label?: string }) {
  if (!assets.length) return null;
  return (
    <div className="results-wrap">
      <div className="section-label">
        <span>{label}</span>
        <span className="section-aside mono">{assets.length}</span>
      </div>
      <div className="results" style={{ gridTemplateColumns: `repeat(${Math.min(3, gridCols(assets.length))}, 1fr)`, marginTop: 0 }}>
        {assets.map((a, i) => (
          <div key={a.id} className="result" style={{ aspectRatio: `${a.width} / ${a.height}` }}>
            <button type="button" className="result-open nodrag" aria-label={`Open result ${i + 1}`} onClick={() => useUi.getState().openViewer({ assets, index: i, fromNode: id })}>
              <img src={a.thumb} alt="" draggable={false} />
            </button>
          </div>
        ))}
      </div>
    </div>
  );
}
