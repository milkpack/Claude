// The server contract (apps/photo-studio/DESIGN.md, "API"). Keep in step with the Rust server.

export type OpenArtStatus = {
  connected: boolean;
  needsReconnect: boolean;
  email?: string;
  plan?: string;
  credits?: number;
};

export type Me = {
  authenticated: boolean;
  openart: OpenArtStatus;
  claude: { configured: boolean; model: string };
};

export type ModelMode = { mode: string; description: string; credits: number; affordable: boolean };

export type Model = {
  id: string;
  name: string;
  description: string;
  modes: { image?: ModelMode[]; video?: ModelMode[] };
  categories: string[];
};

export type ModelsResponse = { balance: number; models: Model[] };

/** The subset of JSON Schema the OpenArt forms use. Unknown keys are tolerated. */
export type JSONSchema = {
  type?: string | string[];
  properties?: Record<string, JSONSchema>;
  required?: string[];
  enum?: unknown[];
  const?: unknown;
  default?: unknown;
  minimum?: number;
  maximum?: number;
  exclusiveMinimum?: number;
  exclusiveMaximum?: number;
  minLength?: number;
  maxLength?: number;
  maxItems?: number;
  minItems?: number;
  items?: JSONSchema;
  description?: string;
  title?: string;
  [key: string]: unknown;
};

export type FormResponse = { schema: JSONSchema };

export type CostResponse = { credits: number; config: Record<string, unknown>; affordable: boolean };

export type Asset = {
  id: string;
  kind: 'upload' | 'generation';
  file: string;
  thumb: string;
  width: number;
  height: number;
  createdAt: number;
  openart?: { id: string; url: string };
  prompt?: string;
  model?: string;
  mode?: string;
  params?: Record<string, unknown>;
  jobId?: string;
};

export type AssetsPage = { assets: Asset[]; next?: string };

export type RefInput = { assetId: string; label?: string };

export type JobStatus = 'queued' | 'running' | 'completed' | 'failed';

export type Job = {
  id: string;
  status: JobStatus;
  model: string;
  mode: string;
  params: Record<string, unknown>;
  references: RefInput[];
  historyId?: string;
  credits?: number;
  assets: Asset[];
  error?: string;
  createdAt: number;
  updatedAt: number;
};

export type JobRequest = {
  model: string;
  mode: string;
  params: Record<string, unknown>;
  references?: RefInput[];
};

export type ShotSpec = {
  use: string;
  format: { aspectRatio: string; resolution: string; copySpace?: string };
  subject: string;
  setting: string;
  light: string;
  camera: string;
  medium: string;
  retouch: string;
  composition: string;
  moment: string;
};

export type DirectorRequest = {
  brief: string;
  references?: { assetId: string; role?: string }[];
  model?: string;
  task?: 'generate' | 'edit';
};

export type PlanStep = { model: string; mode: string; why: string };

export type DirectorResponse = {
  category: string;
  spec: ShotSpec;
  plan: PlanStep[];
  prompt: string;
  params: Record<string, unknown>;
  references: { assetId: string; label: string; role: string }[];
  preserve?: string[];
  questions?: string[];
  notes?: string;
};

export type ReviewRequest = {
  assetId: string;
  brief?: string;
  prompt?: string;
  sourceAssetId?: string;
  preserve?: string[];
};

export type ReviewScores = {
  realism: number;
  skin: number;
  light: number;
  anatomy: number;
  brief: number;
  identity?: number;
  preserve?: number;
};

export type Defect = { area: string; problem: string; severity: 'minor' | 'major' };

export type Verdict = 'accept' | 'retry' | 'rethink';

export type ReviewResponse = {
  scores: ReviewScores;
  defects: Defect[];
  verdict: Verdict;
  fix?: { prompt: string; why: string };
};

export type WorkflowGraph = { nodes: unknown[]; edges: unknown[]; viewport?: unknown };

export type Workflow = { id: string; name: string; graph: WorkflowGraph; updatedAt: number };

export type WorkflowSummary = { id: string; name: string; updatedAt: number };

export type ApiErrorCode =
  | 'unauthenticated'
  | 'openart_not_connected'
  | 'openart_reconnect'
  | 'claude_not_configured'
  | 'bad_request'
  | 'upstream'
  | 'not_found'
  | 'rate_limited';

export type ApiErrorBody = { error: string; code?: string };
