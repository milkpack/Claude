import type {
  ApiErrorBody,
  ApiErrorCode,
  Asset,
  AssetsPage,
  CostResponse,
  DirectorRequest,
  DirectorResponse,
  FormResponse,
  Job,
  JobRequest,
  Me,
  ModelsResponse,
  ReviewRequest,
  ReviewResponse,
  RoutineCheck,
  RoutineSettings,
  SessionMeta,
  Task,
  TaskRequest,
  TaskStatus,
  Workflow,
  WorkflowGraph,
  WorkflowSummary,
} from './types';

export const MOCK = import.meta.env.VITE_MOCK === '1';

const KNOWN_CODES: readonly ApiErrorCode[] = [
  'unauthenticated',
  'openart_not_connected',
  'openart_reconnect',
  'claude_not_configured',
  'bad_request',
  'upstream',
  'not_found',
  'rate_limited',
];

export class ApiError extends Error {
  readonly status: number;
  readonly code: ApiErrorCode | undefined;

  constructor(status: number, message: string, code?: string) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
    this.code = KNOWN_CODES.find((c) => c === code) ?? codeFromStatus(status);
  }
}

function codeFromStatus(status: number): ApiErrorCode | undefined {
  if (status === 401) return 'unauthenticated';
  if (status === 404) return 'not_found';
  if (status === 429) return 'rate_limited';
  if (status === 400 || status === 422) return 'bad_request';
  if (status === 502 || status === 504) return 'upstream';
  return undefined;
}

type Listener = (err: ApiError) => void;
const unauthListeners = new Set<Listener>();
/** Called whenever a request comes back unauthenticated (the session expired). */
export function onUnauthenticated(fn: Listener): () => void {
  unauthListeners.add(fn);
  return () => unauthListeners.delete(fn);
}

type RequestOptions = {
  method?: 'GET' | 'POST' | 'PUT' | 'DELETE';
  body?: unknown;
  form?: FormData;
  signal?: AbortSignal;
};

async function request<T>(path: string, opts: RequestOptions = {}): Promise<T> {
  const headers: Record<string, string> = { Accept: 'application/json' };
  let body: BodyInit | undefined;
  if (opts.form) {
    body = opts.form;
  } else if (opts.body !== undefined) {
    headers['Content-Type'] = 'application/json';
    body = JSON.stringify(opts.body);
  } else if (opts.method && opts.method !== 'GET') {
    // State-changing routes require a JSON content type as a CSRF guard, even without a body.
    headers['Content-Type'] = 'application/json';
    body = '{}';
  }
  let res: Response;
  try {
    res = await fetch(path, {
      method: opts.method ?? 'GET',
      headers,
      body,
      credentials: 'same-origin',
      signal: opts.signal,
    });
  } catch (e) {
    if (e instanceof DOMException && e.name === 'AbortError') throw e;
    throw new ApiError(0, 'The server is not reachable. Check that Photo Studio is running.');
  }
  const text = await res.text();
  let data: unknown = undefined;
  if (text) {
    try {
      data = JSON.parse(text);
    } catch {
      data = text;
    }
  }
  if (!res.ok) {
    const b = (data && typeof data === 'object' ? data : {}) as Partial<ApiErrorBody>;
    const message = typeof b.error === 'string' && b.error ? b.error : `${res.status} ${res.statusText || 'error'}`;
    const err = new ApiError(res.status, message, typeof b.code === 'string' ? b.code : undefined);
    if (err.code === 'unauthenticated' && path !== '/api/login') unauthListeners.forEach((fn) => fn(err));
    throw err;
  }
  return data as T;
}

function qs(params: Record<string, string | number | undefined>): string {
  const u = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) if (v !== undefined && v !== '') u.set(k, String(v));
  const s = u.toString();
  return s ? `?${s}` : '';
}

const enc = encodeURIComponent;

