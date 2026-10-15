import type { LucideIcon } from 'lucide-react';
import { Aperture, Clapperboard, Columns2, FileText, Frame, Image, ScanEye, SquarePen } from 'lucide-react';
import type { NodeKind } from '../graph/types';

export const NODE_META: Record<NodeKind, { label: string; icon: LucideIcon; desc: string }> = {
  brief: { label: 'Brief', icon: FileText, desc: 'The client’s words, any language' },
  reference: { label: 'Reference', icon: Image, desc: 'A photo with a role: identity, product, light…' },
  director: { label: 'Director', icon: Clapperboard, desc: 'Turns the brief into a shot, a model plan and a prompt' },
  generate: { label: 'Generate', icon: Aperture, desc: 'Text or references to images, on any model' },
  edit: { label: 'Edit', icon: SquarePen, desc: 'Change only what you name; keep the rest exactly' },
  review: { label: 'Review', icon: ScanEye, desc: 'Scores, defects, a verdict and a fix' },
  compare: { label: 'Compare', icon: Columns2, desc: 'Lay branches side by side and pick a winner' },
  output: { label: 'Output', icon: Frame, desc: 'The final picture: view, download, copy prompt' },
};
