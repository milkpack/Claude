import { create } from 'zustand';
import { applyEdgeChanges, applyNodeChanges } from '@xyflow/react';
import type { Connection, EdgeChange, NodeChange, Viewport, XYPosition } from '@xyflow/react';
import type { AnyData, AppEdge, AppNode, NodeKind } from './types';
import { defaultData } from './types';
import { portOf } from './ports';
import { uid } from '../lib/util';

type Snapshot = { nodes: AppNode[]; edges: AppEdge[] };

export type Runtime = {
  status: 'running';
  startedAt: number;
  phase?: string;
  jobId?: string;
  /** Review: how many images are done / total. */
  done?: number;
  total?: number;
};

type HistoryMode = 'push' | 'coalesce' | 'none';

export type GraphState = {
  workflowId: string | null;
  name: string;
  nodes: AppNode[];
  edges: AppEdge[];
  viewport: Viewport | null;
  /** Bumps on every change that should be saved. */
  version: number;
  /** Bumps when a whole graph is loaded (the canvas fits the view). */
  loadSeq: number;
  past: Snapshot[];
  future: Snapshot[];
  coalesce: { key: string; at: number } | null;
  runtime: Record<string, Runtime>;

  onNodesChange: (changes: NodeChange<AppNode>[]) => void;
  onEdgesChange: (changes: EdgeChange<AppEdge>[]) => void;
  connect: (c: Connection) => string | null;
  addNode: (kind: NodeKind, position: XYPosition, data?: Partial<AnyData>, opts?: { select?: boolean }) => string;
  addGraph: (nodes: AppNode[], edges: AppEdge[], opts?: { select?: boolean }) => void;
  updateData: (id: string, patch: Partial<AnyData> | ((d: AnyData) => Partial<AnyData>), history?: HistoryMode) => void;
  removeNodes: (ids: string[]) => void;
  removeEdges: (ids: string[]) => void;
  selectOnly: (ids: string[]) => void;
  selectAll: () => void;
  setName: (name: string) => void;
  setViewport: (v: Viewport) => void;
  load: (g: { id: string | null; name: string; nodes: AppNode[]; edges: AppEdge[]; viewport: Viewport | null }) => void;
  setWorkflowId: (id: string) => void;
  checkpoint: (coalesceKey?: string) => void;
  undo: () => void;
  redo: () => void;
  setRuntime: (id: string, rt: Runtime | null) => void;
};

const HISTORY_LIMIT = 100;

function snapshot(s: { nodes: AppNode[]; edges: AppEdge[] }): Snapshot {
  return {
    nodes: s.nodes.map((n) => ({ ...n, selected: false, dragging: false })) as AppNode[],
    edges: s.edges.map((e) => ({ ...e, selected: false })),
  };
}

export function edgeId(c: { source: string; sourceHandle?: string | null; target: string; targetHandle?: string | null }): string {
  return `${c.source}:${c.sourceHandle ?? ''}->${c.target}:${c.targetHandle ?? ''}`;
}

/** True when `to` is reachable from `from` along the edges (used to refuse cycles). */
export function reaches(from: string, to: string, edges: AppEdge[]): boolean {
  const seen = new Set<string>();
  const stack = [from];
  while (stack.length) {
    const cur = stack.pop();
    if (cur === undefined) break;
    if (cur === to) return true;
    if (seen.has(cur)) continue;
    seen.add(cur);
    for (const e of edges) if (e.source === cur) stack.push(e.target);
  }
  return false;
}

export function validConnection(c: Connection | AppEdge, nodes: AppNode[], edges: AppEdge[]): boolean {
  if (!c.source || !c.target || c.source === c.target) return false;
  const s = nodes.find((n) => n.id === c.source);
  const t = nodes.find((n) => n.id === c.target);
  const out = portOf(s?.type, 'out', c.sourceHandle);
  const inp = portOf(t?.type, 'in', c.targetHandle);
  if (!out || !inp || out.type !== inp.type) return false;
  if (reaches(c.target, c.source, edges)) return false;
  return true;
}

