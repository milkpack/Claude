// App mode: the workflow as a clean form. Shows only nodes marked "exposed" (inputs and a few
// parameters) and every Output node, like a published app.
import { useMemo, useRef, useState } from 'react';
import { Download, Library, Play, Upload } from 'lucide-react';
import type { AppNode, GenData, ReferenceData } from '../graph/types';
import { useGraph } from '../graph/store';
import { useNodeInputs, useRuntime } from '../graph/hooks';
import { hasOutput, isStale, orderedRefs, pickMode, topoSort } from '../graph/evaluate';
import { jobParams, runOutputs } from '../graph/run';
import { uploadIntoReference } from '../graph/actions';
import { findModel, useModelForm, useModels } from '../api/queries';
import { NODE_META } from '../canvas/nodeMeta';
import { SchemaForm, coerceParams } from '../canvas/SchemaForm';
import { CostLine } from '../canvas/CostLine';
import { LibraryPicker } from '../canvas/LibraryPicker';
import { RoleSelect } from '../canvas/nodes/ReferenceNode';
import { Elapsed } from '../canvas/nodes/NodeShell';
import { VerdictBadge } from '../canvas/nodes/ReviewNode';
import { useUi } from '../state/ui';
import { assetFilename, downloadUrl, stableStringify } from '../lib/util';
import { Button, ErrorNote, IconButton, Spinner } from '../components/ui';
import { ASSET_MIME, readDraggedAsset } from '../canvas/dnd';
import { gridCols } from '../canvas/ResultsGrid';
import { useEffect } from 'react';

function title(n: AppNode): string {
  return n.data.title || NODE_META[n.type as keyof typeof NODE_META].label;
}

function AppBrief({ n }: { n: Extract<AppNode, { type: 'brief' }> }) {
  return (
    <label className="app-field">
      <span className="app-label">{title(n)}</span>
      <textarea
        className="textarea"
        rows={5}
        value={n.data.text}
        placeholder="Describe the picture in your own words, any language."
        onChange={(e) => useGraph.getState().updateData(n.id, { text: e.target.value }, 'coalesce')}
      />
    </label>
  );
}

function AppReference({ n }: { n: Extract<AppNode, { type: 'reference' }> }) {
  const d = n.data as ReferenceData;
  const fileRef = useRef<HTMLInputElement>(null);
  const [picking, setPicking] = useState(false);
  const src = d.previewUrl ?? d.asset?.thumb;
  return (
    <div className="app-field">
      <span className="app-label">
        {title(n)}
        <RoleSelect value={d.role} onChange={(role) => useGraph.getState().updateData(n.id, { role })} label={`${title(n)} role`} />
      </span>
      <div
        className={`app-drop ${src ? 'has-image' : ''}`}
        onDragOver={(e) => {
          if (e.dataTransfer.types.includes('Files') || e.dataTransfer.types.includes(ASSET_MIME)) e.preventDefault();
        }}
        onDrop={(e) => {
          e.preventDefault();
          const asset = readDraggedAsset(e.dataTransfer);
          const file = Array.from(e.dataTransfer.files)[0];
          if (asset) useGraph.getState().updateData(n.id, { asset });
          else if (file) void uploadIntoReference(n.id, file);
        }}
      >
        {src ? <img src={src} alt={title(n)} /> : <span className="muted">Drop a photo here</span>}
        {d.uploading ? (
          <span className="ref-uploading">
            <Spinner /> Uploading
          </span>
        ) : null}
      </div>
      {d.error ? <ErrorNote error={d.error} compact /> : null}
      <div className="app-row">
        <Button size="sm" icon={Upload} onClick={() => fileRef.current?.click()}>
          {src ? 'Replace' : 'Upload'}
        </Button>
        <Button size="sm" variant="ghost" icon={Library} onClick={() => setPicking((v) => !v)}>
          From library
        </Button>
        <input
          ref={fileRef}
          type="file"
          hidden
          accept="image/png,image/jpeg,image/webp"
          onChange={(e) => {
            const f = e.target.files?.[0];
            if (f) void uploadIntoReference(n.id, f);
            e.target.value = '';
          }}
        />
      </div>
      {picking ? (
        <div className="rel">
          <LibraryPicker
            onClose={() => setPicking(false)}
            onPick={(asset) => {
              useGraph.getState().updateData(n.id, { asset });
              setPicking(false);
            }}
          />
        </div>
      ) : null}
    </div>
  );
}

