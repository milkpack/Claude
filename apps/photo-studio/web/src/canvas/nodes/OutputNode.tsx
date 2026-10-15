import type { NodeProps } from '@xyflow/react';
import { Copy, Download, Library, Maximize2 } from 'lucide-react';
import type { AppNode, OutputData } from '../../graph/types';
import { useGraph } from '../../graph/store';
import { useNodeInputs } from '../../graph/hooks';
import { useUi, toast } from '../../state/ui';
import { assetFilename, copyText, downloadUrl } from '../../lib/util';
import { IconButton } from '../../components/ui';
import { NodeShell } from './NodeShell';

export function OutputNode({ id, data, selected }: NodeProps<Extract<AppNode, { type: 'output' }>>) {
  const d = data as OutputData;
  const ins = useNodeInputs(id);
  const images = (ins.images.image ?? []).map((i) => i.asset);
  const cur = images.find((a) => a.id === d.current) ?? images[0];
  const upstreamRunning = useGraph((s) => Object.keys(s.runtime).some((k) => s.edges.some((e) => e.target === id && e.source === k)));
  return (
    <NodeShell id={id} kind="output" selected={selected} title={d.title} exposed={d.exposed} run="to-here" done={images.length > 0}>
      {!cur ? (
        <div className="output-empty">{upstreamRunning ? 'Waiting for the upstream run…' : 'Nothing yet. Run to here to make the picture.'}</div>
      ) : (
        <>
          <button
            type="button"
            className="output-main nodrag"
            style={{ aspectRatio: `${cur.width} / ${cur.height}` }}
            aria-label="Open full size"
            onClick={() => useUi.getState().openViewer({ assets: images, index: images.indexOf(cur), fromNode: id })}
          >
            <img src={cur.thumb} alt={cur.prompt ?? 'Output'} draggable={false} />
          </button>
          {images.length > 1 ? (
            <div className="output-strip nowheel">
              {images.map((a) => (
                <button
                  type="button"
                  key={a.id}
                  className={`output-thumb nodrag ${a.id === cur.id ? 'is-on' : ''}`}
                  aria-pressed={a.id === cur.id}
                  aria-label="Show this image"
                  onClick={() => useGraph.getState().updateData(id, { current: a.id }, 'none')}
                >
                  <img src={a.thumb} alt="" draggable={false} />
                </button>
              ))}
            </div>
          ) : null}
          <div className="output-actions">
            <span className="mono muted">
              {cur.width}×{cur.height}
            </span>
            <span className="spacer" />
            <IconButton icon={Maximize2} label="Full size" className="nodrag" onClick={() => useUi.getState().openViewer({ assets: images, index: images.indexOf(cur), fromNode: id })} />
            <IconButton icon={Download} label="Download" className="nodrag" onClick={() => downloadUrl(cur.file, assetFilename(cur))} />
            <IconButton
              icon={Copy}
              label="Copy prompt"
              className="nodrag"
              disabled={!cur.prompt}
              onClick={async () => toast((await copyText(cur.prompt ?? '')) ? 'Prompt copied' : 'Could not copy', 'info')}
            />
            <IconButton
              icon={Library}
              label="In the library: show it"
              className="nodrag"
              onClick={() => {
                useUi.getState().setRight(true, 'library');
                toast('Every generation is saved in the library');
              }}
            />
          </div>
        </>
      )}
    </NodeShell>
  );
}
