import { useEffect, useRef, useState } from 'react';
import { Activity, Cloud, LogOut, RefreshCw, Trash, Unplug, X } from 'lucide-react';
import { useQuery } from '@tanstack/react-query';
import { api } from '../api/client';
import { describeError } from '../api/errors';
import { qk, queryClient, useMe } from '../api/queries';
import { toast, useUi } from '../state/ui';
import { formatCredits, modKey } from '../lib/util';
import { Button, Chips, IconButton, Kbd, connectOpenArt } from '../components/ui';
import { flushSave } from '../graph/sync';

const SHORTCUTS: [string, string][] = [
  [`${modKey} Z`, 'Undo'],
  [`${modKey} ⇧ Z`, 'Redo'],
  [`${modKey} C / V / X`, 'Copy, paste, cut nodes'],
  [`${modKey} D`, 'Duplicate'],
  [`${modKey} A`, 'Select all'],
  ['Del', 'Delete selection'],
  [`${modKey} ↵`, 'Run the selected node'],
  [`${modKey} ⇧ ↵`, 'Run to here'],
  ['F', 'Fit the selection (or everything)'],
  ['Space drag', 'Pan'],
  [`${modKey} scroll`, 'Zoom'],
  ['Shift drag', 'Add to selection'],
  [`${modKey} V (image)`, 'Paste a photo as a Reference'],
];

