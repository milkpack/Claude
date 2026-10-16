import type { Edge, Node } from '@xyflow/react';
import type { Asset, DirectorResponse, ReviewResponse, Task } from '../api/types';

export type NodeKind =
  | 'brief'
  | 'reference'
  | 'director'
  | 'generate'
  | 'edit'
  | 'review'
  | 'compare'
  | 'output'
  | 'blender'
  | 'photoshop'
  | 'sketch';

export const NODE_KINDS: readonly NodeKind[] = [
  'brief',
  'reference',
  'director',
  'generate',
  'edit',
  'review',
  'compare',
  'output',
  'blender',
  'photoshop',
  'sketch',
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

/** A Claude handoff attached to a node (DESIGN.md, "Handoff mode"). */
export type Handoff = {
  /** Latest snapshot of the node's task. */
  handoff?: Task;
  /** `result.at` of the task result already applied to the node. */
  handoffApplied?: number;
};

export type Via = 'handoff' | 'direct';

export type BriefData = Common & { text: string };

export type ReferenceData = Common & {
  asset?: Asset;
  role: RefRole;
  uploading?: boolean;
  /** Local preview while an upload is in flight (object URL, never persisted). */
  previewUrl?: string;
  error?: NodeError;
};

export type DirectorData = Common &
  Handoff & {
  via?: Via;
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

export type ReviewData = Common &
  Handoff & {
  via?: Via;
  reviews: Record<string, ReviewResponse>;
  order: string[];
  current?: string;
  cacheKey?: string;
  error?: NodeError;
};

export type CompareData = Common & { winner?: string };

export type BlenderData = Common &
  Handoff & {
    subject: string;
    set: string;
    camera: string;
    lens: string;
    light: string;
    views: string[];
    notes?: string;
    results: Asset[];
    error?: NodeError;
  };

export type MaskRect = { x: number; y: number; w: number; h: number };

export type PhotoshopData = Common &
  Handoff & {
    instruction: string;
    maskMode: 'paint' | 'rect';
    /** Painted mask at display resolution (PNG data URL, white = change). */
    maskUrl?: string;
    /** Rectangle mask in 0–1 image coordinates. */
    maskRect?: MaskRect;
    brush: number;
    /** Full-resolution mask uploaded with the last task. */
    maskAsset?: Asset;
    results: Asset[];
    error?: NodeError;
  };

export type Pt = [number, number];
export type SketchShape =
  | { t: 'pen'; pts: Pt[]; c: string; w: number }
  | { t: 'arrow'; a: Pt; b: Pt; c: string; w: number }
  | { t: 'ellipse'; a: Pt; b: Pt; c: string; w: number }
  | { t: 'text'; at: Pt; s: string; c: string; w: number };
export type SketchTool = SketchShape['t'];

export type SketchData = Common &
  Handoff & {
    shapes: SketchShape[];
    redo: SketchShape[];
    tool: SketchTool;
    color: string;
    width: number;
    /** Flattened image + marks, and the marks alone (saved assets). */
    flattened?: Asset;
    overlay?: Asset;
    /** Hash of the shapes the flattened asset was made from. */
    savedKey?: string;
    notes?: string;
    /** Annotated images returned by Claude. */
    results: Asset[];
    error?: NodeError;
  };

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
  blender: BlenderData;
  photoshop: PhotoshopData;
  sketch: SketchData;
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
  | Node<OutputData, 'output'>
  | Node<BlenderData, 'blender'>
  | Node<PhotoshopData, 'photoshop'>
  | Node<SketchData, 'sketch'>;

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
    case 'blender':
      return { subject: '', set: '', camera: '', lens: '', light: '', views: ['three-quarter'], results: [] };
    case 'photoshop':
      return { instruction: '', maskMode: 'paint', brush: 24, results: [] };
    case 'sketch':
      return { shapes: [], redo: [], tool: 'pen', color: '#e5484d', width: 4, results: [] };
  }
}

export const RUNNABLE: readonly NodeKind[] = ['director', 'generate', 'edit', 'review', 'blender', 'photoshop'];
export function isRunnable(kind: string | undefined): boolean {
  return (RUNNABLE as readonly string[]).includes(kind ?? '');
}
