import type { NodeProps } from '@xyflow/react';
import type { AppNode, BlenderData } from '../../graph/types';
import { useGraph } from '../../graph/store';
import { useNodeInputs } from '../../graph/hooks';
import { SectionLabel } from '../../components/ui';
import { NodeShell } from './NodeShell';
import { TaskCard } from '../TaskCard';
import { ResultDrop, ResultThumbs } from '../HandoffBits';

const VIEWS = ['front', 'three-quarter', 'side', 'top', 'detail'] as const;

export function BlenderNode({ id, data, selected }: NodeProps<Extract<AppNode, { type: 'blender' }>>) {
  const d = data as BlenderData;
  const ins = useNodeInputs(id);
  const up = (patch: Partial<BlenderData>) => useGraph.getState().updateData(id, patch, 'coalesce');
  const field = (key: 'set' | 'camera' | 'lens' | 'light', label: string, placeholder: string, mono = false) => (
    <label className="param">
      <span className="param-label">{label}</span>
      <input className={`input nodrag ${mono ? 'mono' : ''}`} value={d[key]} placeholder={placeholder} onChange={(e) => up({ [key]: e.target.value })} />
    </label>
  );
  const refs = ins.images.refs ?? [];
  return (
    <NodeShell id={id} kind="blender" selected={selected} title={d.title} exposed={d.exposed} error={d.error} done={d.results.length > 0}>
      <ResultDrop id={id}>
        <SectionLabel>Scene</SectionLabel>
        <textarea
          className="textarea nodrag nowheel"
          rows={2}
          value={d.subject}
          placeholder="Subject: what stands in the set (e.g. the amber bottle, 1:1 scale)"
          aria-label="Subject"
          onChange={(e) => up({ subject: e.target.value })}
        />
        <div className="params">
          {field('set', 'Set', 'wet slate, dark seamless')}
          {field('camera', 'Camera', 'eye level, 1.2 m, slight high angle')}
          {field('lens', 'Lens', '100 mm', true)}
          {field('light', 'Light', 'raking backlight, soft fill left')}
          <div className="param">
            <span className="param-label">Views</span>
            <div className="param-control">
              <div className="chips nodrag" role="group" aria-label="Views">
                {VIEWS.map((v) => {
                  const on = d.views.includes(v);
                  return (
                    <button
                      key={v}
                      type="button"
                      aria-pressed={on}
                      className={`chip ${on ? 'is-on' : ''}`}
                      onClick={() => useGraph.getState().updateData(id, { views: on ? d.views.filter((x) => x !== v) : [...d.views, v] })}
                    >
                      {v === 'three-quarter' ? '¾' : v}
                    </button>
                  );
                })}
              </div>
            </div>
          </div>
        </div>
        <p className="hint" style={{ marginTop: 8 }}>
          {refs.length ? `${refs.length} reference${refs.length === 1 ? '' : 's'} wired` : 'Wire references (product, set, light) for Claude to match.'} · {d.views.length || 0} render
          {d.views.length === 1 ? '' : 's'} requested
        </p>
        <TaskCard id={id} kind="blender" task={d.handoff} applied={d.handoffApplied} createLabel="Create Blender task" acceptsFiles />
        <ResultThumbs id={id} assets={d.results} label="Renders" />
      </ResultDrop>
    </NodeShell>
  );
}
