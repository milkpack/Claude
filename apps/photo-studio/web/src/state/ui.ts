import { create } from 'zustand';
import type { XYPosition } from '@xyflow/react';
import type { Asset, Job } from '../api/types';
import type { PortType } from '../graph/types';
import { prefs, uid } from '../lib/util';

export type Theme = 'dark' | 'light';

export type ViewerState = {
  assets: Asset[];
  index: number;
  /** The node the images came from (placement of "use as reference" / "remix"). */
  fromNode?: string;
  fromLibrary?: boolean;
};

export type MenuState =
  | { kind: 'pane'; x: number; y: number; flow: XYPosition }
  | { kind: 'node'; x: number; y: number; id: string }
  | { kind: 'edge'; x: number; y: number; id: string }
  | {
      kind: 'connect';
      x: number;
      y: number;
      flow: XYPosition;
      from: { node: string; handle: string; side: 'source' | 'target'; type: PortType };
    };

export type Toast = { id: string; text: string; tone: 'info' | 'ok' | 'error' };

export type SessionJob = { job: Job; nodeId: string; nodeTitle: string };

type UiState = {
  theme: Theme;
  appMode: boolean;
  leftOpen: boolean;
  rightOpen: boolean;
  rightTab: 'library' | 'jobs';
  viewer: ViewerState | null;
  settingsOpen: boolean;
  menu: MenuState | null;
  toasts: Toast[];
  sessionJobs: Record<string, SessionJob>;
  snap: boolean;
  minimap: boolean;
  saveState: 'idle' | 'saving' | 'saved' | 'error';
  focusNode: { id: string; seq: number } | null;

  setTheme: (t: Theme) => void;
  toggleTheme: () => void;
  setAppMode: (v: boolean) => void;
  setLeft: (v: boolean) => void;
  setRight: (v: boolean, tab?: 'library' | 'jobs') => void;
  openViewer: (v: ViewerState) => void;
  closeViewer: () => void;
  setViewerIndex: (i: number) => void;
  setSettings: (v: boolean) => void;
  setMenu: (m: MenuState | null) => void;
  toast: (text: string, tone?: Toast['tone']) => void;
  dismissToast: (id: string) => void;
  trackJob: (job: Job, nodeId: string, nodeTitle: string) => void;
  setSnap: (v: boolean) => void;
  setMinimap: (v: boolean) => void;
  setSaveState: (s: UiState['saveState']) => void;
  focus: (id: string) => void;
};

const narrow = typeof window !== 'undefined' && window.matchMedia?.('(max-width: 860px)').matches;
const wide = typeof window !== 'undefined' && window.matchMedia?.('(min-width: 1180px)').matches;

function initialTheme(): Theme {
  const t = prefs.get('theme');
  return t === 'light' ? 'light' : 'dark';
}

export const useUi = create<UiState>((set, get) => ({
  theme: initialTheme(),
  appMode: false,
  leftOpen: !narrow && prefs.get('left') !== '0',
  rightOpen: Boolean(wide) && prefs.get('right') !== '0',
  rightTab: 'library',
  viewer: null,
  settingsOpen: false,
  menu: null,
  toasts: [],
  sessionJobs: {},
  snap: prefs.get('snap') !== '0',
  minimap: prefs.get('minimap') !== '0',
  saveState: 'idle',
  focusNode: null,

  setTheme: (t) => {
    prefs.set('theme', t);
    document.documentElement.setAttribute('data-theme', t);
    set({ theme: t });
  },
  toggleTheme: () => get().setTheme(get().theme === 'dark' ? 'light' : 'dark'),
  setAppMode: (v) => set({ appMode: v, menu: null }),
  setLeft: (v) => {
    if (!narrow) prefs.set('left', v ? '1' : '0');
    set(narrow && v ? { leftOpen: true, rightOpen: false } : { leftOpen: v });
  },
  setRight: (v, tab) => {
    if (!narrow) prefs.set('right', v ? '1' : '0');
    set({ rightOpen: v, ...(tab ? { rightTab: tab } : {}), ...(narrow && v ? { leftOpen: false } : {}) });
  },
  openViewer: (v) => set({ viewer: v, menu: null }),
  closeViewer: () => set({ viewer: null }),
  setViewerIndex: (i) => {
    const v = get().viewer;
    if (!v || !v.assets.length) return;
    const n = v.assets.length;
    set({ viewer: { ...v, index: ((i % n) + n) % n } });
  },
  setSettings: (v) => set({ settingsOpen: v, menu: null }),
  setMenu: (m) => set({ menu: m }),
  toast: (text, tone = 'info') => {
    const id = uid('t');
    set((s) => ({ toasts: [...s.toasts.slice(-3), { id, text, tone }] }));
    window.setTimeout(() => get().dismissToast(id), tone === 'error' ? 7000 : 3800);
  },
  dismissToast: (id) => set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) })),
  trackJob: (job, nodeId, nodeTitle) => set((s) => ({ sessionJobs: { ...s.sessionJobs, [job.id]: { job, nodeId, nodeTitle } } })),
  setSnap: (v) => {
    prefs.set('snap', v ? '1' : '0');
    set({ snap: v });
  },
  setMinimap: (v) => {
    prefs.set('minimap', v ? '1' : '0');
    set({ minimap: v });
  },
  setSaveState: (s) => set({ saveState: s }),
  focus: (id) => set((s) => ({ focusNode: { id, seq: (s.focusNode?.seq ?? 0) + 1 }, menu: null })),
}));

export const toast = (text: string, tone?: Toast['tone']) => useUi.getState().toast(text, tone);

export function isNarrow(): boolean {
  return window.matchMedia?.('(max-width: 860px)').matches ?? false;
}
