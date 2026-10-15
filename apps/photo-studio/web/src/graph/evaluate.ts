// Pure functions over (nodes, edges): what flows along each wire, the prompt a node will send,
// and the cache key that says whether a node's stored output still matches its inputs.
import type { Asset, DirectorResponse, Model } from '../api/types';
import type { AppEdge, AppNode, DirectorData, GenData, NodeKind, ReferenceData, ReviewData } from './types';
import { PORTS } from './ports';
import { hash, stableStringify } from '../lib/util';

export type ImageIn = { asset: Asset; role?: string; from: string; fromKind: NodeKind };

export type Inputs = {
  text: Record<string, string[]>;
  images: Record<string, ImageIn[]>;
  spec: DirectorResponse | null;
  /** Wires per input port. */
  wires: Record<string, number>;
  /** Source node ids per input port. */
  from: Record<string, string[]>;
};

type PortValue =
  | { type: 'text'; value: string | null }
  | { type: 'image'; value: ImageIn[] }
  | { type: 'spec'; value: DirectorResponse | null };

const MAX_DEPTH = 64;

export function currentRun(d: GenData) {
  if (!d.runs.length) return undefined;
  const i = d.runIndex ?? d.runs.length - 1;
  return d.runs[Math.min(Math.max(i, 0), d.runs.length - 1)];
}

function nodeById(nodes: AppNode[], id: string): AppNode | undefined {
  return nodes.find((n) => n.id === id);
}

export function nodeOutput(node: AppNode, handle: string, nodes: AppNode[], edges: AppEdge[], depth = 0): PortValue | null {
  if (depth > MAX_DEPTH) return null;
  switch (node.type) {
    case 'brief': {
      const t = node.data.text.trim();
      return { type: 'text', value: t || null };
    }
    case 'reference': {
      const d = node.data as ReferenceData;
      return { type: 'image', value: d.asset ? [{ asset: d.asset, role: d.role, from: node.id, fromKind: 'reference' }] : [] };
    }
    case 'director': {
      const d = node.data as DirectorData;
      if (handle === 'spec') return { type: 'spec', value: d.result ? { ...d.result, prompt: d.prompt ?? d.result.prompt } : null };
      return { type: 'text', value: (d.prompt ?? d.result?.prompt ?? '').trim() || null };
    }
    case 'generate':
    case 'edit': {
      const d = node.data as GenData;
      const run = currentRun(d);
      if (!run) return { type: 'image', value: [] };
      const picked = new Set(d.picked ?? []);
      const assets = picked.size ? run.assets.filter((a) => picked.has(a.id)) : run.assets;
      return { type: 'image', value: assets.map((asset) => ({ asset, from: node.id, fromKind: node.type })) };
    }
    case 'review': {
      const d = node.data as ReviewData;
      const cur = d.current ? d.reviews[d.current] : undefined;
      const pick = cur?.fix ? cur : d.order.map((id) => d.reviews[id]).find((r) => r && r.verdict !== 'accept' && r.fix);
      return { type: 'text', value: pick?.fix?.prompt ?? null };
    }
    case 'compare': {
      const ins = gatherInputs(node.id, nodes, edges, depth + 1);
      const all = ins.images.images ?? [];
      const w = all.find((i) => i.asset.id === node.data.winner);
      return { type: 'image', value: w ? [w] : [] };
    }
    case 'output':
      return null;
    default:
      return null;
  }
}

export function gatherInputs(id: string, nodes: AppNode[], edges: AppEdge[], depth = 0): Inputs {
  const out: Inputs = { text: {}, images: {}, spec: null, wires: {}, from: {} };
  const self = nodeById(nodes, id);
  if (!self?.type) return out;
  // Wires in a stable visual order: top to bottom, then left to right.
  const incoming = edges
    .filter((e) => e.target === id)
    .map((e) => ({ e, src: nodeById(nodes, e.source) }))
    .filter((x): x is { e: AppEdge; src: AppNode } => Boolean(x.src))
    .sort((a, b) => a.src.position.y - b.src.position.y || a.src.position.x - b.src.position.x);
  for (const port of PORTS[self.type].inputs) {
    out.wires[port.id] = 0;
    out.from[port.id] = [];
    if (port.type === 'text') out.text[port.id] = [];
    if (port.type === 'image') out.images[port.id] = [];
  }
  for (const { e, src } of incoming) {
    const port = e.targetHandle ?? '';
    out.wires[port] = (out.wires[port] ?? 0) + 1;
    (out.from[port] ??= []).push(src.id);
    const v = nodeOutput(src, e.sourceHandle ?? '', nodes, edges, depth + 1);
    if (!v) continue;
    if (v.type === 'text' && v.value) (out.text[port] ??= []).push(v.value);
    if (v.type === 'image') {
      const list = (out.images[port] ??= []);
      for (const img of v.value) if (!list.some((x) => x.asset.id === img.asset.id)) list.push(img);
    }
    if (v.type === 'spec' && v.value) out.spec = v.value;
  }
  return out;
}

