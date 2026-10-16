// Executes nodes against the API. "Run node" uses cached upstream outputs and only runs upstream
// nodes that have never produced anything; "Run to here" also re-runs upstream nodes whose inputs
// changed since their output was made. Both walk the graph in topological order.
import { api } from '../api/client';
import { describeError } from '../api/errors';
import { findModel, qk, queryClient } from '../api/queries';
import type { Asset, JSONSchema, Job, ModelsResponse, ReviewResponse } from '../api/types';
import { toast, useUi } from '../state/ui';
import { getNode, useGraph } from './store';
import type { AppNode, GenData, GenRun, NodeError } from './types';
import { isRunnable } from './types';
import { createHandoff, usesHandoff } from './handoff';
import {
  cacheKeyFor,
  editPrompt,
  gatherInputs,
  generatePrompt,
  hasOutput,
  isStale,
  keepList,
  orderedRefs,
  pickMode,
  upstreamOrder,
  upstreamProducer,
} from './evaluate';

const POLL_MS = 1500;
const MAX_REVIEWS = 8;

function titleOf(n: AppNode): string {
  return n.data.title || (n.type ? n.type.charAt(0).toUpperCase() + n.type.slice(1) : 'Node');
}

function setError(id: string, err: unknown): NodeError {
  const v = describeError(err);
  const e: NodeError = { title: v.title, message: v.detail, code: v.code };
  useGraph.getState().updateData(id, { error: e }, 'none');
  return e;
}

function fail(id: string, message: string, title = 'Cannot run'): false {
  useGraph.getState().updateData(id, { error: { title, message } }, 'none');
  return false;
}

function isRunning(id: string): boolean {
  return Boolean(useGraph.getState().runtime[id]);
}

export async function runNode(id: string, mode: 'missing' | 'stale' = 'missing'): Promise<boolean> {
  const node = getNode(id);
  if (!node) return false;
  if (isRunning(id)) {
    toast(`${titleOf(node)} is already running`);
    return false;
  }
  const { nodes, edges } = useGraph.getState();
  for (const uid of upstreamOrder(id, nodes, edges)) {
    const u = getNode(uid);
    if (!u || !isRunnable(u.type)) continue;
    const g = useGraph.getState();
    const need = !hasOutput(u) || (mode === 'stale' && isStale(u, g.nodes, g.edges));
    if (!need) continue;
    if (isRunning(uid)) {
      const ok = await waitUntilIdle(uid);
      if (!ok) return false;
      continue;
    }
    const ok = await execute(uid);
    if (!ok) {
      useUi.getState().focus(uid);
      return false;
    }
  }
  if (!isRunnable(node.type)) return true;
  return execute(id);
}

export const runToHere = (id: string) => runNode(id, 'stale');

/** App mode: bring every Output node up to date. */
export async function runOutputs(): Promise<boolean> {
  const outputs = useGraph.getState().nodes.filter((n) => n.type === 'output');
  if (!outputs.length) {
    toast('This workflow has no Output node', 'error');
    return false;
  }
  let ok = true;
  for (const o of outputs) ok = (await runToHere(o.id)) && ok;
  return ok;
}

function waitUntilIdle(id: string): Promise<boolean> {
  return new Promise((resolve) => {
    const unsub = useGraph.subscribe((s) => {
      if (!s.runtime[id]) {
        unsub();
        const n = s.nodes.find((x) => x.id === id);
        resolve(Boolean(n && hasOutput(n) && !(n.data as { error?: unknown }).error));
      }
    });
  });
}

async function execute(id: string): Promise<boolean> {
  const node = getNode(id);
  if (!node) return false;
  if (usesHandoff(node)) {
    // Handoff: the answer comes back later (polled by the node); this run stops here.
    const open = (node.data as { handoff?: { status: string } }).handoff?.status === 'open';
    if (!open) await createHandoff(id);
    else toast('Waiting for Claude’s answer on this task');
    return false;
  }
  const g = useGraph.getState();
  g.updateData(id, { error: undefined }, 'none');
  g.setRuntime(id, { status: 'running', startedAt: Date.now(), phase: 'starting' });
  try {
    switch (node.type) {
      case 'director':
        return await runDirector(node);
      case 'generate':
      case 'edit':
        return await runGenerate(node);
      case 'review':
        return await runReview(node);
      default:
        return true;
    }
  } catch (err) {
    setError(id, err);
    return false;
  } finally {
    // Generation keeps its runtime entry until the job finishes (see pollJob).
    const n = getNode(id);
    if (!(n && (n.type === 'generate' || n.type === 'edit') && n.data.pendingJobId)) g.setRuntime(id, null);
  }
}

