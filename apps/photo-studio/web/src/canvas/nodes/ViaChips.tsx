import type { Via } from '../../graph/types';
import { useGraph } from '../../graph/store';
import { Chips } from '../../components/ui';

const OPTIONS = ['handoff', 'direct'] as const;

/** Handoff (default) or a direct API run; direct only exists when Claude is configured. */
export function ViaChips({ id, via, claudeOff }: { id: string; via: Via | undefined; claudeOff: boolean }) {
  if (claudeOff) return <p className="hint">Claude runs as a handoff: the server has no API key.</p>;
  return (
    <div className="param">
      <span className="param-label">Run via</span>
      <div className="param-control">
        <Chips options={OPTIONS} value={via ?? 'handoff'} onChange={(v) => useGraph.getState().updateData(id, { via: v })} label="Run via" format={(v) => (v === 'handoff' ? 'handoff' : 'API')} />
      </div>
    </div>
  );
}