/* ---------- prompts ---------- */

export type PromptSource = 'wire' | 'spec' | 'local';

export function generatePrompt(d: GenData, ins: Inputs): { text: string; source: PromptSource } {
  const wired = (ins.text.prompt ?? []).join('\n\n').trim();
  if (wired) return { text: wired, source: 'wire' };
  if ((ins.wires.prompt ?? 0) > 0) return { text: '', source: 'wire' };
  if (ins.spec?.prompt) return { text: ins.spec.prompt, source: 'spec' };
  return { text: d.prompt.trim(), source: 'local' };
}

export function roleLabel(role: string | undefined): string {
  if (!role) return 'reference';
  return role === 'edit-source' ? 'edit source' : role;
}

/** The "change only / keep exactly" builder, written the way the edit models read best. */
export function editPrompt(d: GenData, ins: Inputs, refs: LabeledRef[]): { text: string; instruction: string; source: PromptSource } {
  const base = generatePrompt(d, ins);
  const change = (d.change ?? []).map((s) => s.trim()).filter(Boolean);
  const keep = keepList(d, ins);
  const lines: string[] = [];
  const source = refs.find((r) => r.isSource);
  if (source) lines.push(`Edit ${source.label.replace('image', 'image ')} (the source photo).`);
  for (const r of refs) {
    if (r.isSource) continue;
    lines.push(`Use ${r.label.replace('image', 'image ')} as the ${roleLabel(r.role)} reference.`);
  }
  if (change.length) lines.push(`Change only: ${change.join('; ')}.`);
  if (base.text) lines.push(base.text);
  if (keep.length) lines.push(`Keep exactly as in the source: ${keep.join(', ')}.`);
  return { text: lines.join('\n'), instruction: base.text, source: base.source };
}

export function keepList(d: GenData, ins: Inputs): string[] {
  const own = (d.keep ?? []).map((s) => s.trim()).filter(Boolean);
  if (own.length) return own;
  return (ins.spec?.preserve ?? []).map((s) => s.trim()).filter(Boolean);
}

/* ---------- references ---------- */

export type LabeledRef = { asset: Asset; label: string; role?: string; from: string; isSource: boolean };

/** References in the order they are sent, labelled image1, image2, … (the edit source first). */
export function orderedRefs(kind: NodeKind, ins: Inputs): LabeledRef[] {
  const list: { img: ImageIn; isSource: boolean }[] = [];
  if (kind === 'edit') for (const img of (ins.images.source ?? []).slice(0, 1)) list.push({ img, isSource: true });
  let refs = [...(ins.images.refs ?? [])];
  const specOrder = ins.spec?.references ?? [];
  if (specOrder.length) {
    const rank = (id: string) => {
      const i = specOrder.findIndex((r) => r.assetId === id);
      return i < 0 ? Number.MAX_SAFE_INTEGER : i;
    };
    refs = refs.sort((a, b) => rank(a.asset.id) - rank(b.asset.id));
  }
  for (const img of refs) if (!list.some((x) => x.img.asset.id === img.asset.id)) list.push({ img, isSource: false });
  return list.map(({ img, isSource }, i) => ({
    asset: img.asset,
    label: `image${i + 1}`,
    role: isSource ? 'edit-source' : (specOrder.find((r) => r.assetId === img.asset.id)?.role ?? img.role),
    from: img.from,
    isSource,
  }));
}

/* ---------- modes and recommendations ---------- */

export function pickMode(model: Model | undefined, kind: NodeKind, hasRefs: boolean): string | undefined {
  const modes = (model?.modes.image ?? []).map((m) => m.mode);
  if (!modes.length) return undefined;
  const i2i = modes.find((m) => /image2image|i2i|edit/i.test(m));
  const t2i = modes.find((m) => /text2image|t2i/i.test(m));
  if (kind === 'edit' || hasRefs) return i2i ?? modes[0];
  return t2i ?? modes[0];
}