function AppGenerate({ n }: { n: Extract<AppNode, { type: 'generate' | 'edit' }> }) {
  const d = n.data as GenData;
  const ins = useNodeInputs(n.id);
  const models = useModels();
  const refs = orderedRefs(n.type, ins);
  const model = findModel(models.data?.models, d.model || ins.spec?.plan[0]?.model);
  const mode = pickMode(model, n.type, refs.length > 0);
  const form = useModelForm(model?.id, mode);
  const schema = form.data;
  useEffect(() => {
    if (!schema) return;
    const next = coerceParams(schema, d.params);
    if (stableStringify(next) !== stableStringify(d.params)) useGraph.getState().updateData(n.id, { params: next }, 'none');
  }, [schema, d.params, n.id]);
  const costParams = useMemo(() => {
    const p = schema ? jobParams(schema, d.params, '') : { ...d.params };
    delete p.prompt;
    return p;
  }, [schema, d.params]);
  return (
    <div className="app-field">
      <span className="app-label">
        {title(n)} <span className="muted">· {model?.name ?? 'model from the plan'}</span>
      </span>
      {schema ? <SchemaForm schema={schema} values={d.params} only="common" onChange={(k, v) => useGraph.getState().updateData(n.id, { params: { ...d.params, [k]: v } })} /> : <Spinner />}
      <CostLine model={model?.id} mode={mode} params={costParams} />
    </div>
  );
}

function AppDirector({ n }: { n: Extract<AppNode, { type: 'director' }> }) {
  const r = n.data.result;
  return (
    <label className="app-field">
      <span className="app-label">{title(n)} · prompt</span>
      {r ? (
        <textarea className="textarea" rows={5} value={n.data.prompt ?? r.prompt} onChange={(e) => useGraph.getState().updateData(n.id, { prompt: e.target.value }, 'coalesce')} />
      ) : (
        <span className="muted">Written by the Director when you run.</span>
      )}
    </label>
  );
}

function StepRow({ n }: { n: AppNode }) {
  const rt = useRuntime(n.id);
  const stale = useGraph((s) => {
    const x = s.nodes.find((y) => y.id === n.id);
    return x ? isStale(x, s.nodes, s.edges) : false;
  });
  const err = (n.data as { error?: { title?: string } }).error;
  const state = rt ? 'running' : err ? 'error' : stale ? 'changed' : hasOutput(n) ? 'done' : 'waiting';
  const Icon = NODE_META[n.type as keyof typeof NODE_META].icon;
  return (
    <li className={`step step-${state}`}>
      <Icon size={13} aria-hidden />
      <span>{title(n)}</span>
      <span className="spacer" />
      <span className="mono step-state">
        {rt ? (
          <>
            {rt.phase ?? 'running'} · <Elapsed since={rt.startedAt} />
          </>
        ) : (
          state
        )}
      </span>
    </li>
  );
}

