import { useState } from 'react';
import type { NodeProps } from '@xyflow/react';
import { ArrowUpRight, Circle, Pencil, Redo2, Save, Trash, Type, Undo2 } from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import { api } from '../../api/client';
import { describeError } from '../../api/errors';
import { qk, queryClient } from '../../api/queries';
import type { AppNode, SketchData, SketchShape, SketchTool } from '../../graph/types';
import { useGraph } from '../../graph/store';
import { useNodeInputs } from '../../graph/hooks';
import { flattenSketch } from '../../lib/image';
import { hash, stableStringify } from '../../lib/util';
import { toast } from '../../state/ui';
import { Button, IconButton, SectionLabel } from '../../components/ui';
import { NodeShell } from './NodeShell';
import { TaskCard } from '../TaskCard';
import { ResultDrop, ResultThumbs } from '../HandoffBits';
import { SketchPad } from '../SketchPad';

const TOOLS: { id: SketchTool; icon: LucideIcon; label: string }[] = [
  { id: 'pen', icon: Pencil, label: 'Pen' },
  { id: 'arrow', icon: ArrowUpRight, label: 'Arrow' },
  { id: 'ellipse', icon: Circle, label: 'Circle' },
  { id: 'text', icon: Type, label: 'Text' },
];
// Annotation colours: marks must stand out from any photograph.
const COLORS = ['#e5484d', '#f5c542', '#3fb6c9', '#ffffff', '#111111'];
const WIDTHS = [2, 4, 8];

export const shapesKey = (s: SketchShape[]) => hash(stableStringify(s));

export function SketchNode({ id, data, selected }: NodeProps<Extract<AppNode, { type: 'sketch' }>>) {
  const d = data as SketchData;
  const ins = useNodeInputs(id);
  const source = ins.images.image?.[0]?.asset;
  const [text, setText] = useState('1');
  const [saving, setSaving] = useState(false);
  const g = useGraph.getState();
  const dirty = d.shapes.length > 0 && d.savedKey !== shapesKey(d.shapes);

  const save = async () => {
    if (!source) return;
    setSaving(true);
    try {
      const { flat, overlay } = await flattenSketch(source, d.shapes);
      const [f, o] = [await api.uploadAsset(flat), await api.uploadAsset(overlay)];
      useGraph.getState().updateData(id, { flattened: f, overlay: o, savedKey: shapesKey(d.shapes), error: undefined });
      void queryClient.invalidateQueries({ queryKey: qk.assets });
      toast('Marked image saved to the library', 'ok');
    } catch (e) {
      const v = describeError(e);
      useGraph.getState().updateData(id, { error: { title: 'Could not save the sketch', message: v.detail, code: v.code } }, 'none');
    } finally {
      setSaving(false);
    }
  };

  return (
    <NodeShell id={id} kind="sketch" selected={selected} title={d.title} exposed={d.exposed} error={d.error} done={Boolean(d.flattened)}>
      <ResultDrop id={id}>
        {source ? (
          <>
            <div className="sketch-tools" role="toolbar" aria-label="Sketch tools">
              {TOOLS.map((t) => (
                <IconButton key={t.id} icon={t.icon} label={t.label} size={14} active={d.tool === t.id} className="nodrag" onClick={() => g.updateData(id, { tool: t.id }, 'none')} />
              ))}
              <span className="sep" />
              {COLORS.map((c) => (
                <button
                  key={c}
                  type="button"
                  className={`swatch nodrag ${d.color === c ? 'is-on' : ''}`}
                  style={{ background: c }}
                  aria-label={`Colour ${c}`}
                  aria-pressed={d.color === c}
                  onClick={() => g.updateData(id, { color: c }, 'none')}
                />
              ))}
              <span className="sep" />
              {WIDTHS.map((w) => (
                <button key={w} type="button" className={`width-btn nodrag ${d.width === w ? 'is-on' : ''}`} aria-label={`Stroke ${w}`} aria-pressed={d.width === w} onClick={() => g.updateData(id, { width: w }, 'none')}>
                  <span style={{ height: Math.max(1.5, w * 0.75) }} />
                </button>
              ))}
            </div>
            <div className="sketch-tools">
              <IconButton icon={Undo2} label="Undo mark" size={14} className="nodrag" disabled={!d.shapes.length} onClick={() => g.updateData(id, { shapes: d.shapes.slice(0, -1), redo: [...d.redo, ...d.shapes.slice(-1)] })} />
              <IconButton icon={Redo2} label="Redo mark" size={14} className="nodrag" disabled={!d.redo.length} onClick={() => g.updateData(id, { shapes: [...d.shapes, ...d.redo.slice(-1)], redo: d.redo.slice(0, -1) })} />
              <IconButton icon={Trash} label="Clear marks" size={14} className="nodrag" disabled={!d.shapes.length} onClick={() => g.updateData(id, { shapes: [], redo: [] })} />
              {d.tool === 'text' ? (
                <input className="input nodrag sketch-text" value={text} maxLength={80} aria-label="Text to place" placeholder="text, then click" onChange={(e) => setText(e.target.value)} />
              ) : (
                <span className="hint mono sketch-count">{d.shapes.length} mark{d.shapes.length === 1 ? '' : 's'}</span>
              )}
              <span className="spacer" />
              <Button size="sm" icon={Save} className="nodrag" busy={saving} disabled={!d.shapes.length || !dirty} onClick={save}>
                {dirty || !d.flattened ? 'Save' : 'Saved'}
              </Button>
            </div>
            <SketchPad
              asset={source}
              shapes={d.shapes}
              tool={d.tool}
              color={d.color}
              width={d.width}
              text={text}
              onAdd={(s) => g.updateData(id, { shapes: [...d.shapes, s], redo: [] })}
            />
          </>
        ) : (
          <div className="output-empty">Connect an image to draw on.</div>
        )}
        <SectionLabel>Notes</SectionLabel>
        <textarea
          className="textarea nodrag nowheel"
          rows={2}
          value={d.notes ?? ''}
          placeholder="1 — stray hair over the eye; 2 — collar fold…"
          aria-label="Notes"
          onChange={(e) => g.updateData(id, { notes: e.target.value }, 'coalesce')}
        />
        <div style={{ marginTop: 10 }}>
          <TaskCard id={id} kind="sketch" task={d.handoff} applied={d.handoffApplied} createLabel="Ask Claude to mark issues" acceptsFiles />
        </div>
        <ResultThumbs id={id} assets={d.results} label="Marked by Claude" />
      </ResultDrop>
    </NodeShell>
  );
}
