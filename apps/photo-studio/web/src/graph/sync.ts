// Workflow persistence: load on boot, autosave (debounced) to /api/workflows, switch, create.
import { useEffect } from 'react';
import { api } from '../api/client';
import { describeError } from '../api/errors';
import { qk, queryClient } from '../api/queries';
import { prefs } from '../lib/util';
import { toast, useUi } from '../state/ui';
import { useGraph } from './store';
import { fromGraph, toGraph } from './serialize';
import { resumePending } from './run';
import type { Template } from './templates';

const SAVE_DELAY = 900;
let timer: number | undefined;
let saving: Promise<void> | null = null;
let savedVersion = -1;
let creating: Promise<string> | null = null;

async function saveNow(): Promise<void> {
  window.clearTimeout(timer);
  timer = undefined;
  const s = useGraph.getState();
  if (s.version === savedVersion) return;
  const version = s.version;
  // An empty, never-saved canvas is not worth a workflow entry.
  if (!s.workflowId && s.nodes.length === 0) return;
  const ui = useUi.getState();
  ui.setSaveState('saving');
  try {
    const graph = toGraph(s.nodes, s.edges, s.viewport);
    if (!s.workflowId) {
      creating ??= api.createWorkflow(s.name, graph).then((w) => w.id);
      const id = await creating;
      creating = null;
      useGraph.getState().setWorkflowId(id);
      prefs.set('workflow', id);
    } else {
      await api.saveWorkflow(s.workflowId, s.name, graph);
    }
    savedVersion = version;
    useUi.getState().setSaveState('saved');
    void queryClient.invalidateQueries({ queryKey: qk.workflows });
    // Changes made while saving get their own save.
    if (useGraph.getState().version !== version) schedule();
  } catch (err) {
    creating = null;
    useUi.getState().setSaveState('error');
    const v = describeError(err);
    if (v.code !== 'unauthenticated') toast(`Not saved: ${v.detail}`, 'error');
  }
}

function schedule(): void {
  window.clearTimeout(timer);
  timer = window.setTimeout(() => {
    saving = saveNow().finally(() => {
      saving = null;
    });
  }, SAVE_DELAY);
}

export async function flushSave(): Promise<void> {
  if (saving) await saving;
  if (timer !== undefined || useGraph.getState().version !== savedVersion) await saveNow();
}

export function useAutosave(enabled: boolean): void {
  useEffect(() => {
    if (!enabled) return;
    const unsub = useGraph.subscribe((s, prev) => {
      if (s.version !== prev.version && s.version !== savedVersion) schedule();
    });
    const beforeUnload = (e: BeforeUnloadEvent) => {
      if (timer !== undefined || saving) {
        void saveNow();
        e.preventDefault();
      }
    };
    window.addEventListener('beforeunload', beforeUnload);
    return () => {
      unsub();
      window.removeEventListener('beforeunload', beforeUnload);
    };
  }, [enabled]);
}

export async function openWorkflow(id: string): Promise<boolean> {
  await flushSave();
  try {
    const w = await api.workflow(id);
    const g = fromGraph(w.graph);
    useGraph.getState().load({ id: w.id, name: w.name || 'Untitled workflow', ...g });
    savedVersion = useGraph.getState().version;
    prefs.set('workflow', w.id);
    useUi.getState().setSaveState('saved');
    resumePending();
    return true;
  } catch (err) {
    toast(`Could not open the workflow: ${describeError(err).detail}`, 'error');
    return false;
  }
}

export async function newWorkflow(name = 'Untitled workflow'): Promise<void> {
  await flushSave();
  useGraph.getState().load({ id: null, name, nodes: [], edges: [], viewport: null });
  savedVersion = useGraph.getState().version;
  prefs.set('workflow', null);
  useUi.getState().setSaveState('idle');
}

/** Loads a template: into the current canvas when it is empty, else as a new workflow. */
export async function loadTemplate(t: Template): Promise<void> {
  const { nodes, edges } = t.build();
  const s = useGraph.getState();
  if (s.nodes.length > 0) {
    await flushSave();
    useGraph.getState().load({ id: null, name: t.name, nodes, edges, viewport: null });
    prefs.set('workflow', null);
  } else {
    useGraph.getState().load({ id: s.workflowId, name: s.workflowId && s.name !== 'Untitled workflow' ? s.name : t.name, nodes, edges, viewport: null });
  }
  // Loading is a change worth saving.
  useGraph.setState((x) => ({ version: x.version + 1 }));
}

export async function deleteWorkflow(id: string): Promise<void> {
  try {
    await api.deleteWorkflow(id);
    void queryClient.invalidateQueries({ queryKey: qk.workflows });
    if (useGraph.getState().workflowId === id) {
      savedVersion = useGraph.getState().version;
      window.clearTimeout(timer);
      timer = undefined;
      await newWorkflow();
    }
    toast('Workflow deleted');
  } catch (err) {
    toast(`Could not delete: ${describeError(err).detail}`, 'error');
  }
}

export async function bootWorkflow(): Promise<void> {
  try {
    const { workflows } = await queryClient.fetchQuery({ queryKey: qk.workflows, queryFn: api.workflows });
    const last = prefs.get('workflow');
    const pick = workflows.find((w) => w.id === last) ?? [...workflows].sort((a, b) => b.updatedAt - a.updatedAt)[0];
    if (pick) await openWorkflow(pick.id);
  } catch (err) {
    const v = describeError(err);
    if (v.code !== 'unauthenticated') toast(`Could not load workflows: ${v.detail}`, 'error');
  }
}
