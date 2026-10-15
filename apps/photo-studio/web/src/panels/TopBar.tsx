import { useEffect, useRef, useState } from 'react';
import { ChevronDown, Library, Menu, Moon, Plus, Settings, Sun, Trash } from 'lucide-react';
import { useMe, useWorkflows } from '../api/queries';
import { useGraph } from '../graph/store';
import { deleteWorkflow, newWorkflow, openWorkflow } from '../graph/sync';
import { useUi } from '../state/ui';
import { formatCredits, formatDate } from '../lib/util';
import { IconButton, connectOpenArt } from '../components/ui';

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

function WorkflowSwitcher() {
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
  return (
    <div className="wf-switch" ref={ref}>
      <input
        className="wf-name"
        value={name}
        aria-label="Workflow name"
        spellCheck={false}
        onChange={(e) => useGraph.getState().setName(e.target.value)}
        onBlur={(e) => {
          if (!e.target.value.trim()) useGraph.getState().setName('Untitled workflow');
        }}
        onKeyDown={(e) => e.key === 'Enter' && e.currentTarget.blur()}
      />
      <IconButton icon={ChevronDown} label="Switch workflow" size={14} aria-expanded={open} onClick={() => setOpen((v) => !v)} />
      {open ? (
        <div className="wf-pop" role="menu">
          <button
            type="button"
            className="menu-item"
            role="menuitem"
            onClick={() => {
              setOpen(false);
              void newWorkflow();
            }}
          >
            <Plus size={14} aria-hidden /> <span className="menu-label">New workflow</span>
          </button>
          <div className="menu-sep" />
          <div className="menu-label-row">Workflows</div>
          {list.isLoading ? <div className="menu-empty">Loading…</div> : null}
          {list.data?.workflows.length === 0 ? <div className="menu-empty">None saved yet.</div> : null}
          <div className="wf-list">
            {list.data?.workflows.map((w) => (
              <div key={w.id} className={`wf-row ${w.id === current ? 'is-on' : ''}`}>
                <button
                  type="button"
                  role="menuitem"
                  className="menu-item"
                  onClick={() => {
                    setOpen(false);
                    if (w.id !== current) void openWorkflow(w.id);
                  }}
                >
                  <span className="menu-label">{w.name}</span>
                  <span className="menu-keys mono">{formatDate(w.updatedAt)}</span>
                </button>
                <IconButton
                  icon={Trash}
                  label={`Delete ${w.name}`}
                  size={13}
                  onClick={() => {
                    if (window.confirm(`Delete “${w.name}”? This cannot be undone.`)) void deleteWorkflow(w.id);
                  }}
                />
              </div>
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
        <WorkflowSwitcher />
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
