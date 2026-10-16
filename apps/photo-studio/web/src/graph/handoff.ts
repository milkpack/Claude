// Claude handoff (DESIGN.md, "Handoff mode"): a node builds a task, the owner pastes it into
// Claude, and the answer comes back through the task's return link or the node's paste box.
import { useEffect } from 'react';
import { useQuery } from '@tanstack/react-query';
import { api } from '../api/client';
import { describeError } from '../api/errors';
import { qk, queryClient } from '../api/queries';
import type { Asset, DirectorResponse, Me, ReviewResponse, Task, TaskKind, TaskRequest } from '../api/types';
import { toast } from '../state/ui';
import { renderMask } from '../lib/image';
import { getNode, useGraph } from './store';
import type { AppNode, NodeError } from './types';
import { cacheKeyFor, gatherInputs, keepList, upstreamProducer } from './evaluate';
import { flushSave } from './sync';

export const HANDOFF_KINDS: readonly string[] = ['director', 'review', 'blender', 'photoshop', 'sketch'];

/** Kinds a cloud routine can answer by itself (Blender and Photoshop need the owner's Mac). */
export const AUTO_KINDS: readonly string[] = ['director', 'review', 'sketch', 'custom'];

export function routineConfigured(): boolean {
  return Boolean(queryClient.getQueryData<Me>(qk.me)?.routine?.configured);
}

/** Whether a Director/Review node runs as a handoff (always when Claude isn't configured). */
export function usesHandoff(node: AppNode | undefined): boolean {
  if (!node) return false;
  if (node.type === 'blender' || node.type === 'photoshop' || node.type === 'sketch') return true;
  if (node.type !== 'director' && node.type !== 'review') return false;
  const me = queryClient.getQueryData<Me>(qk.me);
  if (!me?.claude.configured) return true;
  return node.data.via !== 'direct';
}

function setError(id: string, title: string, message: string, code?: string): void {
  const e: NodeError = { title, message, code };
  useGraph.getState().updateData(id, { error: e }, 'none');
}

async function ensureSession(): Promise<string> {
  let id = useGraph.getState().workflowId;
  if (!id) {
    useGraph.setState((s) => ({ version: s.version + 1 }));
    await flushSave();
    id = useGraph.getState().workflowId;
  }
  if (!id) throw new Error('Save the session first (it has not been created yet).');
  return id;
}

const short = (s: string, n = 60) => (s.length > n ? `${s.slice(0, n - 1).trimEnd()}…` : s);

