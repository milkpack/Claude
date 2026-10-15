import { useMemo } from 'react';
import type { Inputs } from './evaluate';
import { gatherInputs, isStale } from './evaluate';
import type { Runtime } from './store';
import { useGraph } from './store';

/**
 * A node's current inputs. The selector returns a string so React only re-renders the node when
 * what flows into it changes (not on every drag of an unrelated node).
 */
export function useNodeInputs(id: string): Inputs {
  const key = useGraph((s) => JSON.stringify(gatherInputs(id, s.nodes, s.edges)));
  return useMemo(() => JSON.parse(key) as Inputs, [key]);
}

export function useRuntime(id: string): Runtime | undefined {
  return useGraph((s) => s.runtime[id]);
}

export function useIsStale(id: string): boolean {
  return useGraph((s) => {
    const n = s.nodes.find((x) => x.id === id);
    return n ? isStale(n, s.nodes, s.edges) : false;
  });
}

export function useConnectedCount(id: string, port: string): number {
  return useGraph((s) => s.edges.filter((e) => e.target === id && e.targetHandle === port).length);
}
