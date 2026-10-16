import { useEffect, useRef, useState } from 'react';
import { ChevronDown, CopyPlus, Image as ImageIcon, Library, Menu, Moon, Pencil, Plus, Settings, Sun, Trash } from 'lucide-react';
import { useQuery } from '@tanstack/react-query';
import { api } from '../api/client';
import type { WorkflowSummary } from '../api/types';
import { useMe, useWorkflows } from '../api/queries';
import { useGraph } from '../graph/store';
import { deleteWorkflow, duplicateSession, newWorkflow, openWorkflow, renameSession } from '../graph/sync';
import { useUi } from '../state/ui';
import { formatCredits, formatDate } from '../lib/util';
import { Button, IconButton, connectOpenArt } from '../components/ui';

function SaveState() {
  const s = useUi((x) => x.saveState);
  const id = useGraph((x) => x.workflowId);
  const label = s === 'saving' ? 'Saving…' : s === 'error' ? 'Not saved' : s === 'saved' || id ? 'Saved' : 'Not saved yet';
  return (
    <span className={`save-state ${s === 'error' ? 'is-error' : ''}`} aria-live="polite">
      {label}
    </span>
  );
}

function Cover({ id }: { id?: string }) {
  const q = useQuery({ queryKey: ['asset', id ?? ''], queryFn: () => api.asset(id ?? ''), enabled: Boolean(id), staleTime: Infinity, retry: false });
  return <span className="session-cover">{q.data ? <img src={q.data.thumb} alt="" draggable={false} /> : <ImageIcon size={14} aria-hidden />}</span>;
}

function SessionRow({ w, current, close }: { w: WorkflowSummary; current: boolean; close: () => void }) {
  const [mode, setMode] = useState<'view' | 'rename' | 'confirm'>('view');
  const [name, setName] = useState(w.name);
  if (mode === 'confirm')
    return (
      <div className="session-row is-confirm" role="alertdialog" aria-label={`Delete ${w.name}?`}>
        <span className="session-confirm">
          Delete <strong>{w.name}</strong>? Its canvas goes; its images stay in the library.
        </span>
        <div className="session-confirm-actions">
          <Button size="sm" variant="danger" autoFocus onClick={() => void deleteWorkflow(w.id)}>
            Delete
          </Button>
          <Button size="sm" variant="ghost" onClick={() => setMode('view')}>
            Keep
          </Button>
        </div>
      </div>
    );
  return (
    <div className={`session-row ${current ? 'is-on' : ''}`}>
      <Cover id={w.cover} />
      {mode === 'rename' ? (
        <input
          className="input session-rename"
          value={name}
          autoFocus
          aria-label="Session name"
          onChange={(e) => setName(e.target.value)}
          onBlur={() => {
            void renameSession(w.id, name);
            setMode('view');
          }}
          onKeyDown={(e) => {
            if (e.key === 'Enter') e.currentTarget.blur();
            if (e.key === 'Escape') {
              setName(w.name);
              setMode('view');
            }
          }}
        />
      ) : (
        <button
          type="button"
          role="menuitem"
          className="session-open"
          onClick={() => {
            close();
            if (!current) void openWorkflow(w.id);
          }}
        >
          <span className="session-name">{w.name}</span>
          <span className="session-date mono">
            {current ? 'open · ' : ''}
            {formatDate(w.lastOpenedAt ?? w.updatedAt)}
          </span>
        </button>
      )}
      <span className="session-actions">
        <IconButton icon={Pencil} label={`Rename ${w.name}`} size={13} onClick={() => setMode('rename')} />
        <IconButton
          icon={CopyPlus}
          label={`Duplicate ${w.name}`}
          size={13}
          onClick={() => {
            close();
            void duplicateSession(w.id);
          }}
        />
        <IconButton icon={Trash} label={`Delete ${w.name}`} size={13} onClick={() => setMode('confirm')} />
      </span>
    </div>
  );
}