export const useGraph = create<GraphState>((set, get) => ({
  workflowId: null,
  name: 'Untitled session',
  nodes: [],
  edges: [],
  viewport: null,
  version: 0,
  loadSeq: 0,
  past: [],
  future: [],
  coalesce: null,
  runtime: {},

  onNodesChange: (changes) => {
    const s = get();
    const removing = changes.some((c) => c.type === 'remove');
    if (removing) get().checkpoint();
    const persist = changes.some(
      (c) => c.type === 'remove' || c.type === 'add' || c.type === 'replace' || (c.type === 'position' && c.dragging === false),
    );
    let edges = s.edges;
    if (removing) {
      const gone = new Set(changes.filter((c) => c.type === 'remove').map((c) => (c as { id: string }).id));
      edges = edges.filter((e) => !gone.has(e.source) && !gone.has(e.target));
    }
    set({
      nodes: applyNodeChanges(changes, get().nodes),
      edges,
      version: persist ? s.version + 1 : s.version,
    });
  },

  onEdgesChange: (changes) => {
    const removing = changes.some((c) => c.type === 'remove');
    if (removing) get().checkpoint();
    set((s) => ({ edges: applyEdgeChanges(changes, s.edges), version: removing ? s.version + 1 : s.version }));
  },

  connect: (c) => {
    const s = get();
    if (!validConnection(c, s.nodes, s.edges)) return null;
    const src = s.nodes.find((n) => n.id === c.source);
    const tgt = s.nodes.find((n) => n.id === c.target);
    const out = portOf(src?.type, 'out', c.sourceHandle);
    const inp = portOf(tgt?.type, 'in', c.targetHandle);
    if (!out || !inp) return null;
    const id = edgeId(c);
    if (s.edges.some((e) => e.id === id)) return id;
    get().checkpoint();
    // A single-input port takes one wire: the new one replaces the old.
    const kept = inp.multi
      ? s.edges
      : s.edges.filter((e) => !(e.target === c.target && (e.targetHandle ?? null) === (c.targetHandle ?? null)));
    const edge: AppEdge = {
      id,
      type: 'typed',
      source: c.source,
      sourceHandle: c.sourceHandle,
      target: c.target,
      targetHandle: c.targetHandle,
      data: { type: out.type },
    };
    set({ edges: [...kept, edge], version: get().version + 1 });
    return id;
  },

  addNode: (kind, position, data, opts) => {
    get().checkpoint();
    const id = uid(`${kind}-`);
    const node = { id, type: kind, position, data: { ...defaultData(kind), ...data }, selected: opts?.select ?? false } as AppNode;
    set((s) => ({
      nodes: [...(opts?.select ? s.nodes.map((n) => (n.selected ? { ...n, selected: false } : n)) : s.nodes), node],
      version: s.version + 1,
    }));
    return id;
  },

  addGraph: (nodes, edges, opts) => {
    if (!nodes.length && !edges.length) return;
    get().checkpoint();
    set((s) => ({
      nodes: [
        ...(opts?.select ? s.nodes.map((n) => (n.selected ? ({ ...n, selected: false } as AppNode) : n)) : s.nodes),
        ...nodes.map((n) => ({ ...n, selected: opts?.select ?? false }) as AppNode),
      ],
      edges: [...s.edges, ...edges],
      version: s.version + 1,
    }));
  },

  updateData: (id, patch, history = 'push') => {
    if (history === 'push') get().checkpoint();
    else if (history === 'coalesce') get().checkpoint(`data:${id}`);
    set((s) => ({
      nodes: s.nodes.map((n) => {
        if (n.id !== id) return n;
        const p = typeof patch === 'function' ? patch(n.data as AnyData) : patch;
        return { ...n, data: { ...n.data, ...p } } as AppNode;
      }),
      version: s.version + 1,
    }));
  },

  removeNodes: (ids) => {
    if (!ids.length) return;
    get().checkpoint();
    const gone = new Set(ids);
    set((s) => ({
      nodes: s.nodes.filter((n) => !gone.has(n.id)),
      edges: s.edges.filter((e) => !gone.has(e.source) && !gone.has(e.target)),
      version: s.version + 1,
    }));
  },

  removeEdges: (ids) => {
    if (!ids.length) return;
    get().checkpoint();
    const gone = new Set(ids);
    set((s) => ({ edges: s.edges.filter((e) => !gone.has(e.id)), version: s.version + 1 }));
  },

  selectOnly: (ids) => {
    const want = new Set(ids);
    set((s) => ({
      nodes: s.nodes.map((n) => (Boolean(n.selected) === want.has(n.id) ? n : ({ ...n, selected: want.has(n.id) } as AppNode))),
      edges: s.edges.map((e) => (e.selected ? { ...e, selected: false } : e)),
    }));
  },

  selectAll: () => set((s) => ({ nodes: s.nodes.map((n) => (n.selected ? n : ({ ...n, selected: true } as AppNode))) })),

  setName: (name) => set((s) => ({ name, version: s.version + 1 })),

  setViewport: (v) => set((s) => ({ viewport: v, version: s.version + 1 })),

  load: (g) =>
    set((s) => ({
      workflowId: g.id,
      name: g.name,
      nodes: g.nodes,
      edges: g.edges,
      viewport: g.viewport,
      past: [],
      future: [],
      coalesce: null,
      runtime: {},
      loadSeq: s.loadSeq + 1,
    })),

  setWorkflowId: (id) => set({ workflowId: id }),

  checkpoint: (coalesceKey) => {
    const s = get();
    const now = Date.now();
    if (coalesceKey && s.coalesce && s.coalesce.key === coalesceKey && now - s.coalesce.at < 1200) {
      set({ coalesce: { key: coalesceKey, at: now } });
      return;
    }
    const past = [...s.past, snapshot(s)];
    if (past.length > HISTORY_LIMIT) past.shift();
    set({ past, future: [], coalesce: coalesceKey ? { key: coalesceKey, at: now } : null });
  },

  undo: () => {
    const s = get();
    const prev = s.past[s.past.length - 1];
    if (!prev) return;
    set({
      past: s.past.slice(0, -1),
      future: [snapshot(s), ...s.future].slice(0, HISTORY_LIMIT),
      nodes: prev.nodes,
      edges: prev.edges,
      coalesce: null,
      version: s.version + 1,
    });
  },

  redo: () => {
    const s = get();
    const next = s.future[0];
    if (!next) return;
    set({
      past: [...s.past, snapshot(s)],
      future: s.future.slice(1),
      nodes: next.nodes,
      edges: next.edges,
      coalesce: null,
      version: s.version + 1,
    });
  },

  setRuntime: (id, rt) =>
    set((s) => {
      const runtime = { ...s.runtime };
      if (rt) runtime[id] = rt;
      else delete runtime[id];
      return { runtime };
    }),
}));

