// In-browser mock of the whole Photo Studio API (`vite --mode mock`). It patches window.fetch for
// /api, /oauth and /healthz and keeps everything in memory. Flags via `?mock=a,b`:
//   loggedout  start signed out          noopenart  OpenArt not connected
//   reconnect  OpenArt needs reconnect   noclaude   no ANTHROPIC_API_KEY
//   broke      a low credit balance      empty      no seeded library
//   fast / slow  job speed
import type { Asset, Job, Me, Workflow } from '../api/types';
import { direct, review } from './brain';
import { MOCK_MODELS, modesOf, outputSize, priceOf, schemaFor } from './models';
import type { Grade, Style } from './paint';
import { GRADES, gradeFromWords, loadImage, paint, regrade, sampleGrade, scaled, seedOf, styleFromWords, toBlobUrl } from './paint';

const flags = new Set(
  (new URLSearchParams(window.location.search).get('mock') ?? '')
    .split(',')
    .map((s) => s.trim())
    .filter(Boolean),
);
const speed = flags.has('fast') ? 0.4 : flags.has('slow') ? 3 : 1;

const state = {
  authenticated: !flags.has('loggedout'),
  failedLogins: [] as number[],
  openart: { connected: !flags.has('noopenart'), needsReconnect: flags.has('reconnect'), email: 'studio@example.com', plan: 'Infinite' },
  claude: !flags.has('noclaude'),
  balance: flags.has('broke') ? 120 : 12_480,
  assets: [] as Asset[],
  jobs: new Map<string, Job>(),
  workflows: new Map<string, Workflow>(),
  seq: 1,
};

let seeded: Promise<void> | null = null;

function id(prefix: string): string {
  return `${prefix}_${(state.seq++).toString(36)}${Math.random().toString(36).slice(2, 8)}`;
}

function me(): Me {
  return {
    authenticated: state.authenticated,
    openart: state.authenticated
      ? {
          connected: state.openart.connected,
          needsReconnect: state.openart.connected && state.openart.needsReconnect,
          ...(state.openart.connected ? { email: state.openart.email, plan: state.openart.plan, credits: state.balance } : {}),
        }
      : { connected: false, needsReconnect: false },
    claude: { configured: state.claude, model: 'claude-opus-5-5' },
  };
}

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } });
const err = (status: number, error: string, code?: string) => json(status, { error, ...(code ? { code } : {}) });
const sleep = (ms: number) => new Promise((r) => window.setTimeout(r, ms));

async function makeAsset(canvas: HTMLCanvasElement, meta: Omit<Asset, 'id' | 'file' | 'thumb' | 'createdAt'>, assetId = id('ast')): Promise<Asset> {
  const file = await toBlobUrl(scaled(canvas, 1400), 'image/jpeg', 0.9);
  const thumb = await toBlobUrl(scaled(canvas, 420), 'image/jpeg', 0.82);
  return { id: assetId, file, thumb, createdAt: Date.now(), ...meta };
}

function sized(params: Record<string, unknown>, longPx = 1400) {
  const out = outputSize(params);
  const k = longPx / Math.max(out.width, out.height);
  return { out, w: Math.round(out.width * k), h: Math.round(out.height * k) };
}

async function seed(): Promise<void> {
  if (flags.has('empty')) return;
  const uploads: { style: Style; grade: Grade; w: number; h: number; s: number }[] = [
    { style: 'portrait', grade: GRADES.daylight!, w: 2400, h: 3000, s: 11 },
    { style: 'landscape', grade: GRADES.dusk!, w: 3000, h: 2000, s: 4 },
    { style: 'product', grade: GRADES.amber!, w: 2400, h: 3000, s: 5 },
    { style: 'interior', grade: GRADES.warm!, w: 3000, h: 2000, s: 9 },
    { style: 'portrait', grade: GRADES.cool!, w: 2400, h: 3000, s: 23 },
  ];
  const now = Date.now();
  const made: Asset[] = [];
  for (const [i, u] of uploads.entries()) {
    const k = 1400 / Math.max(u.w, u.h);
    const c = paint({ seed: u.s, width: u.w * k, height: u.h * k, style: u.style, grade: u.grade });
    const a = await makeAsset(c, { kind: 'upload', width: u.w, height: u.h, openart: { id: `up-${i}`, url: 'https://cdn.example/upload' } });
    made.push({ ...a, createdAt: now - (10 - i) * 3_600_000 });
  }
  const prompt = 'Editorial portrait, low warm side light, soft falloff, natural skin texture.';
  const params = { aspectRatio: '4:5', resolution: '2K', imageCount: 4 };
  const { out, w, h } = sized(params);
  for (let i = 0; i < 4; i++) {
    const c = paint({ seed: 300 + i, width: w, height: h, style: 'portrait', grade: GRADES.warm! });
    const a = await makeAsset(c, { kind: 'generation', width: out.width, height: out.height, prompt, model: 'nano-banana-2-1', mode: 'text2image', params: { ...params, prompt }, jobId: 'job_seed', openart: { id: `cr-${i}`, url: 'https://cdn.example/creation' } });
    made.push({ ...a, createdAt: now - 3_000_000 + i });
  }
  state.assets = made.sort((a, b) => b.createdAt - a.createdAt);
}

