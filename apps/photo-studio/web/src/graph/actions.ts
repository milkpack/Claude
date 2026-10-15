// Compound canvas actions used from nodes, menus, the viewer and drag-and-drop.
import type { XYPosition } from '@xyflow/react';
import { api } from '../api/client';
import { describeError } from '../api/errors';
import { qk, queryClient } from '../api/queries';
import type { Asset } from '../api/types';
import { toast, useUi } from '../state/ui';
import { uid, validateUpload } from '../lib/util';
import { edgeId, getNode, useGraph } from './store';
import type { AppEdge, AppNode, GenData, NodeKind, PortType, RefRole } from './types';
import { defaultData } from './types';
import { gatherInputs, upstreamProducer } from './evaluate';
import { PORTS } from './ports';

export const NODE_WIDTH: Record<NodeKind, number> = {
  brief: 280,
  reference: 260,
  director: 340,
  generate: 340,
  edit: 340,
  review: 320,
  compare: 320,
  output: 300,
};

export function makeNode<K extends NodeKind>(kind: K, position: XYPosition, data: Record<string, unknown> = {}): AppNode {
  return { id: uid(`${kind}-`), type: kind, position, data: { ...defaultData(kind), ...data } } as AppNode;
}

export function makeEdge(source: string, sourceHandle: string, target: string, targetHandle: string, type: PortType): AppEdge {
  const c = { source, sourceHandle, target, targetHandle };
  return { id: edgeId(c), type: 'typed', ...c, data: { type } };
}

function rightOf(n: AppNode | undefined, dy = 0): XYPosition {
  if (!n) return { x: 0, y: 0 };
  const w = n.measured?.width ?? NODE_WIDTH[n.type as NodeKind] ?? 300;
  return { x: n.position.x + w + 120, y: n.position.y + dy };
}

/* ---------- uploads and references ---------- */

export async function uploadIntoReference(nodeId: string, file: File): Promise<void> {
  const g = useGraph.getState();
  const bad = validateUpload(file);
  if (bad) {
    toast(bad, 'error');
    return;
  }
  const preview = URL.createObjectURL(file);
  g.updateData(nodeId, { uploading: true, previewUrl: preview, error: undefined }, 'none');
  try {
    const asset = await api.uploadAsset(file);
    if (!getNode(nodeId)) return;
    useGraph.getState().updateData(nodeId, { asset, uploading: false, previewUrl: undefined }, 'push');
    void queryClient.invalidateQueries({ queryKey: qk.assets });
  } catch (err) {
    const v = describeError(err);
    if (getNode(nodeId))
      useGraph.getState().updateData(nodeId, { uploading: false, previewUrl: undefined, error: { title: v.title, message: v.detail, code: v.code } }, 'none');
  } finally {
    window.setTimeout(() => URL.revokeObjectURL(preview), 30_000);
  }
}

/** Creates one Reference node per image file, cascading from `at` (flow coordinates). */
export function referencesFromFiles(files: File[], at: XYPosition): void {
  const images = files.filter((f) => f.type.startsWith('image/'));
  if (!images.length) {
    if (files.length) toast('Only PNG, JPEG and WebP images can be dropped here', 'error');
    return;
  }
  images.slice(0, 12).forEach((file, i) => {
    const id = useGraph.getState().addNode('reference', { x: at.x + i * 28, y: at.y + i * 28 }, { role: i === 0 ? 'edit-source' : 'style' } as Record<string, unknown>, {
      select: i === 0,
    });
    void uploadIntoReference(id, file);
  });
}

export function referenceFromAsset(asset: Asset, at: XYPosition, role: RefRole = 'style'): string {
  return useGraph.getState().addNode('reference', at, { asset, role } as Record<string, unknown>, { select: true });
}

/* ---------- viewer actions ---------- */