async function requestFor(node: AppNode, sessionId: string): Promise<TaskRequest | string> {
  const { nodes, edges } = useGraph.getState();
  const ins = gatherInputs(node.id, nodes, edges);
  const base = { sessionId, nodeId: node.id };
  switch (node.type) {
    case 'director': {
      const brief = (ins.text.brief ?? []).join('\n\n').trim();
      if (!brief) return 'Connect a Brief node with the client’s words.';
      const refs = (ins.images.refs ?? []).map((r) => ({ assetId: r.asset.id, role: r.role }));
      return {
        ...base,
        kind: 'director',
        title: `Direct: ${short(brief)}`,
        inputs: refs.map((r) => ({ assetId: r.assetId, role: r.role })),
        context: { brief, references: refs, task: node.data.task === 'auto' ? undefined : node.data.task, model: node.data.model },
      };
    }
    case 'review': {
      const images = (ins.images.image ?? []).slice(0, 8);
      if (!images.length) return 'Connect generated images to review. Run the upstream node first.';
      const source = ins.images.source?.[0]?.asset;
      const producer = upstreamProducer(node.id, nodes, edges);
      const preserve = producer?.type === 'edit' ? keepList(producer.data, gatherInputs(producer.id, nodes, edges)) : undefined;
      return {
        ...base,
        kind: 'review',
        title: `Review ${images.length} image${images.length === 1 ? '' : 's'}`,
        inputs: [...images.map((i) => ({ assetId: i.asset.id, role: 'candidate' })), ...(source ? [{ assetId: source.id, role: 'source' }] : [])],
        context: {
          brief: (ins.text.brief ?? []).join('\n\n') || undefined,
          assetIds: images.map((i) => i.asset.id),
          prompts: Object.fromEntries(images.map((i) => [i.asset.id, i.asset.prompt ?? ''])),
          sourceAssetId: source?.id,
          preserve: preserve?.length ? preserve : undefined,
        },
      };
    }
    case 'blender': {
      const d = node.data;
      if (!d.subject.trim()) return 'Describe the subject of the scene.';
      return {
        ...base,
        kind: 'blender',
        title: `Blender: ${short(d.subject)}`,
        inputs: (ins.images.refs ?? []).map((r) => ({ assetId: r.asset.id, role: r.role ?? 'reference' })),
        context: {
          subject: d.subject,
          set: d.set,
          camera: d.camera,
          lens: d.lens,
          light: d.light,
          views: d.views,
          notes: d.notes,
          brief: (ins.text.brief ?? []).join('\n\n') || undefined,
          references: (ins.images.refs ?? []).map((r) => ({ assetId: r.asset.id, role: r.role })),
        },
      };
    }
    case 'photoshop': {
      const d = node.data;
      const source = ins.images.source?.[0]?.asset;
      if (!source) return 'Connect the image to edit into source.';
      const instruction = [d.instruction.trim(), ...(ins.text.prompt ?? [])].filter(Boolean).join('\n\n');
      if (!instruction) return 'Say what to change (“change only …”).';
      let mask: Asset | undefined;
      if ((d.maskMode === 'paint' && d.maskUrl) || (d.maskMode === 'rect' && d.maskRect)) {
        const file = await renderMask(source, d.maskMode === 'paint' ? d.maskUrl : undefined, d.maskMode === 'rect' ? d.maskRect : undefined);
        mask = await api.uploadAsset(file);
        useGraph.getState().updateData(node.id, { maskAsset: mask }, 'none');
      }
      return {
        ...base,
        kind: 'photoshop',
        title: `Photoshop: ${short(instruction)}`,
        instructions: instruction,
        inputs: [
          { assetId: source.id, role: 'source' },
          ...(mask ? [{ assetId: mask.id, role: 'mask' }] : []),
          ...(ins.images.refs ?? []).map((r) => ({ assetId: r.asset.id, role: r.fromKind === 'sketch' ? 'marked' : (r.role ?? 'reference') })),
        ],
        context: { instruction, maskAssetId: mask?.id, maskRect: d.maskMode === 'rect' ? d.maskRect : undefined, keep: 'everything outside the mask pixel-identical' },
      };
    }
    case 'sketch': {
      const d = node.data;
      const source = ins.images.image?.[0]?.asset;
      if (!source) return 'Connect the image to mark up.';
      return {
        ...base,
        kind: 'sketch',
        title: 'Mark the issues in this image',
        inputs: [{ assetId: source.id, role: 'source' }, ...(d.flattened ? [{ assetId: d.flattened.id, role: 'marked' }] : [])],
        context: { notes: d.notes, marks: d.shapes.length },
      };
    }
    default:
      return 'This node does not hand off to Claude.';
  }
}

export async function createHandoff(id: string, opts: { auto?: boolean } = {}): Promise<boolean> {
  const node = getNode(id);
  if (!node) return false;
  try {
    const sessionId = await ensureSession();
    const req = await requestFor(node, sessionId);
    if (typeof req === 'string') {
      setError(id, 'Cannot create the task', req);
      return false;
    }
    const auto = opts.auto ?? (AUTO_KINDS.includes(req.kind) && routineConfigured());
    const task = await api.createTask(auto ? { ...req, auto: true } : req);
    useGraph.getState().updateData(id, { handoff: task, handoffApplied: undefined, error: undefined }, 'none');
    toast(task.run?.state === 'fired' ? 'Task sent to Claude in the cloud' : task.run?.state === 'failed' ? 'Task created; the cloud run failed — copy it into Claude' : 'Task created: copy it into Claude', task.run?.state === 'failed' ? 'error' : 'ok');
    return true;
  } catch (err) {
    const v = describeError(err);
    setError(id, v.title, v.detail, v.code);
    return false;
  }
}