function RoutineSection() {
  const q = useQuery({ queryKey: ['routine'], queryFn: api.routine, retry: false });
  const [url, setUrl] = useState('');
  const [token, setToken] = useState('');
  const [proxy, setProxy] = useState('');
  const [busy, setBusy] = useState(false);
  const [check, setCheck] = useState<{ busy: boolean; text?: string; ok?: boolean }>({ busy: false });
  const r = q.data;
  const refresh = async () => {
    await queryClient.invalidateQueries({ queryKey: ['routine'] });
    await queryClient.invalidateQueries({ queryKey: qk.me });
  };
  const saveUrl = url.trim() || r?.url || '';
  const canSave = Boolean(saveUrl) && (Boolean(token.trim()) || Boolean(r?.configured)) && Boolean(url.trim() || token.trim() || proxy.trim());
  const save = async (body: { url: string; token?: string; proxy?: string | null }, done: string) => {
    setBusy(true);
    try {
      await api.saveRoutine(body);
      setToken('');
      setUrl('');
      setProxy('');
      setCheck({ busy: false });
      await refresh();
      toast(done, 'ok');
    } catch (err) {
      toast(describeError(err).detail, 'error');
    } finally {
      setBusy(false);
    }
  };
  return (
    <section className="set-section" data-testid="routine-settings">
      <h3>Cloud runs (Claude routine)</h3>
      <div className="set-row">
        <span className={`dot ${r?.configured ? 'dot-ok' : 'dot-off'}`} />
        <div className="set-text">
          {r?.configured ? (
            <>
              <strong>Configured</strong>
              <span className="mono muted small">{r.url}</span>
              {r.source ? <span className="muted small">from {r.source}</span> : null}
              <span className="muted">Director, Review and Sketch tasks run in the cloud by themselves. Blender and Photoshop stay manual: they need your Mac.</span>
            </>
          ) : (
            <>
              <strong>Not configured</strong>
              <span className="muted">Paste the trigger URL and token of a Claude Code routine; tasks then run without copy and paste. The token is stored on the server and never shown again.</span>
            </>
          )}
        </div>
      </div>
      <form
        className="routine-form"
        onSubmit={(e) => {
          e.preventDefault();
          if (!canSave) return;
          void save(
            { url: saveUrl, ...(token.trim() ? { token: token.trim() } : {}), ...(proxy.trim() ? { proxy: proxy.trim() } : {}) },
            'Cloud runs saved',
          );
        }}
      >
        <label className="login-field">
          <span>Routine URL</span>
          <input className="mono" value={url} placeholder={r?.url ?? 'https://…'} onChange={(e) => setUrl(e.target.value)} autoComplete="off" />
        </label>
        <label className="login-field">
          <span>Token</span>
          <input type="password" value={token} placeholder={r?.configured ? 'stored · leave empty to keep it' : ''} onChange={(e) => setToken(e.target.value)} autoComplete="new-password" />
        </label>
        <label className="login-field">
          <span>Proxy (optional)</span>
          <input className="mono" value={proxy} placeholder={r?.proxy ?? 'socks5h://user:pass@host:1080'} onChange={(e) => setProxy(e.target.value)} autoComplete="off" spellCheck={false} />
          <span className="field-hint">
            Only for Anthropic; needed where api.anthropic.com is blocked.
            {r?.proxy ? (
              <>
                {' '}
                Saved: <span className="mono">{r.proxy}</span>
                {r.proxySource ? ` (from ${r.proxySource})` : ''}
              </>
            ) : null}
          </span>
        </label>
        <div className="set-actions">
          <Button type="submit" size="sm" variant="primary" icon={Cloud} busy={busy} disabled={!canSave}>
            {r?.configured ? 'Save changes' : 'Save'}
          </Button>
          <Button
            size="sm"
            icon={Activity}
            busy={check.busy}
            disabled={!r?.configured}
            onClick={async () => {
              setCheck({ busy: true });
              try {
                const c = await api.checkRoutine();
                setCheck({
                  busy: false,
                  ok: c.reachable,
                  text: c.reachable
                    ? c.via === 'proxy'
                      ? 'Reachable via proxy'
                      : 'Reachable (direct)'
                    : (c.error ?? `Not reachable${c.via === 'proxy' ? ' via proxy' : ''}${c.status ? ` (HTTP ${c.status})` : ''}`),
                });
              } catch (err) {
                setCheck({ busy: false, ok: false, text: describeError(err).detail });
              }
            }}
          >
            Check connection
          </Button>
          {r?.proxy ? (
            <Button size="sm" variant="ghost" disabled={busy} onClick={() => r.url && void save({ url: r.url, proxy: null }, 'Proxy removed')}>
              Remove proxy
            </Button>
          ) : null}
          {r?.configured ? (
            <Button
              size="sm"
              variant="ghost"
              icon={Trash}
              onClick={async () => {
                try {
                  await api.deleteRoutine();
                  await refresh();
                  toast('Cloud runs removed');
                } catch (err) {
                  toast(describeError(err).detail, 'error');
                }
              }}
            >
              Remove
            </Button>
          ) : null}
        </div>
        {check.text ? (
          <p className={`check-result ${check.ok ? 'is-ok' : 'is-bad'}`} role="status">
            <span className={`dot ${check.ok ? 'dot-ok' : 'dot-danger'}`} /> <span className={check.ok ? '' : 'mono'}>{check.text}</span>
          </p>
        ) : null}
      </form>
    </section>
  );
}