/* ---------- jobs ---------- */

function startJob(job: Job): void {
  const count = job.params.resultType === 'series' ? Number(job.params.seriesAmount ?? 4) : Number(job.params.imageCount ?? 1);
  const n = Math.min(9, Math.max(1, Number.isFinite(count) ? Math.round(count) : 1));
  const duration = (2600 + n * 420) * speed;
  window.setTimeout(() => {
    job.status = 'running';
    job.historyId = id('hist');
    job.updatedAt = Date.now();
  }, 500 * speed);
  window.setTimeout(() => {
    void finishJob(job, n);
  }, duration);
}

async function finishJob(job: Job, n: number): Promise<void> {
  const prompt = String(job.params.prompt ?? '');
  if (/\bfail\b/i.test(prompt)) {
    job.status = 'failed';
    job.error = 'upstream: the provider rejected the prompt (content policy). Nothing was charged.';
    job.updatedAt = Date.now();
    return;
  }
  try {
    const { out, w, h } = sized(job.params);
    const refAssets = job.references.map((r) => state.assets.find((a) => a.id === r.assetId)).filter((a): a is Asset => Boolean(a));
    const style = styleFromWords(prompt);
    let source: HTMLImageElement | null = null;
    let grade: Grade = gradeFromWords(prompt);
    if (job.mode === 'image2image' && refAssets[0] && style !== 'poster' && style !== 'product') {
      source = await loadImage(refAssets[0].file);
      if (refAssets[1]) grade = sampleGrade(await loadImage(refAssets[1].file));
    }
    const quoted = /[“"]([^”"]{2,40})[”"]/.exec(prompt)?.[1];
    const sub = /[“"][^”"]+[”"][^“"]*[“"]([^”"]{2,40})[”"]/.exec(prompt)?.[1] ?? '';
    const assets: Asset[] = [];
    for (let i = 0; i < n; i++) {
      const s = seedOf(job.id) + i * 7919;
      const canvas = source
        ? regrade(source, { seed: s, width: w, height: h, grade, strength: 0.85 })
        : paint({ seed: s, width: w, height: h, style, grade, text: { headline: (quoted ?? 'BLUE HOUR').toUpperCase(), sub } });
      assets.push(
        await makeAsset(canvas, {
          kind: 'generation',
          width: out.width,
          height: out.height,
          prompt,
          model: job.model,
          mode: job.mode,
          params: job.params,
          jobId: job.id,
          openart: { id: id('cr'), url: 'https://cdn.example/creation' },
        }),
      );
    }
    state.assets = [...assets.reverse(), ...state.assets];
    state.balance = Math.max(0, state.balance - (job.credits ?? 0));
    job.assets = assets.reverse();
    job.status = 'completed';
  } catch (e) {
    job.status = 'failed';
    job.error = `mock: ${e instanceof Error ? e.message : String(e)}`;
  }
  job.updatedAt = Date.now();
}

/* ---------- router ---------- */

async function body(init?: RequestInit): Promise<Record<string, unknown>> {
  if (!init?.body || typeof init.body !== 'string') return {};
  try {
    const v: unknown = JSON.parse(init.body);
    return v && typeof v === 'object' ? (v as Record<string, unknown>) : {};
  } catch {
    return {};
  }
}

function openartGuard(): Response | null {
  if (!state.openart.connected) return err(409, 'OpenArt is not connected.', 'openart_not_connected');
  if (state.openart.needsReconnect) return err(409, 'The OpenArt session expired.', 'openart_reconnect');
  return null;
}

