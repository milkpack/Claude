import { useEffect, useRef, useState } from 'react';
import { ChevronLeft, ChevronRight, Copy, Download, ImagePlus, Shuffle, Trash, X } from 'lucide-react';
import { api } from '../api/client';
import { describeError } from '../api/errors';
import { findModel, qk, queryClient, useModels } from '../api/queries';
import { addAsReference, remix } from '../graph/actions';
import { toast, useUi } from '../state/ui';
import { assetFilename, copyText, downloadUrl, formatDate } from '../lib/util';
import { Button, IconButton, Spinner } from '../components/ui';

const HIDDEN_PARAMS = new Set(['prompt', 'visualReferences']);

export function Viewer() {
  const v = useUi((s) => s.viewer);
  const models = useModels();
  const [loaded, setLoaded] = useState<string | null>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  const asset = v?.assets[v.index];

  useEffect(() => {
    if (!v) return;
    const prev = document.activeElement as HTMLElement | null;
    closeRef.current?.focus();
    const key = (e: KeyboardEvent) => {
      const ui = useUi.getState();
      if (e.key === 'Escape') ui.closeViewer();
      else if (e.key === 'ArrowRight') ui.setViewerIndex((ui.viewer?.index ?? 0) + 1);
      else if (e.key === 'ArrowLeft') ui.setViewerIndex((ui.viewer?.index ?? 0) - 1);
    };
    window.addEventListener('keydown', key);
    return () => {
      window.removeEventListener('keydown', key);
      prev?.focus?.();
    };
  }, [v]);

  if (!v || !asset) return null;
  const params = Object.entries(asset.params ?? {}).filter(([k]) => !HIDDEN_PARAMS.has(k));
  const refs = Array.isArray(asset.params?.visualReferences) ? (asset.params.visualReferences as { label?: string }[]) : [];
  const n = v.assets.length;

  return (
    <div className="viewer" role="dialog" aria-modal="true" aria-label="Image viewer">
      <div className="viewer-stage" onClick={(e) => e.target === e.currentTarget && useUi.getState().closeViewer()}>
        {loaded !== asset.id ? (
          <>
            <img className="viewer-img is-thumb" src={asset.thumb} alt="" aria-hidden />
            <span className="viewer-loading">
              <Spinner />
            </span>
          </>
        ) : null}
        <img
          key={asset.id}
          className={`viewer-img ${loaded === asset.id ? '' : 'is-loading'}`}
          src={asset.file}
          alt={asset.prompt ?? 'Image'}
          onLoad={() => setLoaded(asset.id)}
          draggable={false}
        />
        {n > 1 ? (
          <>
            <IconButton icon={ChevronLeft} label="Previous (←)" className="viewer-nav prev" size={20} onClick={() => useUi.getState().setViewerIndex(v.index - 1)} />
            <IconButton icon={ChevronRight} label="Next (→)" className="viewer-nav next" size={20} onClick={() => useUi.getState().setViewerIndex(v.index + 1)} />
            <span className="viewer-count mono">
              {v.index + 1} / {n}
            </span>
          </>
        ) : null}
      </div>
      <aside className="viewer-info">
        <div className="viewer-head">
          <span className="viewer-kind">{asset.kind === 'upload' ? 'Upload' : 'Generation'}</span>
          <IconButton ref={closeRef} icon={X} label="Close (Esc)" onClick={() => useUi.getState().closeViewer()} />
        </div>
        <div className="viewer-actions">
          <Button size="sm" icon={ImagePlus} onClick={() => addAsReference(asset, v.fromNode)}>
            Use as reference
          </Button>
          <Button size="sm" icon={Shuffle} disabled={!asset.model} onClick={() => remix(asset, v.fromNode)} title={asset.model ? 'New node with these settings' : 'Uploads have no settings to remix'}>
            Remix
          </Button>
          <Button size="sm" icon={Download} onClick={() => downloadUrl(asset.file, assetFilename(asset))}>
            Download
          </Button>
          <Button size="sm" icon={Copy} disabled={!asset.prompt} onClick={async () => toast((await copyText(asset.prompt ?? '')) ? 'Prompt copied' : 'Could not copy the prompt', 'info')}>
            Copy prompt
          </Button>
        </div>
        {asset.prompt ? (
          <section>
            <h3>Prompt</h3>
            <p className="viewer-prompt">{asset.prompt}</p>
          </section>
        ) : null}
        <section>
          <h3>Details</h3>
          <dl className="kv">
            {asset.model ? (
              <>
                <dt>Model</dt>
                <dd>{findModel(models.data?.models, asset.model)?.name ?? asset.model}</dd>
              </>
            ) : null}
            {asset.mode ? (
              <>
                <dt>Mode</dt>
                <dd className="mono">{asset.mode}</dd>
              </>
            ) : null}
            <dt>Size</dt>
            <dd className="mono">
              {asset.width} × {asset.height}
            </dd>
            <dt>Created</dt>
            <dd className="mono">{formatDate(asset.createdAt)}</dd>
            {refs.length ? (
              <>
                <dt>References</dt>
                <dd className="mono">{refs.map((r) => r.label ?? '?').join(', ')}</dd>
              </>
            ) : null}
          </dl>
        </section>
        {params.length ? (
          <section>
            <h3>Parameters</h3>
            <dl className="kv">
              {params.map(([k, val]) => (
                <span className="kv-row" key={k}>
                  <dt className="mono">{k}</dt>
                  <dd className="mono">{typeof val === 'object' ? JSON.stringify(val) : String(val)}</dd>
                </span>
              ))}
            </dl>
          </section>
        ) : null}
        <section>
          <h3>Identifiers</h3>
          <dl className="kv">
            <dt>Asset</dt>
            <dd className="mono small">{asset.id}</dd>
            {asset.jobId ? (
              <>
                <dt>Job</dt>
                <dd className="mono small">{asset.jobId}</dd>
              </>
            ) : null}
            {asset.openart ? (
              <>
                <dt>OpenArt</dt>
                <dd className="mono small">{asset.openart.id}</dd>
              </>
            ) : null}
          </dl>
        </section>
        {v.fromLibrary ? (
          <Button
            size="sm"
            variant="danger"
            icon={Trash}
            onClick={async () => {
              if (!window.confirm('Delete this image from the library? Nodes that use it keep a broken link.')) return;
              try {
                await api.deleteAsset(asset.id);
                await queryClient.invalidateQueries({ queryKey: qk.assets });
                useUi.getState().closeViewer();
                toast('Deleted from the library');
              } catch (e) {
                toast(describeError(e).detail, 'error');
              }
            }}
          >
            Delete from library
          </Button>
        ) : null}
      </aside>
    </div>
  );
}
