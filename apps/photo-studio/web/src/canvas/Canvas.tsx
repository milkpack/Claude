import type { DragEvent, MouseEvent as ReactMouseEvent } from 'react';
import { useCallback, useEffect, useMemo, useRef } from 'react';
import { Background, BackgroundVariant, MiniMap, ReactFlow, useReactFlow, useViewport } from '@xyflow/react';
import type { Connection, EdgeTypes, FinalConnectionState, NodeTypes, OnConnectStartParams, XYPosition } from '@xyflow/react';
import { Grid3x3, Map as MapIcon, Maximize, Minus, Plus, Redo2, Undo2 } from 'lucide-react';
import type { AppEdge, AppNode } from '../graph/types';
import { copySelection, duplicateSelection, hasClipboard, pasteClipboard, useGraph, validConnection } from '../graph/store';
import { portOf } from '../graph/ports';
import { referenceFromAsset, referencesFromFiles, registerViewCenter, uploadIntoReference } from '../graph/actions';
import { runNode, runToHere } from '../graph/run';
import { TEMPLATES } from '../graph/templates';
import { loadTemplate } from '../graph/sync';
import { isNarrow, toast, useUi } from '../state/ui';
import { isEditableTarget, modKey } from '../lib/util';
import { Button, IconButton } from '../components/ui';
import { BriefNode } from './nodes/BriefNode';
import { ReferenceNode } from './nodes/ReferenceNode';
import { DirectorNode } from './nodes/DirectorNode';
import { GenerateNode } from './nodes/GenerateNode';
import { ReviewNode } from './nodes/ReviewNode';
import { CompareNode } from './nodes/CompareNode';
import { OutputNode } from './nodes/OutputNode';
import { BlenderNode } from './nodes/BlenderNode';
import { PhotoshopNode } from './nodes/PhotoshopNode';
import { SketchNode } from './nodes/SketchNode';
import { ConnectionLine, TypedEdge } from './edges';
import { ASSET_MIME, NODE_MIME, readDraggedAsset, readDraggedKind } from './dnd';

const nodeTypes: NodeTypes = {
  brief: BriefNode,
  reference: ReferenceNode,
  director: DirectorNode,
  generate: GenerateNode,
  edit: GenerateNode,
  review: ReviewNode,
  compare: CompareNode,
  output: OutputNode,
  blender: BlenderNode,
  photoshop: PhotoshopNode,
  sketch: SketchNode,
} as NodeTypes;

const edgeTypes: EdgeTypes = { typed: TypedEdge } as EdgeTypes;

const coarse = typeof window !== 'undefined' && window.matchMedia?.('(pointer: coarse)').matches;

function minimapColor(n: AppNode): string {
  return n.selected ? 'var(--selection)' : 'var(--minimap-node)';
}

