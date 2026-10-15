import type { NodeKind, PortType } from './types';

export type Port = { id: string; type: PortType; label: string; multi?: boolean; optional?: boolean };
export type PortSet = { inputs: Port[]; outputs: Port[] };

/** Typed handles per node kind. Handle ids are stable: saved graphs refer to them. */
export const PORTS: Record<NodeKind, PortSet> = {
  brief: { inputs: [], outputs: [{ id: 'text', type: 'text', label: 'text' }] },
  reference: { inputs: [], outputs: [{ id: 'image', type: 'image', label: 'image' }] },
  director: {
    inputs: [
      { id: 'brief', type: 'text', label: 'brief', multi: true },
      { id: 'refs', type: 'image', label: 'references', multi: true, optional: true },
    ],
    outputs: [
      { id: 'spec', type: 'spec', label: 'spec' },
      { id: 'prompt', type: 'text', label: 'prompt' },
    ],
  },
  generate: {
    inputs: [
      { id: 'prompt', type: 'text', label: 'prompt', multi: true, optional: true },
      { id: 'refs', type: 'image', label: 'references', multi: true, optional: true },
      { id: 'spec', type: 'spec', label: 'spec', optional: true },
    ],
    outputs: [{ id: 'images', type: 'image', label: 'images' }],
  },
  edit: {
    inputs: [
      { id: 'source', type: 'image', label: 'source' },
      { id: 'refs', type: 'image', label: 'references', multi: true, optional: true },
      { id: 'prompt', type: 'text', label: 'instruction', multi: true, optional: true },
      { id: 'spec', type: 'spec', label: 'spec', optional: true },
    ],
    outputs: [{ id: 'images', type: 'image', label: 'images' }],
  },
  review: {
    inputs: [
      { id: 'image', type: 'image', label: 'images', multi: true },
      { id: 'source', type: 'image', label: 'source', optional: true },
      { id: 'brief', type: 'text', label: 'brief', multi: true, optional: true },
    ],
    outputs: [{ id: 'fix', type: 'text', label: 'fix' }],
  },
  compare: {
    inputs: [{ id: 'images', type: 'image', label: 'images', multi: true }],
    outputs: [{ id: 'image', type: 'image', label: 'winner' }],
  },
  output: { inputs: [{ id: 'image', type: 'image', label: 'image', multi: true }], outputs: [] },
};

export function portOf(kind: string | undefined, side: 'in' | 'out', handle: string | null | undefined): Port | undefined {
  if (!kind || !(kind in PORTS)) return undefined;
  const set = PORTS[kind as NodeKind];
  const list = side === 'in' ? set.inputs : set.outputs;
  return list.find((p) => p.id === handle) ?? (handle == null ? list[0] : undefined);
}
