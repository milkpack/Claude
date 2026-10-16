import type { NodeProps } from '@xyflow/react';
import { Eraser } from 'lucide-react';
import type { AppNode, PhotoshopData } from '../../graph/types';
import { useGraph } from '../../graph/store';
import { useNodeInputs } from '../../graph/hooks';
import { Button, Chips, SectionLabel } from '../../components/ui';
import { NodeShell } from './NodeShell';
import { TaskCard } from '../TaskCard';
import { ResultDrop, ResultThumbs } from '../HandoffBits';
import { MaskPainter } from '../MaskPainter';

const MODES = ['paint', 'rect'] as const;
const BRUSHES = ['12', '24', '48'] as const;

export function PhotoshopNode({ id, data, selected }: NodeProps<Extract<AppNode, { type: 'photoshop' }>>) {
  const d = data as PhotoshopData;
  const ins = useNodeInputs(id);
  const source = ins.images.source?.[0]?.asset;
  const wired = (ins.text.prompt ?? []).join('\n\n');
  const g = useGraph.getState();
  const hasMask = d.maskMode === 'paint' ? Boolean(d.maskUrl) : Boolean(d.maskRect);
  return (
    <NodeShell id={id} kind="photoshop" selected={selected} title={d.title} exposed={d.exposed} error={d.error} done={d.results.length > 0}>
      <ResultDrop id={id}>
        {source ? (
          <>
            <div className="mask-tools">
              <Chips options={MODES} value={d.maskMode} onChange={(maskMode) => g.updateData(id, { maskMode })} label="Mask" format={(m) => (m === 'paint' ? 'brush' : 'rectangle')} />
              {d.maskMode === 'paint' ? (
                <Chips options={BRUSHES} value={String(d.brush) as (typeof BRUSHES)[number]} onChange={(b) => g.updateData(id, { brush: Number(b) })} label="Brush size" />
              ) : null}
              <span className="spacer" />
              <Button size="sm" variant="ghost" icon={Eraser} className="nodrag" disabled={!hasMask} onClick={() => g.updateData(id, d.maskMode === 'paint' ? { maskUrl: undefined } : { maskRect: undefined })}>
                Clear
              </Button>
            </div>
            <MaskPainter
              asset={source}
              mode={d.maskMode}
              brush={d.brush}
              maskUrl={d.maskUrl}
              rect={d.maskRect}
              onMask={(maskUrl) => g.updateData(id, { maskUrl })}
              onRect={(maskRect) => g.updateData(id, { maskRect })}
            />
            <p className="hint mono" style={{ marginTop: 4 }}>
              {hasMask ? 'mask set · only the marked area may change' : 'no mask · the instruction must name the area'}
            </p>
          </>
        ) : (
          <div className="output-empty">Connect the image to edit into source.</div>
        )}
        <SectionLabel>Change only</SectionLabel>
        <textarea
          className="textarea nodrag nowheel"
          rows={3}
          value={d.instruction}
          placeholder="e.g. remove the stray hair over the left eye; keep skin texture"
          aria-label="Change only"
          onChange={(e) => g.updateData(id, { instruction: e.target.value }, 'coalesce')}
        />
        {wired ? (
          <div className="wired-text nowheel" style={{ marginTop: 6 }}>
            <span className="wired-from type-text">from wire</span>
            <p>{wired}</p>
          </div>
        ) : null}
        <div style={{ marginTop: 10 }}>
          <TaskCard id={id} kind="photoshop" task={d.handoff} applied={d.handoffApplied} createLabel="Create Photoshop task" acceptsFiles />
        </div>
        <ResultThumbs id={id} assets={d.results} label="Edited" />
      </ResultDrop>
    </NodeShell>
  );
}