export async function cancelHandoff(id: string): Promise<void> {
  const node = getNode(id);
  const task = (node?.data as { handoff?: Task } | undefined)?.handoff;
  if (!task) return;
  try {
    const t = await api.cancelTask(task.id);
    useGraph.getState().updateData(id, { handoff: t }, 'none');
  } catch (err) {
    toast(describeError(err).detail, 'error');
  }
}

export async function fireHandoff(id: string): Promise<void> {
  const task = (getNode(id)?.data as { handoff?: Task } | undefined)?.handoff;
  if (!task) return;
  try {
    const t = await api.fireTask(task.id);
    useGraph.getState().updateData(id, { handoff: t }, 'none');
    toast(t.run?.state === 'failed' ? `Cloud run failed: ${t.run.error ?? 'unknown error'}` : 'Sent to Claude in the cloud again', t.run?.state === 'failed' ? 'error' : 'ok');
  } catch (err) {
    toast(describeError(err).detail, 'error');
  }
}

/* ---------- parsing answers ---------- */

/** The JSON in an answer: a fenced ```json block, else the outermost {…} or […]. */
export function extractJson(text: string): unknown {
  const fence = /```(?:json)?\s*([\s\S]*?)```/i.exec(text);
  const body = fence?.[1] ?? text;
  const start = body.search(/[[{]/);
  if (start < 0) throw new Error('No JSON found in the answer.');
  const open = body[start];
  const end = body.lastIndexOf(open === '{' ? '}' : ']');
  if (end <= start) throw new Error('The JSON in the answer is incomplete.');
  return JSON.parse(body.slice(start, end + 1)) as unknown;
}

const obj = (v: unknown): Record<string, unknown> | null => (v && typeof v === 'object' && !Array.isArray(v) ? (v as Record<string, unknown>) : null);
const str = (v: unknown, d = ''): string => (typeof v === 'string' ? v : d);

export function asDirector(v: unknown): DirectorResponse {
  const o = obj(v);
  if (!o) throw new Error('Expected a JSON object with category, plan and prompt.');
  if (!Array.isArray(o.plan) || !o.plan.length) throw new Error('The answer has no plan.');
  if (typeof o.prompt !== 'string' || !o.prompt.trim()) throw new Error('The answer has no prompt.');
  const spec = obj(o.spec) ?? {};
  const fmt = obj(spec.format) ?? {};
  return {
    category: str(o.category, 'editorial'),
    spec: {
      use: str(spec.use),
      format: { aspectRatio: str(fmt.aspectRatio, '4:5'), resolution: str(fmt.resolution, '2K'), ...(typeof fmt.copySpace === 'string' ? { copySpace: fmt.copySpace } : {}) },
      subject: str(spec.subject),
      setting: str(spec.setting),
      light: str(spec.light),
      camera: str(spec.camera),
      medium: str(spec.medium),
      retouch: str(spec.retouch),
      composition: str(spec.composition),
      moment: str(spec.moment),
    },
    plan: o.plan.slice(0, 6).flatMap((p) => {
      const x = obj(p);
      return x && typeof x.model === 'string' ? [{ model: x.model, mode: str(x.mode, 'text2image'), why: str(x.why) }] : [];
    }),
    prompt: o.prompt,
    params: obj(o.params) ?? {},
    references: Array.isArray(o.references)
      ? o.references.flatMap((r) => {
          const x = obj(r);
          return x && typeof x.assetId === 'string' ? [{ assetId: x.assetId, label: str(x.label), role: str(x.role) }] : [];
        })
      : [],
    preserve: Array.isArray(o.preserve) ? o.preserve.filter((x): x is string => typeof x === 'string') : undefined,
    questions: Array.isArray(o.questions) ? o.questions.filter((x): x is string => typeof x === 'string') : undefined,
    notes: typeof o.notes === 'string' ? o.notes : undefined,
  };
}

function asReview(v: unknown): ReviewResponse | null {
  const o = obj(v);
  const s = obj(o?.scores);
  if (!o || !s) return null;
  const n = (x: unknown) => (typeof x === 'number' && Number.isFinite(x) ? Math.max(1, Math.min(5, x)) : undefined);
  const verdict = o.verdict === 'accept' || o.verdict === 'retry' || o.verdict === 'rethink' ? o.verdict : 'retry';
  const fix = obj(o.fix);
  return {
    scores: { realism: n(s.realism) ?? 3, skin: n(s.skin) ?? 3, light: n(s.light) ?? 3, anatomy: n(s.anatomy) ?? 3, brief: n(s.brief) ?? 3, identity: n(s.identity), preserve: n(s.preserve) },
    defects: Array.isArray(o.defects)
      ? o.defects.flatMap((d) => {
          const x = obj(d);
          return x ? [{ area: str(x.area, 'image'), problem: str(x.problem), severity: x.severity === 'major' ? ('major' as const) : ('minor' as const) }] : [];
        })
      : [],
    verdict,
    fix: fix && typeof fix.prompt === 'string' ? { prompt: fix.prompt, why: str(fix.why) } : undefined,
  };
}

/** One review, an array in image order, `{reviews:[{assetId,…}]}` or a map keyed by asset id. */
export function asReviews(v: unknown, ids: string[]): Record<string, ReviewResponse> {
  const out: Record<string, ReviewResponse> = {};
  const single = asReview(v);
  if (single) {
    if (ids[0]) out[ids[0]] = single;
    return out;
  }
  const o = obj(v);
  const list = Array.isArray(v) ? v : Array.isArray(o?.reviews) ? (o?.reviews as unknown[]) : null;
  if (list) {
    list.forEach((r, i) => {
      const x = obj(r);
      const id = typeof x?.assetId === 'string' && ids.includes(x.assetId) ? x.assetId : ids[i];
      const rv = asReview(r);
      if (id && rv) out[id] = rv;
    });
  } else if (o) {
    for (const id of ids) {
      const rv = asReview(o[id]);
      if (rv) out[id] = rv;
    }
  }
  if (!Object.keys(out).length) throw new Error('Expected a review JSON object (scores, defects, verdict).');
  return out;
}

/* ---------- applying results ---------- */

/** Applies a task's result to its node exactly like a direct run would. Returns false on a bad answer. */
export function applyTaskResult(id: string, task: Task): boolean {
  const node = getNode(id);
  const r = task.result;
  if (!node || !r) return false;
  const g = useGraph.getState();
  const done = { handoff: task, handoffApplied: r.at, error: undefined };
  if (r.error) {
    g.updateData(id, { handoff: task, handoffApplied: r.at, error: { title: 'Claude’s answer did not validate', message: r.error } }, 'none');
    return false;
  }
  try {
    const answer = () => (r.json !== undefined && r.json !== null ? r.json : extractJson(r.text ?? ''));
    switch (node.type) {
      case 'director': {
        const res = asDirector(answer());
        g.updateData(id, { ...done, result: res, prompt: res.prompt, cacheKey: cacheKeyFor(node, g.nodes, g.edges) ?? undefined }, 'none');
        return true;
      }
      case 'review': {
        const ids = (gatherInputs(id, g.nodes, g.edges).images.image ?? []).map((i) => i.asset.id);
        const reviews = asReviews(answer(), ids.length ? ids : task.inputs);
        const order = Object.keys(reviews);
        g.updateData(id, { ...done, reviews, order, current: order[0], cacheKey: cacheKeyFor(node, g.nodes, g.edges) ?? undefined }, 'none');
        return true;
      }
      case 'blender':
      case 'photoshop':
      case 'sketch':
        g.updateData(id, { ...done, ...(r.assets.length ? { results: r.assets } : {}) }, 'none');
        void queryClient.invalidateQueries({ queryKey: qk.assets });
        return true;
      default:
        return false;
    }
  } catch (err) {
    g.updateData(id, { handoff: task, handoffApplied: r.at, error: { title: 'Could not read Claude’s answer', message: err instanceof Error ? err.message : String(err) } }, 'none');
    return false;
  }
}

/** The paste box: send the answer to the task (marks it done) and apply it. */
export async function pasteAnswer(id: string, text: string): Promise<boolean> {
  const node = getNode(id);
  if (!node || !text.trim()) return false;
  let json: unknown = undefined;
  try {
    json = extractJson(text);
  } catch {
    if (node.type === 'director' || node.type === 'review') {
      setError(id, 'Could not read the answer', 'Paste the JSON block from Claude’s answer (```json … ```).');
      return false;
    }
  }
  const task = (node.data as { handoff?: Task }).handoff;
  try {
    const t: Task = task
      ? await api.taskResultJson(task.id, { text, json })
      : { ...localTask(id, node.type as TaskKind), status: 'done', result: { text, json, assets: [], at: Date.now() } };
    return applyTaskResult(id, t);
  } catch (err) {
    const v = describeError(err);
    setError(id, v.title, v.detail, v.code);
    return false;
  }
}

/** Uploaded result files (dropped on the node or picked): into the task, or straight to the node. */
export async function uploadResults(id: string, files: File[], text?: string): Promise<void> {
  const node = getNode(id);
  const images = files.filter((f) => f.type.startsWith('image/'));
  if (!node || !images.length) {
    if (files.length) toast('Only images can be returned here', 'error');
    return;
  }
  const task = (node.data as { handoff?: Task }).handoff;
  try {
    if (task && task.status !== 'cancelled') {
      const t = await api.taskResultFiles(task.id, images, text);
      applyTaskResult(id, t);
    } else {
      const assets: Asset[] = [];
      for (const f of images.slice(0, 20)) assets.push(await api.uploadAsset(f));
      useGraph.getState().updateData(id, { results: assets, error: undefined });
      void queryClient.invalidateQueries({ queryKey: qk.assets });
    }
    toast(`${images.length} result${images.length === 1 ? '' : 's'} added`, 'ok');
  } catch (err) {
    const v = describeError(err);
    setError(id, v.title, v.detail, v.code);
  }
}

function localTask(nodeId: string, kind: TaskKind): Task {
  const now = Date.now();
  return { id: 'local', sessionId: '', nodeId, kind, status: 'open', title: '', prompt: '', inputs: [], links: [], returnUrl: '', createdAt: now, updatedAt: now };
}

/** Polls the node's open task and applies the result when it arrives. */
export function useHandoffPoll(id: string, task: Task | undefined, applied: number | undefined): void {
  const q = useQuery({
    queryKey: ['task', task?.id ?? ''],
    queryFn: ({ signal }) => api.task(task?.id ?? '', signal),
    enabled: Boolean(task && task.id !== 'local' && task.status === 'open'),
    refetchInterval: 3000,
    staleTime: 0,
  });
  useEffect(() => {
    const t = q.data;
    if (!t || !task || t.id !== task.id) return;
    if (t.result && t.result.at !== applied) {
      applyTaskResult(id, t);
      toast('Claude’s answer arrived', 'ok');
    } else if (t.status !== task.status || t.updatedAt !== task.updatedAt) {
      useGraph.getState().updateData(id, { handoff: t }, 'none');
    }
  }, [q.data, task, applied, id]);
}
