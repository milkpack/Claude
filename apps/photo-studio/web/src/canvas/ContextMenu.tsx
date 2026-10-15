import type { ReactNode } from 'react';
import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { useReactFlow } from '@xyflow/react';
import { ClipboardPaste, Copy, CopyPlus, Eye, EyeOff, Maximize, Play, SquareDashedMousePointer, Trash, Unplug } from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import { useUi } from '../state/ui';
import { copySelection, duplicateSelection, getNode, hasClipboard, pasteClipboard, useGraph } from '../graph/store';
import { runNode, runToHere } from '../graph/run';
import { addConnectedNode, kindsAccepting } from '../graph/actions';
import { NODE_KINDS, isRunnable } from '../graph/types';
import type { NodeKind } from '../graph/types';
import { NODE_META } from './nodeMeta';
import { modKey } from '../lib/util';

function Item({ icon: Icon, label, keys, onClick, danger, disabled }: { icon?: LucideIcon; label: ReactNode; keys?: string; onClick: () => void; danger?: boolean; disabled?: boolean }) {
  return (
    <button
      type="button"
      role="menuitem"
      className={`menu-item ${danger ? 'is-danger' : ''}`}
      disabled={disabled}
      onClick={() => {
        useUi.getState().setMenu(null);
        onClick();
      }}
    >
      {Icon ? <Icon size={14} aria-hidden /> : <span className="menu-icon-pad" />}
      <span className="menu-label">{label}</span>
      {keys ? <span className="menu-keys mono">{keys}</span> : null}
    </button>
  );
}

function AddItems({ kinds, onPick }: { kinds: readonly NodeKind[]; onPick: (k: NodeKind) => void }) {
  return (
    <>
      {kinds.map((k) => (
        <Item key={k} icon={NODE_META[k].icon} label={NODE_META[k].label} onClick={() => onPick(k)} />
      ))}
    </>
  );
}

export function ContextMenu() {
  const menu = useUi((s) => s.menu);
  const ref = useRef<HTMLDivElement>(null);
  const rf = useReactFlow();
  const [pos, setPos] = useState<{ x: number; y: number } | null>(null);

  useLayoutEffect(() => {
    if (!menu || !ref.current) {
      setPos(null);
      return;
    }
    const r = ref.current.getBoundingClientRect();
    const x = Math.min(menu.x, window.innerWidth - r.width - 8);
    const y = Math.min(menu.y, window.innerHeight - r.height - 8);
    setPos({ x: Math.max(8, x), y: Math.max(8, y) });
    ref.current.querySelector<HTMLButtonElement>('.menu-item:not(:disabled)')?.focus();
  }, [menu]);

  useEffect(() => {
    if (!menu) return;
    const close = (e: PointerEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) useUi.getState().setMenu(null);
    };
    const key = (e: KeyboardEvent) => {
      if (e.key === 'Escape') useUi.getState().setMenu(null);
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault();
        const items = Array.from(ref.current?.querySelectorAll<HTMLButtonElement>('.menu-item:not(:disabled)') ?? []);
        const i = items.indexOf(document.activeElement as HTMLButtonElement);
        const next = items[(i + (e.key === 'ArrowDown' ? 1 : items.length - 1)) % Math.max(1, items.length)];
        next?.focus();
      }
    };
    window.addEventListener('pointerdown', close, true);
    window.addEventListener('keydown', key);
    return () => {
      window.removeEventListener('pointerdown', close, true);
      window.removeEventListener('keydown', key);
    };
  }, [menu]);

  if (!menu) return null;
  const g = useGraph.getState();
  let content: ReactNode = null;

  if (menu.kind === 'pane') {
    content = (
      <>
        <div className="menu-label-row">Add node</div>
        <AddItems kinds={NODE_KINDS} onPick={(k) => g.addNode(k, menu.flow, undefined, { select: true })} />
        <div className="menu-sep" />
        <Item icon={ClipboardPaste} label="Paste" keys={`${modKey}+V`} disabled={!hasClipboard()} onClick={() => pasteClipboard(menu.flow)} />
        <Item icon={SquareDashedMousePointer} label="Select all" keys={`${modKey}+A`} onClick={() => g.selectAll()} />
        <Item icon={Maximize} label="Fit view" keys="F" onClick={() => void rf.fitView({ padding: 0.12, maxZoom: 1, duration: 200 })} />
      </>
    );
  } else if (menu.kind === 'node') {
    const n = getNode(menu.id);
    if (!n) return null;
    const selected = g.nodes.filter((x) => x.selected).map((x) => x.id);
    const ids = selected.includes(n.id) ? selected : [n.id];
    const exposed = Boolean(n.data.exposed);
    content = (
      <>
        {isRunnable(n.type) ? <Item icon={Play} label="Run node" keys={`${modKey}+↵`} onClick={() => void runNode(n.id)} /> : null}
        <Item icon={Play} label="Run to here" keys={`${modKey}+⇧+↵`} onClick={() => void runToHere(n.id)} />
        <div className="menu-sep" />
        <Item
          icon={exposed ? EyeOff : Eye}
          label={exposed ? 'Hide from app mode' : 'Show in app mode'}
          onClick={() => g.updateData(n.id, { exposed: !exposed })}
        />
        <Item
          icon={Copy}
          label={ids.length > 1 ? `Copy ${ids.length} nodes` : 'Copy'}
          keys={`${modKey}+C`}
          onClick={() => {
            g.selectOnly(ids);
            copySelection();
          }}
        />
        <Item
          icon={CopyPlus}
          label="Duplicate"
          keys={`${modKey}+D`}
          onClick={() => {
            g.selectOnly(ids);
            duplicateSelection();
          }}
        />
        <Item icon={Unplug} label="Disconnect wires" onClick={() => g.removeEdges(g.edges.filter((e) => ids.includes(e.source) || ids.includes(e.target)).map((e) => e.id))} />
        <div className="menu-sep" />
        <Item icon={Trash} label={ids.length > 1 ? `Delete ${ids.length} nodes` : 'Delete'} keys="Del" danger onClick={() => g.removeNodes(ids)} />
      </>
    );
  } else if (menu.kind === 'edge') {
    content = <Item icon={Trash} label="Delete wire" danger onClick={() => g.removeEdges([menu.id])} />;
  } else if (menu.kind === 'connect') {
    const kinds = kindsAccepting(menu.from.type, menu.from.side);
    content = (
      <>
        <div className="menu-label-row">
          Connect <span className={`type-dot type-${menu.from.type}`} /> {menu.from.type} to…
        </div>
        <AddItems kinds={kinds} onPick={(k) => addConnectedNode(k, { x: menu.flow.x - (menu.from.side === 'target' ? 300 : 0), y: menu.flow.y - 30 }, menu.from)} />
      </>
    );
  }

  return (
    <div
      ref={ref}
      className="menu"
      role="menu"
      style={{ left: pos?.x ?? menu.x, top: pos?.y ?? menu.y, visibility: pos ? 'visible' : 'hidden' }}
      onContextMenu={(e) => e.preventDefault()}
    >
      {content}
    </div>
  );
}