/** Recommended models first: the Director's plan, then models routed to the category. */
export function recommendedModels(models: Model[], opts: { plan?: string[]; category?: string; needI2I?: boolean }): {
  recommended: Model[];
  rest: Model[];
} {
  const usable = (m: Model) => !opts.needI2I || (m.modes.image ?? []).some((x) => /image2image|i2i|edit/i.test(x.mode));
  const rec: Model[] = [];
  for (const id of opts.plan ?? []) {
    const m = models.find((x) => x.id === id);
    if (m && !rec.includes(m)) rec.push(m);
  }
  if (opts.category) {
    const cat = opts.category.toLowerCase();
    for (const m of models)
      if (!rec.includes(m) && usable(m) && m.categories.some((c) => cat.includes(c.toLowerCase()) || c.toLowerCase().includes(cat)))
        rec.push(m);
  }
  const recommended = rec.slice(0, 4);
  return { recommended, rest: models.filter((m) => !recommended.includes(m)) };
}

/* ---------- cache keys ---------- */

export function cacheKeyFor(node: AppNode, nodes: AppNode[], edges: AppEdge[]): string | null {
  const ins = gatherInputs(node.id, nodes, edges);
  switch (node.type) {
    case 'director': {
      const d = node.data;
      return hash(
        stableStringify({
          brief: ins.text.brief ?? [],
          refs: (ins.images.refs ?? []).map((r) => [r.asset.id, r.role ?? '']),
          task: d.task,
          model: d.model ?? '',
        }),
      );
    }
    case 'generate':
    case 'edit': {
      const d = node.data;
      const refs = orderedRefs(node.type, ins);
      const prompt = node.type === 'edit' ? editPrompt(d, ins, refs).text : generatePrompt(d, ins).text;
      return hash(
        stableStringify({
          model: d.model ?? '',
          params: d.params,
          prompt,
          refs: refs.map((r) => [r.asset.id, r.label]),
        }),
      );
    }
    case 'review':
      return hash(
        stableStringify({
          images: (ins.images.image ?? []).map((i) => i.asset.id),
          source: ins.images.source?.[0]?.asset.id ?? '',
          brief: ins.text.brief ?? [],
        }),
      );
    default:
      return null;
  }
}

/** Whether a runnable node has a usable stored output. */
export function hasOutput(node: AppNode): boolean {
  switch (node.type) {
    case 'director':
      return Boolean(node.data.result);
    case 'generate':
    case 'edit':
      return node.data.runs.length > 0;
    case 'review':
      return node.data.order.length > 0;
    default:
      return true;
  }
}

export function isStale(node: AppNode, nodes: AppNode[], edges: AppEdge[]): boolean {
  const stored = (node.data as { cacheKey?: string }).cacheKey;
  if (!stored || !hasOutput(node)) return false;
  return cacheKeyFor(node, nodes, edges) !== stored;
}

/** Upstream nodes of `id` in topological order (ancestors first), excluding `id`. */
export function upstreamOrder(id: string, nodes: AppNode[], edges: AppEdge[]): string[] {
  const anc = new Set<string>();
  const stack = [id];
  while (stack.length) {
    const cur = stack.pop();
    if (cur === undefined) break;
    for (const e of edges)
      if (e.target === cur && !anc.has(e.source)) {
        anc.add(e.source);
        stack.push(e.source);
      }
  }
  anc.delete(id);
  return topoSort([...anc], edges).filter((x) => nodes.some((n) => n.id === x));
}

export function topoSort(ids: string[], edges: AppEdge[]): string[] {
  const set = new Set(ids);
  const indeg = new Map<string, number>(ids.map((i) => [i, 0]));
  for (const e of edges) if (set.has(e.source) && set.has(e.target)) indeg.set(e.target, (indeg.get(e.target) ?? 0) + 1);
  const queue = ids.filter((i) => (indeg.get(i) ?? 0) === 0);
  const out: string[] = [];
  while (queue.length) {
    const cur = queue.shift();
    if (cur === undefined) break;
    out.push(cur);
    for (const e of edges)
      if (e.source === cur && set.has(e.target)) {
        const n = (indeg.get(e.target) ?? 0) - 1;
        indeg.set(e.target, n);
        if (n === 0) queue.push(e.target);
      }
  }
  // Any leftovers (a cycle slipped in from a hand-edited file) go last rather than being lost.
  for (const i of ids) if (!out.includes(i)) out.push(i);
  return out;
}

/** The node and wire that produced the images reviewed by a Review node. */
export function upstreamProducer(reviewId: string, nodes: AppNode[], edges: AppEdge[]): AppNode | undefined {
  const e = edges.find((x) => x.target === reviewId && x.targetHandle === 'image');
  return e ? nodeById(nodes, e.source) : undefined;
}