export function addAsReference(asset: Asset, fromNode?: string): void {
  const src = fromNode ? getNode(fromNode) : undefined;
  const at = src ? { x: src.position.x, y: src.position.y + (src.measured?.height ?? 300) + 60 } : centerOfView();
  const id = referenceFromAsset(asset, at, 'style');
  useUi.getState().closeViewer();
  useUi.getState().focus(id);
  toast('Added a Reference node');
}

export function remix(asset: Asset, fromNode?: string): void {
  const src = fromNode ? getNode(fromNode) : undefined;
  const at = src ? rightOf(src, 40) : centerOfView();
  const params = { ...(asset.params ?? {}) };
  delete params.prompt;
  delete params.visualReferences;
  const kind: NodeKind = src?.type === 'edit' ? 'edit' : 'generate';
  const data: Partial<GenData> = {
    model: asset.model,
    mode: asset.mode,
    params,
    prompt: asset.prompt ?? '',
    title: 'Remix',
    runs: [],
  };
  const node = makeNode(kind, at, data as Record<string, unknown>);
  const edges: AppEdge[] = [];
  if (src) {
    // Keep the original's reference wires (sources, references) but not its prompt or spec.
    for (const e of useGraph.getState().edges)
      if (e.target === src.id && (e.targetHandle === 'refs' || e.targetHandle === 'source'))
        edges.push(makeEdge(e.source, e.sourceHandle ?? '', node.id, e.targetHandle, 'image'));
  }
  useGraph.getState().addGraph([node], edges, { select: true });
  useUi.getState().closeViewer();
  useUi.getState().focus(node.id);
  toast('Remix node added: adjust and run');
}

/* ---------- director ---------- */

export function spawnFromPlan(directorId: string, stepIndex: number): void {
  const g = useGraph.getState();
  const dir = getNode(directorId);
  if (!dir || dir.type !== 'director' || !dir.data.result) return;
  const res = dir.data.result;
  const step = res.plan[stepIndex];
  if (!step) return;
  const ins = gatherInputs(directorId, g.nodes, g.edges);
  const refs = ins.images.refs ?? [];
  const sourceRef = refs.find((r) => r.role === 'edit-source');
  const isEdit = Boolean(sourceRef) || /image2image|edit/i.test(step.mode) && dir.data.task === 'edit';
  const kind: NodeKind = isEdit && sourceRef ? 'edit' : 'generate';
  const params = { ...res.params };
  delete params.prompt;
  delete params.visualReferences;
  const node = makeNode(kind, rightOf(dir, stepIndex * 60), {
    title: stepIndex === 0 ? undefined : `Alt: ${step.model}`,
    model: step.model,
    mode: step.mode,
    params,
    category: res.category,
    prompt: '',
    keep: kind === 'edit' ? [] : undefined,
    change: kind === 'edit' ? [] : undefined,
    runs: [],
  });
  const edges: AppEdge[] = [makeEdge(directorId, 'spec', node.id, 'spec', 'spec')];
  // Wire the same references the Director saw, so labels line up with its prompt.
  for (const e of g.edges) {
    if (e.target !== directorId || e.targetHandle !== 'refs') continue;
    const srcNode = getNode(e.source);
    const role = srcNode?.type === 'reference' ? srcNode.data.role : undefined;
    const port = kind === 'edit' && role === 'edit-source' ? 'source' : 'refs';
    edges.push(makeEdge(e.source, e.sourceHandle ?? 'image', node.id, port, 'image'));
  }
  g.addGraph([node], edges, { select: true });
  useUi.getState().focus(node.id);
}

/* ---------- review ---------- */