export const api = {
  me: () => request<Me>('/api/me'),
  login: (password: string) => request<Me>('/api/login', { method: 'POST', body: { password } }),
  logout: () => request<object>('/api/logout', { method: 'POST' }),
  disconnectOpenArt: () => request<Me>('/api/openart/disconnect', { method: 'POST' }),

  models: (refresh = false) => request<ModelsResponse>(`/api/models${refresh ? '?refresh=1' : ''}`),
  form: (model: string, mode: string) => request<FormResponse>(`/api/models/${enc(model)}/${enc(mode)}/form`),
  cost: (model: string, mode: string, params: Record<string, unknown>, signal?: AbortSignal) =>
    request<CostResponse>('/api/cost', { method: 'POST', body: { model, mode, params }, signal }),

  uploadAsset: (file: File) => {
    const form = new FormData();
    form.append('file', file);
    return request<Asset>('/api/assets', { method: 'POST', form });
  },
  assets: (cursor?: string, limit = 60) => request<AssetsPage>(`/api/assets${qs({ cursor, limit })}`),
  asset: (id: string) => request<Asset>(`/api/assets/${enc(id)}`),
  deleteAsset: (id: string) => request<object>(`/api/assets/${enc(id)}`, { method: 'DELETE' }),

  createJob: (job: JobRequest) => request<Job>('/api/jobs', { method: 'POST', body: job }),
  job: (id: string, signal?: AbortSignal) => request<Job>(`/api/jobs/${enc(id)}`, { signal }),
  activeJobs: () => request<{ jobs: Job[] }>('/api/jobs?active=1'),

  director: (req: DirectorRequest) => request<DirectorResponse>('/api/director', { method: 'POST', body: req }),
  review: (req: ReviewRequest) => request<ReviewResponse>('/api/review', { method: 'POST', body: req }),

  workflows: () => request<{ workflows: WorkflowSummary[] }>('/api/workflows'),
  workflow: (id: string) => request<Workflow>(`/api/workflows/${enc(id)}`),
  createWorkflow: (name: string, graph: WorkflowGraph, meta: SessionMeta = {}) =>
    request<Workflow>('/api/workflows', { method: 'POST', body: { name, graph, kind: 'session', ...meta } }),
  saveWorkflow: (id: string, name: string, graph: WorkflowGraph, meta: SessionMeta = {}) =>
    request<Workflow>(`/api/workflows/${enc(id)}`, { method: 'PUT', body: { name, graph, kind: 'session', ...meta } }),
  deleteWorkflow: (id: string) => request<object>(`/api/workflows/${enc(id)}`, { method: 'DELETE' }),

  createTask: (req: TaskRequest) => request<Task>('/api/tasks', { method: 'POST', body: req }),
  tasks: (sessionId: string, status?: TaskStatus) => request<{ tasks: Task[] }>(`/api/tasks${qs({ sessionId, status })}`),
  task: (id: string, signal?: AbortSignal) => request<Task>(`/api/tasks/${enc(id)}`, { signal }),
  taskResultJson: (id: string, body: { text?: string; json?: unknown }) =>
    request<Task>(`/api/tasks/${enc(id)}/result`, { method: 'POST', body }),
  taskResultFiles: (id: string, files: File[], text?: string) => {
    const form = new FormData();
    for (const f of files.slice(0, 20)) form.append('files', f);
    if (text) form.append('text', text);
    return request<Task>(`/api/tasks/${enc(id)}/result`, { method: 'POST', form });
  },
  taskFileUrl: (id: string, name: string) => `/api/tasks/${enc(id)}/files/${enc(name)}`,
  duplicateWorkflow: (id: string, name?: string) =>
    request<Workflow>(`/api/workflows/${enc(id)}/duplicate`, { method: 'POST', body: name ? { name } : {} }),
  fireTask: (id: string) => request<Task>(`/api/tasks/${enc(id)}/fire`, { method: 'POST' }),
  routine: () => request<RoutineSettings>('/api/settings/routine'),
  /** token omitted keeps the saved one; proxy null removes it. */
  saveRoutine: (body: { url: string; token?: string; proxy?: string | null }) =>
    request<RoutineSettings>('/api/settings/routine', { method: 'POST', body }),
  checkRoutine: () => request<RoutineCheck>('/api/settings/routine/check'),
  deleteRoutine: () => request<object>('/api/settings/routine', { method: 'DELETE' }),
  cancelTask: (id: string) => request<Task>(`/api/tasks/${enc(id)}/cancel`, { method: 'POST' }),
};

/**
 * Connect (or reconnect) OpenArt. The real server does this with a browser redirect through
 * OpenArt's consent page and back to `/?openart=connected`; the mock answers in place.
 */
export async function startOpenArtConnect(): Promise<'redirected' | 'connected'> {
  if (MOCK) {
    await fetch('/oauth/openart/start', { credentials: 'same-origin' });
    return 'connected';
  }
  window.location.assign('/oauth/openart/start');
  return 'redirected';
}
