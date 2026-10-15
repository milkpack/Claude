import type { NodeProps } from '@xyflow/react';
import type { AppNode, BriefData } from '../../graph/types';
import { useGraph } from '../../graph/store';
import { NodeShell } from './NodeShell';

export function BriefNode({ id, data, selected }: NodeProps<Extract<AppNode, { type: 'brief' }>>) {
  const d = data as BriefData;
  return (
    <NodeShell id={id} kind="brief" selected={selected} title={d.title} exposed={d.exposed}>
      <textarea
        className="textarea nodrag nowheel"
        rows={6}
        value={d.text}
        placeholder="The client’s words, in any language. What is the picture for, who or what is in it, the mood…"
        aria-label="Brief"
        onChange={(e) => useGraph.getState().updateData(id, { text: e.target.value }, 'coalesce')}
      />
      <div className="node-foot mono muted">
        <span>{d.text.trim() ? `${d.text.trim().split(/\s+/).length} words` : 'empty'}</span>
        <span>{d.text.length} chars</span>
      </div>
    </NodeShell>
  );
}