function AppOutput({ n }: { n: Extract<AppNode, { type: 'output' }> }) {
  const ins = useNodeInputs(n.id);
  const images = (ins.images.image ?? []).map((i) => i.asset);
  const review = useGraph((s) => {
    const r = s.nodes.find((x) => x.type === 'review' && s.edges.some((e) => e.target === x.id && ins.from.image?.includes(e.source)));
    return r?.type === 'review' ? r.data : undefined;
  });
  return (
    <section className="app-output">
      <div className="app-output-head">
        <h2>{title(n)}</h2>
        <span className="mono muted">{images.length ? `${images.length} image${images.length === 1 ? '' : 's'}` : ''}</span>
      </div>
      {images.length ? (
        <div className="app-grid" style={{ gridTemplateColumns: `repeat(${Math.min(gridCols(images.length), 4)}, minmax(0, 1fr))` }}>
          {images.map((a, i) => {
            const v = review?.reviews[a.id]?.verdict;
            return (
              <figure key={a.id} className="app-img" style={{ aspectRatio: `${a.width} / ${a.height}` }}>
                <button type="button" onClick={() => useUi.getState().openViewer({ assets: images, index: i, fromNode: n.id })} aria-label={`Open image ${i + 1}`}>
                  <img src={a.thumb} alt="" loading="lazy" />
                </button>
                <figcaption>
                  {v ? <VerdictBadge v={v} /> : <span />}
                  <IconButton icon={Download} label="Download" size={13} onClick={() => downloadUrl(a.file, assetFilename(a))} />
                </figcaption>
              </figure>
            );
          })}
        </div>
      ) : (
        <div className="app-placeholder">
          <span className="muted">Results appear here.</span>
        </div>
      )}
    </section>
  );
}

export function AppMode() {
  const nodes = useGraph((s) => s.nodes);
  const edges = useGraph((s) => s.edges);
  const name = useGraph((s) => s.name);
  const anyRunning = useGraph((s) => Object.keys(s.runtime).length > 0);
  const order = useMemo(() => topoSort(nodes.map((n) => n.id), edges), [nodes, edges]);
  const byId = useMemo(() => new Map(nodes.map((n) => [n.id, n])), [nodes]);
  const ordered = order.map((id) => byId.get(id)).filter((n): n is AppNode => Boolean(n));
  const inputs = ordered.filter((n) => n.data.exposed && n.type !== 'output');
  const outputs = ordered.filter((n) => n.type === 'output');
  const steps = ordered.filter((n) => ['director', 'generate', 'edit', 'review'].includes(n.type ?? ''));
  const missing = ordered.filter((n) => n.type === 'reference' && n.data.exposed && !(n.data as ReferenceData).asset).length;
  const emptyBrief = ordered.some((n) => n.type === 'brief' && n.data.exposed && !n.data.text.trim());

  return (
    <main className="appmode">
      <div className="app-form">
        <header className="app-head">
          <span className="mono app-kicker">App</span>
          <h1>{name}</h1>
          <p className="muted">Fill in the inputs and run. The canvas behind this form does the rest.</p>
        </header>
        {inputs.length === 0 ? (
          <p className="hint">Nothing is exposed yet. In the canvas, open a node’s menu and choose “Show in app mode”.</p>
        ) : (
          inputs.map((n) => {
            switch (n.type) {
              case 'brief':
                return <AppBrief key={n.id} n={n} />;
              case 'reference':
                return <AppReference key={n.id} n={n} />;
              case 'generate':
              case 'edit':
                return <AppGenerate key={n.id} n={n} />;
              case 'director':
                return <AppDirector key={n.id} n={n} />;
              default:
                return null;
            }
          })
        )}
        <div className="app-run">
          <Button variant="primary" icon={anyRunning ? undefined : Play} busy={anyRunning} disabled={anyRunning || !outputs.length} onClick={() => void runOutputs()}>
            {anyRunning ? 'Running' : 'Run'}
          </Button>
          <span className="muted small">
            {!outputs.length ? 'Add an Output node in the canvas.' : missing ? `${missing} reference${missing === 1 ? '' : 's'} still empty` : emptyBrief ? 'The brief is empty' : 'Ready'}
          </span>
        </div>
        {steps.length ? (
          <ol className="steps" aria-label="Steps">
            {steps.map((n) => (
              <StepRow key={n.id} n={n} />
            ))}
          </ol>
        ) : null}
        {steps
          .map((n) => ({ n, e: (n.data as { error?: { title?: string; message: string; code?: string } }).error }))
          .filter((x) => x.e)
          .map(({ n, e }) => (e ? <ErrorNote key={n.id} error={e} /> : null))}
      </div>
      <div className="app-results">{outputs.length ? outputs.map((n) => (n.type === 'output' ? <AppOutput key={n.id} n={n} /> : null)) : null}</div>
    </main>
  );
}