export function Canvas() {
  const nodes = useGraph((s) => s.nodes);
  const edges = useGraph((s) => s.edges);
  const loadSeq = useGraph((s) => s.loadSeq);
  const snap = useUi((s) => s.snap);
  const minimap = useUi((s) => s.minimap);
  const theme = useUi((s) => s.theme);
  const focusNode = useUi((s) => s.focusNode);
  const rf = useReactFlow<AppNode, AppEdge>();
  const wrap = useRef<HTMLDivElement>(null);
  const pointer = useRef<XYPosition | null>(null);
  const connectStart = useRef<OnConnectStartParams | null>(null);

  const flowAt = useCallback((clientX: number, clientY: number) => rf.screenToFlowPosition({ x: clientX, y: clientY }), [rf]);

  useEffect(() => {
    registerViewCenter(() => {
      const r = wrap.current?.getBoundingClientRect();
      if (!r) return { x: 0, y: 0 };
      return rf.screenToFlowPosition({ x: r.left + r.width / 2, y: r.top + r.height / 2 });
    });
  }, [rf]);

  // A freshly loaded graph: restore its viewport or fit it.
  useEffect(() => {
    if (!loadSeq) return;
    const vp = useGraph.getState().viewport;
    const t = window.setTimeout(() => {
      if (vp) void rf.setViewport(vp);
      else void rf.fitView({ padding: 0.12, maxZoom: 1, duration: 0 });
    }, 30);
    return () => window.clearTimeout(t);
  }, [loadSeq, rf]);

  useEffect(() => {
    if (!focusNode) return;
    const t = window.setTimeout(() => {
      void rf.fitView({ nodes: [{ id: focusNode.id }], padding: 0.6, maxZoom: 1, duration: 280 });
    }, 40);
    return () => window.clearTimeout(t);
  }, [focusNode, rf]);

  const onConnect = useCallback((c: Connection) => {
    useGraph.getState().connect(c);
  }, []);

  const isValidConnection = useCallback((c: Connection | AppEdge) => {
    const s = useGraph.getState();
    return validConnection(c, s.nodes, s.edges);
  }, []);

  const onConnectEnd = useCallback(
    (event: MouseEvent | TouchEvent, state: FinalConnectionState) => {
      const start = connectStart.current;
      connectStart.current = null;
      if (state.isValid || state.toNode || !start?.nodeId || !start.handleId || !start.handleType) return;
      const pt = 'changedTouches' in event ? event.changedTouches[0] : event;
      if (!pt) return;
      const n = useGraph.getState().nodes.find((x) => x.id === start.nodeId);
      const port = portOf(n?.type, start.handleType === 'source' ? 'out' : 'in', start.handleId);
      if (!port) return;
      useUi.getState().setMenu({
        kind: 'connect',
        x: pt.clientX,
        y: pt.clientY,
        flow: flowAt(pt.clientX, pt.clientY),
        from: { node: start.nodeId, handle: start.handleId, side: start.handleType, type: port.type },
      });
    },
    [flowAt],
  );

  const onDragOver = (e: DragEvent) => {
    const t = e.dataTransfer.types;
    if (t.includes('Files') || t.includes(ASSET_MIME) || t.includes(NODE_MIME)) {
      e.preventDefault();
      e.dataTransfer.dropEffect = t.includes(NODE_MIME) ? 'move' : 'copy';
    }
  };

  const onDrop = (e: DragEvent) => {
    const at = flowAt(e.clientX, e.clientY);
    const kind = readDraggedKind(e.dataTransfer);
    if (kind) {
      e.preventDefault();
      useGraph.getState().addNode(kind, { x: at.x - 40, y: at.y - 20 }, undefined, { select: true });
      return;
    }
    const asset = readDraggedAsset(e.dataTransfer);
    if (asset) {
      e.preventDefault();
      referenceFromAsset(asset, { x: at.x - 120, y: at.y - 40 }, 'style');
      return;
    }
    const files = Array.from(e.dataTransfer.files);
    if (files.length) {
      e.preventDefault();
      referencesFromFiles(files, { x: at.x - 120, y: at.y - 40 });
    }
  };

  // Keyboard shortcuts (outside text fields).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const ui = useUi.getState();
      if (ui.appMode || ui.viewer || ui.settingsOpen) return;
      if (e.key === 'Escape') {
        ui.setMenu(null);
        return;
      }
      if (isEditableTarget(e.target)) return;
      const mod = e.metaKey || e.ctrlKey;
      const g = useGraph.getState();
      const key = e.key.toLowerCase();
      const selected = g.nodes.filter((n) => n.selected);
      if (mod && key === 'z') {
        e.preventDefault();
        if (e.shiftKey) g.redo();
        else g.undo();
      } else if (mod && key === 'y') {
        e.preventDefault();
        g.redo();
      } else if (mod && key === 'c') {
        if (copySelection()) e.preventDefault();
      } else if (mod && key === 'x') {
        if (copySelection()) {
          e.preventDefault();
          g.removeNodes(selected.map((n) => n.id));
        }
      } else if (mod && key === 'd') {
        e.preventDefault();
        duplicateSelection();
      } else if (mod && key === 'a') {
        e.preventDefault();
        g.selectAll();
      } else if (mod && key === 'enter') {
        const first = selected[0];
        if (first) {
          e.preventDefault();
          void (e.shiftKey ? runToHere(first.id) : runNode(first.id));
        }
      } else if (!mod && key === 'f') {
        const sel = selected.map((n) => ({ id: n.id }));
        void rf.fitView({ nodes: sel.length ? sel : undefined, padding: sel.length ? 0.08 : 0.12, maxZoom: 1, duration: 200 });
      }
    };
    const onPaste = (e: ClipboardEvent) => {
      const ui = useUi.getState();
      if (ui.appMode || ui.viewer || isEditableTarget(e.target)) return;
      const files = Array.from(e.clipboardData?.files ?? []).filter((f) => f.type.startsWith('image/'));
      const g = useGraph.getState();
      if (files.length) {
        e.preventDefault();
        const sel = g.nodes.filter((n) => n.selected);
        const only = sel.length === 1 ? sel[0] : undefined;
        if (only?.type === 'reference' && files[0]) void uploadIntoReference(only.id, files[0]);
        else {
          const r = wrap.current?.getBoundingClientRect();
          const at = pointer.current ?? (r ? flowAt(r.left + r.width / 2, r.top + r.height / 2) : { x: 0, y: 0 });
          referencesFromFiles(files, at);
        }
        return;
      }
      if (hasClipboard()) {
        e.preventDefault();
        pasteClipboard(pointer.current ?? undefined);
      }
    };
    window.addEventListener('keydown', onKey);
    window.addEventListener('paste', onPaste);
    return () => {
      window.removeEventListener('keydown', onKey);
      window.removeEventListener('paste', onPaste);
    };
  }, [rf, flowAt]);

  const onPaneContextMenu = (e: ReactMouseEvent | MouseEvent) => {
    e.preventDefault();
    useUi.getState().setMenu({ kind: 'pane', x: e.clientX, y: e.clientY, flow: flowAt(e.clientX, e.clientY) });
  };

  const empty = nodes.length === 0;
  const fitViewOptions = useMemo(() => ({ padding: 0.12, maxZoom: 1 }), []);

  return (
    <div className="canvas" ref={wrap} onDragOver={onDragOver} onDrop={onDrop}>
      <ReactFlow<AppNode, AppEdge>
        nodes={nodes}
        edges={edges}
        nodeTypes={nodeTypes}
        edgeTypes={edgeTypes}
        onNodesChange={useGraph.getState().onNodesChange}
        onEdgesChange={useGraph.getState().onEdgesChange}
        onConnect={onConnect}
        onConnectStart={(_, p) => {
          connectStart.current = p;
        }}
        onConnectEnd={onConnectEnd}
        isValidConnection={isValidConnection}
        connectionLineComponent={ConnectionLine}
        onNodeDragStart={() => useGraph.getState().checkpoint()}
        onSelectionDragStart={() => useGraph.getState().checkpoint()}
        onMoveEnd={(_, vp) => useGraph.getState().setViewport(vp)}
        onPaneContextMenu={onPaneContextMenu}
        onNodeContextMenu={(e, n) => {
          e.preventDefault();
          if (!n.selected) useGraph.getState().selectOnly([n.id]);
          useUi.getState().setMenu({ kind: 'node', id: n.id, x: e.clientX, y: e.clientY });
        }}
        onEdgeContextMenu={(e, edge) => {
          e.preventDefault();
          useUi.getState().setMenu({ kind: 'edge', id: edge.id, x: e.clientX, y: e.clientY });
        }}
        onPaneClick={() => useUi.getState().setMenu(null)}
        onPaneMouseMove={(e) => {
          pointer.current = flowAt(e.clientX, e.clientY);
        }}
        snapToGrid={snap}
        snapGrid={[16, 16]}
        minZoom={0.1}
        maxZoom={2.5}
        fitViewOptions={fitViewOptions}
        deleteKeyCode={['Delete', 'Backspace']}
        multiSelectionKeyCode={['Shift', 'Meta', 'Control']}
        selectionOnDrag={!coarse}
        panOnDrag={coarse ? true : [1]}
        panOnScroll={!coarse}
        zoomOnPinch
        zoomActivationKeyCode={['Meta', 'Control']}
        colorMode={theme}
        proOptions={{ hideAttribution: true }}
        defaultEdgeOptions={{ type: 'typed' }}
        elevateNodesOnSelect
        onlyRenderVisibleElements={false}
        aria-label="Workflow canvas"
      >
        <Background variant={BackgroundVariant.Dots} gap={16} size={1.2} color="var(--canvas-dot)" />
        {minimap && !isNarrow() ? (
          <MiniMap<AppNode>
            pannable
            zoomable
            position="bottom-right"
            nodeColor={minimapColor}
            nodeStrokeWidth={0}
            style={{ width: 168, height: 112 }}
            nodeBorderRadius={2}
            maskColor="var(--minimap-mask)"
            bgColor="var(--panel)"
            ariaLabel="Minimap"
          />
        ) : null}
      </ReactFlow>
      <CanvasControls />
      {empty ? <EmptyState /> : null}
    </div>
  );
}