export function Settings() {
  const open = useUi((s) => s.settingsOpen);
  const theme = useUi((s) => s.theme);
  const me = useMe();
  const [busy, setBusy] = useState<string | null>(null);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    ref.current?.querySelector<HTMLElement>('button')?.focus();
    const key = (e: KeyboardEvent) => e.key === 'Escape' && useUi.getState().setSettings(false);
    window.addEventListener('keydown', key);
    return () => window.removeEventListener('keydown', key);
  }, [open]);
  if (!open) return null;
  const o = me.data?.openart;
  const c = me.data?.claude;
  const close = () => useUi.getState().setSettings(false);

  const act = async (name: string, fn: () => Promise<unknown>) => {
    setBusy(name);
    try {
      await fn();
    } catch (e) {
      toast(describeError(e).detail, 'error');
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="modal-scrim" onClick={(e) => e.target === e.currentTarget && close()}>
      <div className="modal" role="dialog" aria-modal="true" aria-label="Settings" ref={ref}>
        <div className="modal-head">
          <h2>Settings</h2>
          <IconButton icon={X} label="Close" onClick={close} />
        </div>
        <section className="set-section">
          <h3>OpenArt</h3>
          <div className="set-row">
            <span className={`dot ${o?.connected && !o.needsReconnect ? 'dot-ok' : 'dot-warn'}`} />
            <div className="set-text">
              {o?.connected ? (
                <>
                  <strong>{o.needsReconnect ? 'Needs reconnecting' : 'Connected'}</strong>
                  <span className="muted">
                    {o.email ?? 'account'}
                    {o.plan ? ` · ${o.plan}` : ''}
                  </span>
                  <span className="mono">{formatCredits(o.credits)} credits</span>
                </>
              ) : (
                <>
                  <strong>Not connected</strong>
                  <span className="muted">Generation, uploads and prices use your OpenArt account. Connect it once; tokens stay on the server.</span>
                </>
              )}
            </div>
          </div>
          <div className="set-actions">
            <Button variant={o?.connected && !o.needsReconnect ? 'default' : 'primary'} size="sm" busy={busy === 'connect'} onClick={() => act('connect', connectOpenArt)}>
              {o?.connected ? 'Reconnect' : 'Connect OpenArt'}
            </Button>
            {o?.connected ? (
              <Button
                size="sm"
                variant="ghost"
                icon={Unplug}
                busy={busy === 'disconnect'}
                onClick={() =>
                  act('disconnect', async () => {
                    if (!window.confirm('Disconnect OpenArt? Running jobs keep going, but nothing new can start.')) return;
                    const m = await api.disconnectOpenArt();
                    queryClient.setQueryData(qk.me, m);
                    await queryClient.invalidateQueries({ queryKey: qk.models });
                  })
                }
              >
                Disconnect
              </Button>
            ) : null}
            <Button
              size="sm"
              variant="ghost"
              icon={RefreshCw}
              busy={busy === 'models'}
              disabled={!o?.connected}
              onClick={() =>
                act('models', async () => {
                  const r = await api.models(true);
                  queryClient.setQueryData(qk.models, r);
                  await queryClient.invalidateQueries({ queryKey: qk.me });
                  toast(`Models and prices refreshed · ${r.models.length} models`);
                })
              }
            >
              Refresh models and prices
            </Button>
          </div>
        </section>
        <section className="set-section">
          <h3>Claude</h3>
          <div className="set-row">
            <span className={`dot ${c?.configured ? 'dot-ok' : 'dot-off'}`} />
            <div className="set-text">
              {c?.configured ? (
                <>
                  <strong>Configured</strong>
                  <span className="mono muted">{c.model}</span>
                  <span className="muted">Director and Review can run directly (set “Run via: API” on the node); handoff stays the default.</span>
                </>
              ) : (
                <>
                  <strong>Not configured</strong>
                  <span className="muted">
                    Set <code>ANTHROPIC_API_KEY</code> in the server’s environment and restart it. Until then Director, Review, Blender, Photoshop and Sketch work as handoffs: each builds a task you paste into Claude, and the answer comes back into the node.
                  </span>
                </>
              )}
            </div>
          </div>
        </section>
        <RoutineSection />
        <section className="set-section">
          <h3>Appearance</h3>
          <div className="set-row">
            <Chips options={['dark', 'light'] as const} value={theme} onChange={(t) => useUi.getState().setTheme(t)} label="Theme" />
            <span className="muted small">Neutral greys in both: the photographs supply the colour.</span>
          </div>
        </section>
        <section className="set-section">
          <h3>Keyboard</h3>
          <dl className="shortcuts">
            {SHORTCUTS.map(([k, d]) => (
              <span key={k} className="shortcut">
                <dt>
                  <Kbd>{k}</Kbd>
                </dt>
                <dd>{d}</dd>
              </span>
            ))}
          </dl>
        </section>
        <section className="set-section">
          <h3>Session</h3>
          <Button
            size="sm"
            icon={LogOut}
            busy={busy === 'logout'}
            onClick={() =>
              act('logout', async () => {
                await flushSave();
                await api.logout();
                useUi.getState().setSettings(false);
                queryClient.clear();
                await queryClient.invalidateQueries({ queryKey: qk.me });
              })
            }
          >
            Sign out
          </Button>
        </section>
      </div>
    </div>
  );
}