/** Re-run the reviewed step with the review's corrected prompt (a new downstream node). */
export function applyFix(reviewId: string, assetId?: string): void {
  const g = useGraph.getState();
  const review = getNode(reviewId);
  if (!review || review.type !== 'review') return;
  const r = assetId ? review.data.reviews[assetId] : undefined;
  const fix = (r ?? review.data.order.map((id) => review.data.reviews[id]).find((x) => x?.fix))?.fix;
  if (!fix) {
    toast('This review has no fix to apply');
    return;
  }
  const producer = upstreamProducer(reviewId, g.nodes, g.edges);
  const kind: NodeKind = producer?.type === 'edit' ? 'edit' : 'generate';
  const base = producer && (producer.type === 'edit' || producer.type === 'generate') ? producer.data : undefined;
  const node = makeNode(kind, rightOf(review), {
    title: 'Fix',
    model: base?.model,
    mode: base?.mode,
    params: { ...(base?.params ?? {}) },
    change: base?.change ? [...base.change] : kind === 'edit' ? [] : undefined,
    keep: base?.keep ? [...base.keep] : kind === 'edit' ? [] : undefined,
    category: base?.category,
    prompt: fix.prompt,
    runs: [],
  });
  const edges: AppEdge[] = [];
  if (producer)
    for (const e of g.edges)
      if (e.target === producer.id && (e.targetHandle === 'refs' || e.targetHandle === 'source'))
        edges.push(makeEdge(e.source, e.sourceHandle ?? '', node.id, e.targetHandle, 'image'));
  g.addGraph([node], edges, { select: true });
  useUi.getState().focus(node.id);
  toast('Fix applied as a new node: run it when ready');
}

/** Targeted edit of the reviewed image itself: Reference (the image) → Edit (the fix). */
export function editReviewedImage(reviewId: string, asset: Asset): void {
  const g = useGraph.getState();
  const review = getNode(reviewId);
  if (!review || review.type !== 'review') return;
  const fix = review.data.reviews[asset.id]?.fix;
  const at = rightOf(review);
  const ref = makeNode('reference', at, { asset, role: 'edit-source', title: 'Reviewed image' });
  const edit = makeNode('edit', { x: at.x + 360, y: at.y }, {
    title: 'Targeted fix',
    model: 'grok-imagine-image-2',
    mode: 'image2image',
    prompt: fix?.prompt ?? '',
    change: [],
    keep: [],
    params: {},
    runs: [],
  });
  g.addGraph([ref, edit], [makeEdge(ref.id, 'image', edit.id, 'source', 'image')], { select: true });
  useUi.getState().focus(edit.id);
}

/* ---------- canvas helpers ---------- */

let viewCenter: () => XYPosition = () => ({ x: 0, y: 0 });
export function registerViewCenter(fn: () => XYPosition): void {
  viewCenter = fn;
}
export function centerOfView(): XYPosition {
  const c = viewCenter();
  return { x: Math.round(c.x - 150), y: Math.round(c.y - 120) };
}

export function addNodeAtCenter(kind: NodeKind): string {
  const n = useGraph.getState().nodes.length;
  const c = centerOfView();
  const id = useGraph.getState().addNode(kind, { x: c.x + (n % 5) * 16, y: c.y + (n % 5) * 16 }, undefined, { select: true });
  return id;
}

/** Connect a dragged wire to a freshly created node, on the first compatible port. */
export function addConnectedNode(kind: NodeKind, at: XYPosition, from: { node: string; handle: string; side: 'source' | 'target'; type: PortType }): void {
  const g = useGraph.getState();
  const node = makeNode(kind, at);
  const ports = from.side === 'source' ? PORTS[kind].inputs : PORTS[kind].outputs;
  const port = ports.find((p) => p.type === from.type);
  const edges: AppEdge[] = [];
  if (port)
    edges.push(
      from.side === 'source'
        ? makeEdge(from.node, from.handle, node.id, port.id, from.type)
        : makeEdge(node.id, port.id, from.node, from.handle, from.type),
    );
  g.addGraph([node], edges, { select: true });
}

export function kindsAccepting(type: PortType, side: 'source' | 'target'): NodeKind[] {
  return (Object.keys(PORTS) as NodeKind[]).filter((k) =>
    (side === 'source' ? PORTS[k].inputs : PORTS[k].outputs).some((p) => p.type === type),
  );
}