function CanvasControls() {
  const rf = useReactFlow();
  const { zoom } = useViewport();
  const snap = useUi((s) => s.snap);
  const minimap = useUi((s) => s.minimap);
  const canUndo = useGraph((s) => s.past.length > 0);
  const canRedo = useGraph((s) => s.future.length > 0);
  return (
    <div className="canvas-controls" role="toolbar" aria-label="Canvas">
      <IconButton icon={Undo2} label={`Undo (${modKey}+Z)`} disabled={!canUndo} onClick={() => useGraph.getState().undo()} />
      <IconButton icon={Redo2} label={`Redo (${modKey}+Shift+Z)`} disabled={!canRedo} onClick={() => useGraph.getState().redo()} />
      <span className="sep" />
      <IconButton icon={Minus} label="Zoom out" onClick={() => void rf.zoomOut({ duration: 150 })} />
      <button type="button" className="zoom-label mono" title="Zoom to 100 %" onClick={() => void rf.zoomTo(1, { duration: 150 })}>
        {Math.round(zoom * 100)}%
      </button>
      <IconButton icon={Plus} label="Zoom in" onClick={() => void rf.zoomIn({ duration: 150 })} />
      <IconButton icon={Maximize} label="Fit view (F)" onClick={() => void rf.fitView({ padding: 0.12, maxZoom: 1, duration: 200 })} />
      <span className="sep" />
      <IconButton icon={Grid3x3} label="Snap to grid" active={snap} onClick={() => useUi.getState().setSnap(!snap)} />
      <IconButton icon={MapIcon} label="Minimap" active={minimap} className="hide-narrow" onClick={() => useUi.getState().setMinimap(!minimap)} />
    </div>
  );
}

function EmptyState() {
  const main = TEMPLATES[0];
  return (
    <div className="empty-state">
      <div className="empty-card">
        <span className="empty-kicker mono">Empty canvas</span>
        <h2>Start from your usual workflow, or build one.</h2>
        <p>Drop a photo anywhere to make a Reference node. Add nodes from the palette, or right-click the canvas.</p>
        <div className="empty-actions">
          {main ? (
            <Button variant="primary" onClick={() => void loadTemplate(main)}>
              {main.name}
            </Button>
          ) : null}
          <Button
            onClick={() => {
              useUi.getState().setLeft(true);
              toast('Templates are in the left panel');
            }}
          >
            Other templates
          </Button>
        </div>
        <p className="empty-keys mono">
          {modKey}+Z undo · {modKey}+C / V copy, paste · {modKey}+Enter run · F fit selection
        </p>
      </div>
    </div>
  );
}
