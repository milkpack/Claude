import { useEffect } from 'react';
import { ReactFlowProvider } from '@xyflow/react';
import { onUnauthenticated } from './api/client';
import { describeError } from './api/errors';
import { qk, queryClient, useMe } from './api/queries';
import type { Me } from './api/types';
import { Canvas } from './canvas/Canvas';
import { ContextMenu } from './canvas/ContextMenu';
import { bootWorkflow, useAutosave } from './graph/sync';
import { AppMode } from './panels/AppMode';
import { LeftPanel } from './panels/LeftPanel';
import { Login } from './panels/Login';
import { RightPanel } from './panels/RightPanel';
import { Settings } from './panels/Settings';
import { Toasts } from './panels/Toasts';
import { TopBar } from './panels/TopBar';
import { Viewer } from './panels/Viewer';
import { toast, useUi } from './state/ui';
import { Button, Spinner } from './components/ui';

function Studio() {
  const appMode = useUi((s) => s.appMode);
  const leftOpen = useUi((s) => s.leftOpen);
  const rightOpen = useUi((s) => s.rightOpen);
  useAutosave(true);

  useEffect(() => {
    void bootWorkflow();
    // Back from OpenArt's consent page: /?openart=connected|error
    const p = new URLSearchParams(window.location.search);
    const v = p.get('openart');
    if (v) {
      if (v === 'connected') toast('OpenArt connected', 'ok');
      else toast('OpenArt was not connected. Try again from Settings.', 'error');
      p.delete('openart');
      const q = p.toString();
      window.history.replaceState(null, '', `${window.location.pathname}${q ? `?${q}` : ''}${window.location.hash}`);
      void queryClient.invalidateQueries({ queryKey: qk.me });
    }
  }, []);

  return (
    <ReactFlowProvider>
      <div className={`app ${appMode ? 'is-app' : 'is-canvas'}`}>
        <TopBar />
        {appMode ? (
          <AppMode />
        ) : (
          <div className="workspace">
            <LeftPanel />
            <main className="main" aria-label="Canvas">
              <Canvas />
            </main>
            <RightPanel />
            {leftOpen || rightOpen ? (
              <div
                className="side-scrim"
                aria-hidden
                onClick={() => {
                  useUi.getState().setLeft(false);
                  useUi.getState().setRight(false);
                }}
              />
            ) : null}
          </div>
        )}
        <Viewer />
        <Settings />
        <ContextMenu />
        <Toasts />
      </div>
    </ReactFlowProvider>
  );
}

export function App() {
  const me = useMe();
  useEffect(
    () =>
      onUnauthenticated(() => {
        queryClient.setQueryData<Me>(qk.me, (m) => (m ? { ...m, authenticated: false } : m));
      }),
    [],
  );
  if (me.isLoading)
    return (
      <div className="splash">
        <Spinner size={18} />
      </div>
    );
  if (me.error)
    return (
      <div className="splash">
        <div className="login-card">
          <strong>Photo Studio is not reachable</strong>
          <p className="muted">{describeError(me.error).detail}</p>
          <Button variant="primary" onClick={() => void me.refetch()}>
            Try again
          </Button>
        </div>
        <Toasts />
      </div>
    );
  if (!me.data?.authenticated) return <Login />;
  return <Studio />;
}