function SessionSwitcher() {
  const name = useGraph((s) => s.name);
  const current = useGraph((s) => s.workflowId);
  const [open, setOpen] = useState(false);
  const list = useWorkflows(open);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const close = (e: PointerEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    const key = (e: KeyboardEvent) => e.key === 'Escape' && setOpen(false);
    window.addEventListener('pointerdown', close, true);
    window.addEventListener('keydown', key);
    return () => {
      window.removeEventListener('pointerdown', close, true);
      window.removeEventListener('keydown', key);
    };
  }, [open]);
  const sessions = [...(list.data?.workflows ?? [])].sort((a, b) => (b.lastOpenedAt ?? b.updatedAt) - (a.lastOpenedAt ?? a.updatedAt));
  return (
    <div className="wf-switch" ref={ref}>
      <input
        className="wf-name"
        value={name}
        aria-label="Session name"
        spellCheck={false}
        onChange={(e) => useGraph.getState().setName(e.target.value)}
        onBlur={(e) => {
          if (!e.target.value.trim()) useGraph.getState().setName('Untitled session');
        }}
        onKeyDown={(e) => e.key === 'Enter' && e.currentTarget.blur()}
      />
      <IconButton icon={ChevronDown} label="Sessions" size={14} aria-expanded={open} onClick={() => setOpen((v) => !v)} />
      {open ? (
        <div className="wf-pop session-pop" role="menu" aria-label="Sessions">
          <div className="session-pop-head">
            <span className="menu-label-row">Sessions</span>
            <Button
              size="sm"
              icon={Plus}
              onClick={() => {
                setOpen(false);
                void newWorkflow();
              }}
            >
              New session
            </Button>
          </div>
          {list.isLoading ? <div className="menu-empty">Loading…</div> : null}
          {list.data && sessions.length === 0 ? <div className="menu-empty">No saved sessions yet. Anything you build is saved as one.</div> : null}
          <div className="wf-list">
            {sessions.map((w) => (
              <SessionRow key={w.id} w={w} current={w.id === current} close={() => setOpen(false)} />
            ))}
          </div>
        </div>
      ) : null}
    </div>
  );
}

function OpenArtStatus() {
  const me = useMe();
  const o = me.data?.openart;
  if (!o) return null;
  if (!o.connected || o.needsReconnect)
    return (
      <button type="button" className="status-pill is-warn" onClick={() => void connectOpenArt()} title={o.needsReconnect ? 'The OpenArt session expired' : 'Connect your OpenArt account'}>
        <span className="dot dot-warn" />
        <span className="hide-narrow">OpenArt ·</span> {o.needsReconnect ? 'Reconnect' : 'Connect'}
      </button>
    );
  return (
    <button type="button" className="status-pill" onClick={() => useUi.getState().setSettings(true)} title={`OpenArt${o.email ? ` · ${o.email}` : ''}${o.plan ? ` · ${o.plan}` : ''}`}>
      <span className="dot dot-ok" />
      <span className="hide-narrow">OpenArt</span>
      <span className="mono credits">{formatCredits(o.credits)}</span>
      <span className="muted hide-narrow">cr</span>
    </button>
  );
}

function ClaudeStatus() {
  const me = useMe();
  const c = me.data?.claude;
  if (!c) return null;
  return (
    <button
      type="button"
      className={`status-pill ${c.configured ? '' : 'is-muted'} hide-narrow`}
      onClick={() => useUi.getState().setSettings(true)}
      title={c.configured ? `Claude · ${c.model}` : 'Claude is not configured on the server (ANTHROPIC_API_KEY)'}
    >
      <span className={`dot ${c.configured ? 'dot-ok' : 'dot-off'}`} />
      Claude{c.configured ? '' : ' off'}
    </button>
  );
}

export function TopBar() {
  const appMode = useUi((s) => s.appMode);
  const theme = useUi((s) => s.theme);
  const leftOpen = useUi((s) => s.leftOpen);
  const rightOpen = useUi((s) => s.rightOpen);
  const active = useUi((s) => Object.values(s.sessionJobs).filter((j) => j.job.status === 'queued' || j.job.status === 'running').length);
  return (
    <header className="topbar">
      <div className="topbar-left">
        {!appMode ? <IconButton icon={Menu} label="Nodes and templates" active={leftOpen} onClick={() => useUi.getState().setLeft(!leftOpen)} /> : null}
        <span className="brand" aria-label="Photo Studio">
          <span className="brand-mark" aria-hidden />
          <span className="brand-name hide-narrow">Photo Studio</span>
        </span>
        <span className="topbar-sep hide-narrow" aria-hidden />
        <SessionSwitcher />
        <span className="hide-narrow">
          <SaveState />
        </span>
      </div>
      <div className="mode-toggle" role="tablist" aria-label="Mode">
        <button type="button" role="tab" aria-selected={!appMode} className={!appMode ? 'is-on' : ''} onClick={() => useUi.getState().setAppMode(false)}>
          Canvas
        </button>
        <button type="button" role="tab" aria-selected={appMode} className={appMode ? 'is-on' : ''} onClick={() => useUi.getState().setAppMode(true)}>
          App
        </button>
      </div>
      <div className="topbar-right">
        <OpenArtStatus />
        <ClaudeStatus />
        {!appMode ? (
          <span className="rel">
            <IconButton icon={Library} label="Library and jobs" active={rightOpen} onClick={() => useUi.getState().setRight(!rightOpen)} />
            {active ? <span className="count-badge mono">{active}</span> : null}
          </span>
        ) : null}
        <IconButton icon={theme === 'dark' ? Sun : Moon} label={theme === 'dark' ? 'Light theme' : 'Dark theme'} onClick={() => useUi.getState().toggleTheme()} />
        <IconButton icon={Settings} label="Settings" onClick={() => useUi.getState().setSettings(true)} />
      </div>
    </header>
  );
}
