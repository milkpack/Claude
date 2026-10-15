import type { ReactNode } from 'react';
import { useEffect, useRef, useState } from 'react';
import { Handle, Position } from '@xyflow/react';
import { Ellipsis, Play } from 'lucide-react';
import { NODE_META } from '../nodeMeta';
import { PORTS } from '../../graph/ports';
import type { NodeError, NodeKind } from '../../graph/types';
import { useGraph } from '../../graph/store';
import { useIsStale, useRuntime } from '../../graph/hooks';
import { runNode, runToHere } from '../../graph/run';
import { useUi } from '../../state/ui';
import { formatElapsed } from '../../lib/util';
import { ErrorNote } from '../../components/ui';
import { NODE_WIDTH } from '../../graph/actions';

type Props = {
  id: string;
  kind: NodeKind;
  selected: boolean;
  title?: string;
  exposed?: boolean;
  error?: NodeError;
  children: ReactNode;
  /** Shown under the title while running (e.g. "queued", "3 / 8"). */
  phase?: string;
  /** Run button in the header: run this node, or (Output) run to here. */
  run?: 'node' | 'to-here';
  /** Whether the node has a stored output (shows a quiet check). */
  done?: boolean;
};

export function useConnectedHandles(id: string): Set<string> {
  const key = useGraph((s) =>
    s.edges
      .filter((e) => e.source === id || e.target === id)
      .map((e) => (e.source === id ? `out:${e.sourceHandle}` : `in:${e.targetHandle}`))
      .sort()
      .join('|'),
  );
  return new Set(key ? key.split('|') : []);
}

export function Elapsed({ since }: { since: number }) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(t);
  }, []);
  return <span className="mono">{formatElapsed(now - since)}</span>;
}

export function NodeShell({ id, kind, selected, title, exposed, error, children, phase, run, done }: Props) {
  const meta = NODE_META[kind];
  const Icon = meta.icon;
  const rt = useRuntime(id);
  const stale = useIsStale(id);
  const connected = useConnectedHandles(id);
  const ports = PORTS[kind];
  const rows = Math.max(ports.inputs.length, ports.outputs.length);
  const [editing, setEditing] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const running = Boolean(rt);

  useEffect(() => {
    if (editing) inputRef.current?.select();
  }, [editing]);

  const openMenu = (el: HTMLElement) => {
    const r = el.getBoundingClientRect();
    useUi.getState().setMenu({ kind: 'node', id, x: r.left, y: r.bottom + 4 });
  };

  const status = running ? 'running' : error ? 'error' : stale ? 'stale' : done ? 'done' : 'idle';

  return (
    <div
      className={`node node-${kind} ${selected ? 'is-selected' : ''} ${running ? 'is-running' : ''} status-${status}`}
      style={{ width: NODE_WIDTH[kind] }}
      data-testid={`node-${kind}`}
    >
      <header className="node-head">
        <Icon size={13} className="node-icon" aria-hidden />
        <span className="node-kind">{meta.label}</span>
        {editing ? (
          <input
            ref={inputRef}
            className="node-title-input nodrag"
            defaultValue={title ?? ''}
            placeholder={meta.label}
            aria-label="Node title"
            onBlur={(e) => {
              useGraph.getState().updateData(id, { title: e.currentTarget.value.trim() || undefined });
              setEditing(false);
            }}
            onKeyDown={(e) => {
              if (e.key === 'Enter') e.currentTarget.blur();
              if (e.key === 'Escape') setEditing(false);
            }}
          />
        ) : (
          <span className="node-title" title="Double-click to rename" onDoubleClick={() => setEditing(true)}>
            {title ?? ''}
          </span>
        )}
        <span className="node-head-aside">
          {exposed ? (
            <span className="badge" title="Shown in app mode">
              App
            </span>
          ) : null}
          {status === 'running' && rt ? (
            <span className="node-status is-running" aria-live="polite">
              {phase ?? rt.phase ?? 'running'} · <Elapsed since={rt.startedAt} />
            </span>
          ) : status === 'stale' ? (
            <span className="node-status is-stale" title="Inputs changed since this output was made">
              changed
            </span>
          ) : status === 'error' ? (
            <span className="dot dot-danger" title="Failed" />
          ) : status === 'done' ? (
            <span className="dot dot-ok" title="Up to date" />
          ) : null}
          {run ? (
            <button
              type="button"
              className="node-run nodrag"
              disabled={running}
              title={run === 'node' ? 'Run node (uses cached upstream outputs)' : 'Run to here (runs everything upstream that changed)'}
              aria-label={run === 'node' ? 'Run node' : 'Run to here'}
              onClick={() => void (run === 'node' ? runNode(id) : runToHere(id))}
            >
              <Play size={12} aria-hidden />
            </button>
          ) : null}
          <button type="button" className="node-menu nodrag" aria-label="Node menu" onClick={(e) => openMenu(e.currentTarget)}>
            <Ellipsis size={14} aria-hidden />
          </button>
        </span>
        {running ? <span className="node-progress" aria-hidden /> : null}
      </header>
      {rows > 0 ? (
        <div className="node-ports">
          {Array.from({ length: rows }, (_, i) => {
            const inp = ports.inputs[i];
            const out = ports.outputs[i];
            return (
              <div className="port-row" key={i}>
                {inp ? (
                  <span className={`port-label port-in type-${inp.type}`}>
                    <Handle
                      type="target"
                      position={Position.Left}
                      id={inp.id}
                      className={`handle type-${inp.type} ${connected.has(`in:${inp.id}`) ? 'is-connected' : ''}`}
                      title={`${inp.label} (${inp.type}${inp.multi ? ', many' : ''})`}
                    />
                    {inp.label}
                    {inp.optional ? null : <span className="req" title="Required" />}
                  </span>
                ) : (
                  <span />
                )}
                {out ? (
                  <span className={`port-label port-out type-${out.type}`}>
                    {out.label}
                    <Handle
                      type="source"
                      position={Position.Right}
                      id={out.id}
                      className={`handle type-${out.type} ${connected.has(`out:${out.id}`) ? 'is-connected' : ''}`}
                      title={`${out.label} (${out.type})`}
                    />
                  </span>
                ) : (
                  <span />
                )}
              </div>
            );
          })}
        </div>
      ) : null}
      <div className="node-body">
        {error && !running ? <ErrorNote error={error} compact onRetry={() => void runNode(id)} /> : null}
        {children}
      </div>
    </div>
  );
}
