import type { ConnectionLineComponentProps, EdgeProps } from '@xyflow/react';
import { BaseEdge, getBezierPath } from '@xyflow/react';
import type { AppEdge, AppNode } from '../graph/types';
import { useGraph } from '../graph/store';
import { portOf } from '../graph/ports';

/** Wires coloured by data type; dashes flow while either end is running. */
export function TypedEdge({ id, source, target, sourceX, sourceY, targetX, targetY, sourcePosition, targetPosition, data, selected }: EdgeProps<AppEdge>) {
  const running = useGraph((s) => Boolean(s.runtime[source] || s.runtime[target]));
  const [path] = getBezierPath({ sourceX, sourceY, targetX, targetY, sourcePosition, targetPosition, curvature: 0.35 });
  const type = data?.type ?? 'image';
  return (
    <>
      <BaseEdge id={id} path={path} className={`wire type-${type} ${selected ? 'is-selected' : ''} ${running ? 'is-running' : ''}`} interactionWidth={14} />
      {running ? <path d={path} className={`wire-flow type-${type}`} fill="none" /> : null}
    </>
  );
}

export function ConnectionLine({ fromX, fromY, toX, toY, fromPosition, toPosition, fromHandle, fromNode }: ConnectionLineComponentProps<AppNode>) {
  const side = fromHandle?.type === 'target' ? 'in' : 'out';
  const type = portOf(fromNode?.type, side, fromHandle?.id)?.type ?? 'image';
  const [path] = getBezierPath({ sourceX: fromX, sourceY: fromY, sourcePosition: fromPosition, targetX: toX, targetY: toY, targetPosition: toPosition, curvature: 0.35 });
  return (
    <g>
      <path d={path} fill="none" className={`wire wire-drag type-${type}`} />
      <circle cx={toX} cy={toY} r={3.5} className={`wire-end type-${type}`} />
    </g>
  );
}