async function route(url: URL, init?: RequestInit): Promise<Response> {
  const method = (init?.method ?? 'GET').toUpperCase();
  const path = url.pathname;
  await sleep((90 + Math.random() * 160) * Math.min(speed, 1.5));

  if (path === '/healthz') return new Response('ok', { status: 200 });
  if (path === '/api/me') return json(200, me());
  if (path === '/api/login' && method === 'POST') {
    const now = Date.now();
    state.failedLogins = state.failedLogins.filter((t) => now - t < 60_000);
    if (state.failedLogins.length >= 5) return err(429, 'Too many attempts. Wait a minute and try again.', 'rate_limited');
    const b = await body(init);
    if (typeof b.password !== 'string' || !b.password || b.password === 'wrong') {
      state.failedLogins.push(now);
      return err(401, 'Wrong password.', 'unauthenticated');
    }
    state.authenticated = true;
    return json(200, me());
  }
  if (!state.authenticated) return err(401, 'Not signed in.', 'unauthenticated');
  await (seeded ??= seed());

  if (path === '/oauth/openart/start') {
    state.openart.connected = true;
    state.openart.needsReconnect = false;
    return json(200, { ok: true });
  }
  if (path === '/api/logout' && method === 'POST') {
    state.authenticated = false;
    return json(200, {});
  }
  if (path === '/api/openart/disconnect' && method === 'POST') {
    state.openart.connected = false;
    return json(200, me());
  }

  if (path === '/api/models') {
    const g = openartGuard();
    if (g) return g;
    return json(200, {
      balance: state.balance,
      models: MOCK_MODELS.map((m) => ({
        id: m.id,
        name: m.name,
        description: m.description,
        categories: m.categories,
        modes: {
          image: modesOf(m).map((mode) => {
            const credits = priceOf(m, mode, {}).credits;
            return { mode, description: mode === 'text2image' ? 'Text to image' : 'Image to image', credits, affordable: credits <= state.balance };
          }),
        },
      })),
    });
  }
  const form = /^\/api\/models\/([^/]+)\/([^/]+)\/form$/.exec(path);
  if (form) {
    const m = MOCK_MODELS.find((x) => x.id === decodeURIComponent(form[1] ?? ''));
    const mode = decodeURIComponent(form[2] ?? '');
    if (!m || !modesOf(m).includes(mode)) return err(404, 'Unknown model or mode.', 'not_found');
    return json(200, { schema: schemaFor(m, mode) });
  }
  if (path === '/api/cost' && method === 'POST') {
    const b = await body(init);
    const m = MOCK_MODELS.find((x) => x.id === b.model);
    if (!m) return err(400, 'Unknown model.', 'bad_request');
    const { credits, config } = priceOf(m, String(b.mode ?? 'text2image'), (b.params as Record<string, unknown>) ?? {});
    return json(200, { credits, config, affordable: credits <= state.balance });
  }

  if (path === '/api/assets' && method === 'POST') {
    const fd = init?.body instanceof FormData ? init.body : null;
    const file = fd?.get('file');
    if (!(file instanceof File)) return err(400, 'Missing file.', 'bad_request');
    if (!['image/png', 'image/jpeg', 'image/webp'].includes(file.type)) return err(400, 'Only PNG, JPEG and WebP.', 'bad_request');
    if (file.size > 30 * 1024 * 1024) return err(400, 'Larger than 30 MB.', 'bad_request');
    const g = openartGuard();
    if (g) return g;
    await sleep(500 * speed);
    const url = URL.createObjectURL(file);
    const img = await loadImage(url);
    const thumb = await toBlobUrl(scaled(img, 420), 'image/jpeg', 0.82);
    const a: Asset = { id: id('ast'), kind: 'upload', file: url, thumb, width: img.naturalWidth, height: img.naturalHeight, createdAt: Date.now(), openart: { id: id('up'), url: 'https://cdn.example/upload' } };
    state.assets = [a, ...state.assets];
    return json(200, a);
  }
  if (path === '/api/assets' && method === 'GET') {
    const start = Number(url.searchParams.get('cursor') ?? 0) || 0;
    const limit = Math.min(200, Math.max(1, Number(url.searchParams.get('limit') ?? 60) || 60));
    const page = state.assets.slice(start, start + limit);
    const next = start + limit < state.assets.length ? String(start + limit) : undefined;
    return json(200, { assets: page, ...(next ? { next } : {}) });
  }
  const asset = /^\/api\/assets\/([^/]+)$/.exec(path);
  if (asset) {
    const a = state.assets.find((x) => x.id === decodeURIComponent(asset[1] ?? ''));
    if (!a) return err(404, 'Asset not found.', 'not_found');
    if (method === 'DELETE') {
      state.assets = state.assets.filter((x) => x !== a);
      return json(200, {});
    }
    return json(200, a);
  }

  if (path === '/api/jobs' && method === 'POST') {
    const g = openartGuard();
    if (g) return g;
    const b = await body(init);
    const m = MOCK_MODELS.find((x) => x.id === b.model);
    const mode = String(b.mode ?? '');
    if (!m || !modesOf(m).includes(mode)) return err(400, 'Unknown model or mode.', 'bad_request');
    const params = { ...((b.params as Record<string, unknown>) ?? {}) };
    const refs = Array.isArray(b.references) ? (b.references as { assetId: string; label?: string }[]) : [];
    if (mode === 'image2image' && !refs.length) return err(400, 'image2image needs at least one reference.', 'bad_request');
    const { credits } = priceOf(m, mode, params);
    if (credits > state.balance) return err(400, `Not enough credits: this job costs ${credits}, the balance is ${state.balance}.`, 'bad_request');
    if (refs.length) params.visualReferences = refs.map((r, i) => ({ type: 'image', id: r.assetId, url: 'https://cdn.example/ref', label: r.label ?? `image${i + 1}` }));
    const now = Date.now();
    const job: Job = { id: id('job'), status: 'queued', model: m.id, mode, params, references: refs, credits, assets: [], createdAt: now, updatedAt: now };
    state.jobs.set(job.id, job);
    startJob(job);
    return json(200, job);
  }
  if (path === '/api/jobs' && method === 'GET') {
    const active = url.searchParams.get('active') === '1';
    const jobs = [...state.jobs.values()].filter((j) => !active || j.status === 'queued' || j.status === 'running').sort((a, b) => b.createdAt - a.createdAt);
    return json(200, { jobs });
  }
  const job = /^\/api\/jobs\/([^/]+)$/.exec(path);
  if (job) {
    const j = state.jobs.get(decodeURIComponent(job[1] ?? ''));
    return j ? json(200, structuredClone(j)) : err(404, 'Job not found.', 'not_found');
  }

  if (path === '/api/director' && method === 'POST') {
    if (!state.claude) return err(503, 'Claude is not configured on the server.', 'claude_not_configured');
    const b = await body(init);
    if (typeof b.brief !== 'string' || !b.brief.trim()) return err(400, 'The brief is empty.', 'bad_request');
    await sleep(1400 * speed);
    return json(200, direct(b as unknown as Parameters<typeof direct>[0]));
  }
  if (path === '/api/review' && method === 'POST') {
    if (!state.claude) return err(503, 'Claude is not configured on the server.', 'claude_not_configured');
    const b = await body(init);
    if (typeof b.assetId !== 'string') return err(400, 'assetId is required.', 'bad_request');
    await sleep(700 * speed);
    return json(200, review(b as unknown as Parameters<typeof review>[0]));
  }

  if (path === '/api/workflows' && method === 'GET') {
    const workflows = [...state.workflows.values()].map(({ id: wid, name, updatedAt }) => ({ id: wid, name, updatedAt })).sort((a, b) => b.updatedAt - a.updatedAt);
    return json(200, { workflows });
  }
  if (path === '/api/workflows' && method === 'POST') {
    const b = await body(init);
    const w: Workflow = { id: id('wf'), name: String(b.name ?? 'Untitled workflow'), graph: (b.graph as Workflow['graph']) ?? { nodes: [], edges: [] }, updatedAt: Date.now() };
    state.workflows.set(w.id, w);
    return json(200, w);
  }
  const wf = /^\/api\/workflows\/([^/]+)$/.exec(path);
  if (wf) {
    const wid = decodeURIComponent(wf[1] ?? '');
    const w = state.workflows.get(wid);
    if (!w) return err(404, 'Workflow not found.', 'not_found');
    if (method === 'DELETE') {
      state.workflows.delete(wid);
      return json(200, {});
    }
    if (method === 'PUT') {
      const b = await body(init);
      const next: Workflow = { ...w, name: typeof b.name === 'string' ? b.name : w.name, graph: (b.graph as Workflow['graph']) ?? w.graph, updatedAt: Date.now() };
      state.workflows.set(wid, next);
      return json(200, next);
    }
    return json(200, structuredClone(w));
  }
  return err(404, `No mock route for ${method} ${path}`, 'not_found');
}

export function installMock(): void {
  const original = window.fetch.bind(window);
  window.fetch = async (input: RequestInfo | URL, init?: RequestInit) => {
    const raw = typeof input === 'string' ? input : input instanceof URL ? input.href : input.url;
    const url = new URL(raw, window.location.href);
    if (url.origin === window.location.origin && (url.pathname.startsWith('/api/') || url.pathname.startsWith('/oauth/') || url.pathname === '/healthz')) {
      if (init?.signal?.aborted) throw new DOMException('Aborted', 'AbortError');
      return route(url, init);
    }
    return original(input, init);
  };
  console.warn('[photo-studio] mock API active (in-memory, placeholder images drawn in code)');
}
