// Saved graphs are untrusted input (hand-edited files, older versions): validate on the way in.
import type { Viewport } from '@xyflow/react';
import type { WorkflowGraph } from '../api/types';
import type { AppEdge, AppNode } from './types';
import { defaultData, isKind } from './types';
import { portOf } from './ports';
import { edgeId } from './store';

const EPHEMERAL = ['uploading', 'previewUrl'];

export function toGraph(nodes: AppNode[], edges: AppEdge[], viewport: Viewport | null): WorkflowGraph {
  return {
    nodes: nodes.map((n) => {
      const data: Record<string, unknown> = { ...n.data };
      for (const k of EPHEMERAL) delete data[k];
      return { id: n.id, type: n.type, position: { x: Math.round(n.position.x), y: Math.round(n.position.y) }, data };
    }),
    edges: edges.map((e) => ({
      id: e.id,
      source: e.source,
      sourceHandle: e.sourceHandle,
      target: e.target,
      targetHandle: e.targetHandle,
      type: e.data?.type,
    })),
    viewport: viewport ?? undefined,
  };
}

function obj(v: unknown): Record<string, unknown> | null {
  return v && typeof v === 'object' && !Array.isArray(v) ? (v as Record<string, unknown>) : null;
}

function num(v: unknown, d = 0): number {
  return typeof v === 'number' && Number.isFinite(v) ? v : d;
}

export function fromGraph(g: unknown): { nodes: AppNode[]; edges: AppEdge[]; viewport: Viewport | null } {
  const root = obj(g);
  const rawNodes = Array.isArray(root?.nodes) ? (root.nodes as unknown[]).slice(0, 500) : [];
  const nodes: AppNode[] = [];
  const seen = new Set<string>();
  for (const r of rawNodes) {
    const o = obj(r);
    if (!o || typeof o.id !== 'string' || !isKind(o.type) || seen.has(o.id)) continue;
    seen.add(o.id);
    const pos = obj(o.position);
    const data = { ...defaultData(o.type), ...(obj(o.data) ?? {}) } as Record<string, unknown>;
    for (const k of EPHEMERAL) delete data[k];
    // Arrays the UI maps over must be arrays.
    for (const k of ['runs', 'order', 'change', 'keep', 'picked', 'results', 'shapes', 'redo', 'views'])
      if (k in data && !Array.isArray(data[k])) data[k] = [];
    if ('params' in data && !obj(data.params)) data.params = {};
    if ('reviews' in data && !obj(data.reviews)) data.reviews = {};
    if ('text' in data && typeof data.text !== 'string') data.text = '';
    if ('prompt' in data && data.prompt !== undefined && typeof data.prompt !== 'string') data.prompt = '';
    nodes.push({ id: o.id, type: o.type, position: { x: num(pos?.x), y: num(pos?.y) }, data } as AppNode);
  }
  const byId = new Map(nodes.map((n) => [n.id, n]));
  const rawEdges = Array.isArray(root?.edges) ? (root.edges as unknown[]).slice(0, 2000) : [];
  const edges: AppEdge[] = [];
  for (const r of rawEdges) {
    const o = obj(r);
    if (!o || typeof o.source !== 'string' || typeof o.target !== 'string') continue;
    const s = byId.get(o.source);
    const t = byId.get(o.target);
    const sh = typeof o.sourceHandle === 'string' ? o.sourceHandle : null;
    const th = typeof o.targetHandle === 'string' ? o.targetHandle : null;
    const out = portOf(s?.type, 'out', sh);
    const inp = portOf(t?.type, 'in', th);
    if (!out || !inp || out.type !== inp.type) continue;
    const c = { source: o.source, sourceHandle: out.id, target: o.target, targetHandle: inp.id };
    const id = edgeId(c);
    if (edges.some((e) => e.id === id)) continue;
    edges.push({ id, type: 'typed', ...c, data: { type: out.type } });
  }
  const vp = obj(root?.viewport);
  const viewport =
    vp && Number.isFinite(vp.x) && Number.isFinite(vp.y) && Number.isFinite(vp.zoom) && num(vp.zoom) > 0
      ? { x: num(vp.x), y: num(vp.y), zoom: num(vp.zoom, 1) }
      : null;
  return { nodes, edges, viewport };
}
