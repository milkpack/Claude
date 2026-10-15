import type { Edge, Node } from '@xyflow/react';
import type { Asset, DirectorResponse, ReviewResponse } from '../api/types';

export type NodeKind = 'brief' | 'reference' | 'director' | 'generate' | 'edit' | 'review' | 'compare' | 'output';

export const NODE_KINDS: readonly NodeKind[] = [
  'brief',
  'reference',
  'director',
  'generate',
  'edit',
  'review',
  'compare',
  'output',
];

export type PortType = 'text' | 'image' | 'spec';

export type RefRole = 'identity' | 'outfit' | 'product' | 'lighting' | 'style' | 'pose' | 'background' | 'edit-source';

export const REF_ROLES: readonly { id: RefRole; label: string }[] = [
  { id: 'identity', label: 'Identity' },
  { id: 'outfit', label: 'Outfit' },
  { id: 'product', label: 'Product' },
  { id: 'lighting', label: 'Lighting' },
  { id: 'style', label: 'Style' },
  { id: 'pose', label: 'Pose' },
  { id: 'background', label: 'Background' },
  { id: 'edit-source', label: 'Edit source' },
];

export type NodeError = { message: string; code?: string; title?: string };

type Common = {
  title?: string;
  /** Shown in app mode. */
  exposed?: boolean;
};

export type BriefData = Common & { text: string };

export type ReferenceData = Common & {
  asset?: Asset;
  role: RefRole;
  uploading?: boolean;
  /** Local preview while an upload is in flight (object URL, never persisted). */
  previewUrl?: string;
  error?: NodeError;
};

export type DirectorData = Common & {
  task: 'auto' | 'generate' | 'edit';
  model?: string;
  result?: DirectorResponse;
  /** The prompt as edited by the user (starts as result.prompt). */
  prompt?: string;
  cacheKey?: string;
  error?: NodeError;
};

export type GenRun = {
  jobId: string;
  assets: Asset[];
  createdAt: number;
  finishedAt?: number;
  credits?: number;
  prompt: string;
  model: string;
  mode: string;
  params: Record<string, unknown>;
};

export type GenData = Common & {
  model?: string;
  mode?: string;
  params: Record<string, unknown>;
  /** Local prompt, used when nothing is wired into the prompt port. */
  prompt: string;
  /** Routing category hint (from the Director or a template), used to recommend models. */
  category?: string;
  runs: GenRun[];
  /** Index into runs of the run shown (default: the latest). */
  runIndex?: number;
  /** Asset ids picked to flow downstream; empty = all of the shown run. */
  picked?: string[];
  pendingJobId?: string;
  pendingSince?: number;
  cacheKey?: string;
  error?: NodeError;
  /** Edit only: the "change only / keep exactly" builder. */
  change?: string[];
  keep?: string[];
};

export type ReviewData = Common & {
  reviews: Record<string, ReviewResponse>;
  order: string[];
  current?: string;
  cacheKey?: string;
  error?: NodeError;
};

export type CompareData = Common & { winner?: string };

export type OutputData = Common & { current?: string };

export type DataByKind = {
  brief: BriefData;
  reference: ReferenceData;
  director: DirectorData;
  generate: GenData;
  edit: GenData;
  review: ReviewData;
  compare: CompareData;
  output: OutputData;
};

export type AnyData = DataByKind[NodeKind];

export type AppNode =
  | Node<BriefData, 'brief'>
  | Node<ReferenceData, 'reference'>
  | Node<DirectorData, 'director'>
  | Node<GenData, 'generate'>
  | Node<GenData, 'edit'>
  | Node<ReviewData, 'review'>
  | Node<CompareData, 'compare'>
  | Node<OutputData, 'output'>;

export type EdgeData = { type: PortType };
export type AppEdge = Edge<EdgeData, 'typed'>;

export function isKind(v: unknown): v is NodeKind {
  return typeof v === 'string' && (NODE_KINDS as readonly string[]).includes(v);
}

export function defaultData(kind: NodeKind): AnyData {
  switch (kind) {
    case 'brief':
      return { text: '' };
    case 'reference':
      return { role: 'style' };
    case 'director':
      return { task: 'auto' };
    case 'generate':
      return { params: {}, prompt: '', runs: [] };
    case 'edit':
      return { params: {}, prompt: '', runs: [], change: [], keep: [] };
    case 'review':
      return { reviews: {}, order: [] };
    case 'compare':
      return {};
    case 'output':
      return {};
  }
}

export const RUNNABLE: readonly NodeKind[] = ['director', 'generate', 'edit', 'review'];
export function isRunnable(kind: string | undefined): boolean {
  return (RUNNABLE as readonly string[]).includes(kind ?? '');
}