async function runDirector(node: Extract<AppNode, { type: 'director' }>): Promise<boolean> {
  const { nodes, edges } = useGraph.getState();
  const ins = gatherInputs(node.id, nodes, edges);
  const brief = (ins.text.brief ?? []).join('\n\n').trim();
  if (!brief) return fail(node.id, 'Connect a Brief node with the client’s words.', 'No brief');
  const refs = (ins.images.refs ?? []).map((r) => ({ assetId: r.asset.id, role: r.role }));
  const res = await api.director({
    brief,
    references: refs.length ? refs : undefined,
    model: node.data.model || undefined,
    task: node.data.task === 'auto' ? undefined : node.data.task,
  });
  const key = cacheKeyFor(node, nodes, edges) ?? undefined;
  useGraph.getState().updateData(node.id, { result: res, prompt: res.prompt, cacheKey: key, error: undefined }, 'none');
  return true;
}

async function models(): Promise<ModelsResponse> {
  return queryClient.fetchQuery({ queryKey: qk.models, queryFn: () => api.models(), staleTime: 10 * 60_000 });
}

async function schemaFor(model: string, mode: string): Promise<JSONSchema> {
  const r = await queryClient.fetchQuery({ queryKey: qk.form(model, mode), queryFn: () => api.form(model, mode), staleTime: 60 * 60_000 });
  return r.schema;
}

/** Params for a job: the node's values for fields the schema knows, defaults for the rest. */
export function jobParams(schema: JSONSchema, values: Record<string, unknown>, prompt: string): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  const props = schema.properties ?? {};
  for (const [k, p] of Object.entries(props)) {
    if (k === 'prompt' || k === 'visualReferences') continue;
    if (k === 'autoEnhancePrompt') {
      out[k] = false; // the director writes the prompt; the aggregator's enhancer would genericise it
      continue;
    }
    if (p.const !== undefined) {
      out[k] = p.const;
      continue;
    }
    const v = values[k];
    if (v !== undefined && v !== '' && v !== null) out[k] = v;
    else if (p.default !== undefined) out[k] = p.default;
  }
  if (props.prompt || !schema.properties) out.prompt = prompt;
  return out;
}

async function runGenerate(node: Extract<AppNode, { type: 'generate' | 'edit' }>): Promise<boolean> {
  const id = node.id;
  const { nodes, edges } = useGraph.getState();
  const ins = gatherInputs(id, nodes, edges);
  const refs = orderedRefs(node.type, ins);
  if (node.type === 'edit' && !refs.some((r) => r.isSource))
    return fail(id, 'Connect the photo to edit into the source port.', 'No source photo');
  const list = await models();
  const modelId = node.data.model || ins.spec?.plan[0]?.model;
  const model = findModel(list.models, modelId);
  if (!model) return fail(id, modelId ? `Model “${modelId}” is not available.` : 'Choose a model.', 'No model');
  const mode = pickMode(model, node.type, refs.length > 0);
  if (!mode) return fail(id, `${model.name} has no image mode.`, 'No mode');
  const schema = await schemaFor(model.id, mode);
  const prompt = node.type === 'edit' ? editPrompt(node.data, ins, refs).text : generatePrompt(node.data, ins).text;
  if (!prompt.trim()) return fail(id, 'Write a prompt, or connect a Brief or a Director.', 'No prompt');
  const maxRefs = schema.properties?.visualReferences?.maxItems;
  if (typeof maxRefs === 'number' && refs.length > maxRefs)
    return fail(id, `${model.name} takes at most ${maxRefs} reference images; ${refs.length} are connected.`, 'Too many references');
  if (refs.length && !schema.properties?.visualReferences)
    return fail(id, `${model.name} in ${mode} does not take reference images.`, 'References not supported');
  if (!refs.length && schema.required?.includes('visualReferences'))
    return fail(id, `${model.name} in ${mode} needs at least one reference image.`, 'No references');
  const maxLen = schema.properties?.prompt?.maxLength;
  if (typeof maxLen === 'number' && prompt.length > maxLen)
    return fail(id, `The prompt is ${prompt.length} characters; ${model.name} accepts ${maxLen}.`, 'Prompt too long');

  const params = jobParams(schema, node.data.params, prompt);
  const key = cacheKeyFor(node, nodes, edges) ?? undefined;
  const job = await api.createJob({
    model: model.id,
    mode,
    params,
    references: refs.length ? refs.map((r) => ({ assetId: r.asset.id, label: r.label })) : undefined,
  });
  useGraph.getState().updateData(id, { pendingJobId: job.id, pendingSince: Date.now(), error: undefined }, 'none');
  useUi.getState().trackJob(job, id, titleOf(node));
  void queryClient.invalidateQueries({ queryKey: qk.activeJobs });
  return pollJob(id, job, key);
}