export function getNode(id: string): AppNode | undefined {
  return useGraph.getState().nodes.find((n) => n.id === id);
}

/* ---------- clipboard (copy/paste of nodes) ---------- */

let clipboard: { nodes: AppNode[]; edges: AppEdge[] } | null = null;

export function copySelection(): number {
  const s = useGraph.getState();
  const nodes = s.nodes.filter((n) => n.selected);
  if (!nodes.length) return 0;
  const ids = new Set(nodes.map((n) => n.id));
  clipboard = {
    nodes: structuredClone(nodes.map((n) => ({ ...n, selected: false }))) as AppNode[],
    edges: structuredClone(s.edges.filter((e) => ids.has(e.source) && ids.has(e.target))),
  };
  return nodes.length;
}

export function hasClipboard(): boolean {
  return clipboard !== null && clipboard.nodes.length > 0;
}

/** Paste the copied nodes (new ids), at `at` (flow coordinates) or offset from the originals. */
export function pasteClipboard(at?: XYPosition): number {
  if (!clipboard) return 0;
  const src = clipboard;
  const minX = Math.min(...src.nodes.map((n) => n.position.x));
  const minY = Math.min(...src.nodes.map((n) => n.position.y));
  const dx = at ? at.x - minX : 40;
  const dy = at ? at.y - minY : 40;
  const map = new Map<string, string>();
  const nodes = src.nodes.map((n) => {
    const id = uid(`${n.type ?? 'node'}-`);
    map.set(n.id, id);
    const data = structuredClone(n.data) as Record<string, unknown>;
    delete data.pendingJobId;
    delete data.uploading;
    delete data.previewUrl;
    return { ...n, id, position: { x: n.position.x + dx, y: n.position.y + dy }, data } as AppNode;
  });
  const edges = src.edges.flatMap((e) => {
    const source = map.get(e.source);
    const target = map.get(e.target);
    if (!source || !target) return [];
    return [{ ...e, id: edgeId({ ...e, source, target }), source, target }];
  });
  useGraph.getState().addGraph(nodes, edges, { select: true });
  // Shift the clipboard so repeated pastes cascade instead of stacking.
  if (!at) clipboard = { nodes: src.nodes.map((n) => ({ ...n, position: { x: n.position.x + 40, y: n.position.y + 40 } })) as AppNode[], edges: src.edges };
  return nodes.length;
}

export function duplicateSelection(): number {
  const saved = clipboard;
  const n = copySelection();
  if (n) pasteClipboard();
  clipboard = saved;
  return n;
}