/** Follows a job until it finishes; also used to resume jobs after a reload. */
export async function pollJob(id: string, first: Job | { id: string }, key?: string): Promise<boolean> {
  const g = useGraph.getState();
  const startedAt = getNodeGen(id)?.pendingSince ?? Date.now();
  g.setRuntime(id, { status: 'running', startedAt, jobId: first.id, phase: 'queued' });
  let job: Job | undefined = 'status' in first ? first : undefined;
  let failures = 0;
  try {
    for (;;) {
      const n = getNodeGen(id);
      if (!n || n.pendingJobId !== first.id) return false; // node deleted or a new run replaced this one
      if (job && (job.status === 'completed' || job.status === 'failed')) break;
      await sleep(job ? POLL_MS : 0);
      try {
        job = await api.job(first.id);
        failures = 0;
      } catch (err) {
        failures++;
        if (failures >= 5) throw err;
        await sleep(POLL_MS * failures);
        continue;
      }
      useGraph.getState().setRuntime(id, { status: 'running', startedAt, jobId: job.id, phase: job.status });
      const node = getNode(id);
      if (node) useUi.getState().trackJob(job, id, titleOf(node));
    }
    const n = getNodeGen(id);
    if (!n || !job) return false;
    if (job.status === 'failed') {
      useGraph.getState().updateData(
        id,
        { pendingJobId: undefined, pendingSince: undefined, error: { title: 'Generation failed', message: job.error || 'The provider did not return an image.', code: 'upstream' } },
        'none',
      );
      return false;
    }
    const run: GenRun = {
      jobId: job.id,
      assets: job.assets,
      createdAt: job.createdAt,
      finishedAt: job.updatedAt,
      credits: job.credits,
      prompt: typeof job.params.prompt === 'string' ? job.params.prompt : '',
      model: job.model,
      mode: job.mode,
      params: job.params,
    };
    useGraph.getState().updateData(
      id,
      (d) => ({
        runs: [...(d as GenData).runs, run].slice(-12),
        runIndex: undefined,
        picked: [],
        pendingJobId: undefined,
        pendingSince: undefined,
        cacheKey: key ?? (d as GenData).cacheKey,
        error: undefined,
      }),
      'none',
    );
    void queryClient.invalidateQueries({ queryKey: qk.assets });
    void queryClient.invalidateQueries({ queryKey: qk.me });
    return true;
  } catch (err) {
    useGraph.getState().updateData(id, { pendingJobId: undefined, pendingSince: undefined }, 'none');
    setError(id, err);
    return false;
  } finally {
    useGraph.getState().setRuntime(id, null);
    void queryClient.invalidateQueries({ queryKey: qk.activeJobs });
  }
}

function getNodeGen(id: string): GenData | undefined {
  const n = getNode(id);
  return n && (n.type === 'generate' || n.type === 'edit') ? n.data : undefined;
}

/** After loading a workflow, keep following jobs that were still running when it was saved. */
export function resumePending(): void {
  for (const n of useGraph.getState().nodes) {
    if ((n.type === 'generate' || n.type === 'edit') && n.data.pendingJobId && !useGraph.getState().runtime[n.id]) {
      void pollJob(n.id, { id: n.data.pendingJobId });
    }
  }
}

async function runReview(node: Extract<AppNode, { type: 'review' }>): Promise<boolean> {
  const id = node.id;
  const { nodes, edges } = useGraph.getState();
  const ins = gatherInputs(id, nodes, edges);
  const images = (ins.images.image ?? []).slice(0, MAX_REVIEWS);
  if (!images.length) return fail(id, 'Connect generated images to review. Run the upstream node first.', 'Nothing to review');
  const brief = (ins.text.brief ?? []).join('\n\n').trim() || undefined;
  const source = ins.images.source?.[0]?.asset;
  const producer = upstreamProducer(id, nodes, edges);
  let preserve: string[] | undefined;
  if (producer?.type === 'edit') {
    const pins = gatherInputs(producer.id, nodes, edges);
    const k = keepList(producer.data, pins);
    preserve = k.length ? k : undefined;
  }
  const key = cacheKeyFor(node, nodes, edges) ?? undefined;
  const reviews: GenDataReviews = {};
  const order: string[] = [];
  const g = useGraph.getState();
  g.updateData(id, { reviews: {}, order: [], current: undefined }, 'none');
  for (let i = 0; i < images.length; i++) {
    const img = images[i];
    if (!img) continue;
    g.setRuntime(id, { status: 'running', startedAt: Date.now(), phase: 'reviewing', done: i, total: images.length });
    const r = await api.review({
      assetId: img.asset.id,
      brief,
      prompt: img.asset.prompt,
      sourceAssetId: source?.id,
      preserve,
    });
    reviews[img.asset.id] = r;
    order.push(img.asset.id);
    useGraph.getState().updateData(id, { reviews: { ...reviews }, order: [...order], current: order[0] }, 'none');
  }
  useGraph.getState().updateData(id, { cacheKey: key, error: undefined }, 'none');
  return true;
}

type GenDataReviews = Record<string, ReviewResponse>;

function sleep(ms: number): Promise<void> {
  return new Promise((r) => window.setTimeout(r, ms));
}

export function assetsOfRun(d: GenData): Asset[] {
  const i = d.runIndex ?? d.runs.length - 1;
  return d.runs[i]?.assets ?? [];
}
