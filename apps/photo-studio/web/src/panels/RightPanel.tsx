import { useMemo, useRef, useState } from 'react';
import { Upload, X } from 'lucide-react';
import { api } from '../api/client';
import { describeError } from '../api/errors';
import { findModel, qk, queryClient, useActiveJobs, useAssets, useModels } from '../api/queries';
import type { Asset, Job } from '../api/types';
import { ASSET_MIME } from '../canvas/dnd';
import { toast, useUi } from '../state/ui';
import { Button, Chips, ErrorNote, IconButton, Spinner, errorFrom } from '../components/ui';
import { formatCredits, formatDate, formatElapsed, timeMs, validateUpload } from '../lib/util';
import { useEffect } from 'react';

const KINDS = ['all', 'uploads', 'generations'] as const;

function Library() {
  const q = useAssets();
  const models = useModels();
  const [kind, setKind] = useState<(typeof KINDS)[number]>('all');
  const [model, setModel] = useState('');
  const [uploading, setUploading] = useState(0);
  const fileRef = useRef<HTMLInputElement>(null);
  const all = useMemo(() => q.data?.pages.flatMap((p) => p.assets) ?? [], [q.data]);
  const modelIds = useMemo(() => [...new Set(all.map((a) => a.model).filter((m): m is string => Boolean(m)))], [all]);
  const shown = all.filter(
    (a) => (kind === 'all' || (kind === 'uploads' ? a.kind === 'upload' : a.kind === 'generation')) && (!model || a.model === model),
  );

  const upload = async (files: File[]) => {
    for (const f of files) {
      const bad = validateUpload(f);
      if (bad) {
        toast(bad, 'error');
        continue;
      }
      setUploading((n) => n + 1);
      try {
        await api.uploadAsset(f);
      } catch (e) {
        toast(`Upload failed: ${describeError(e).detail}`, 'error');
      } finally {
        setUploading((n) => n - 1);
      }
    }
    void queryClient.invalidateQueries({ queryKey: qk.assets });
  };

  return (
    <div
      className="library"
      onDragOver={(e) => {
        if (e.dataTransfer.types.includes('Files')) e.preventDefault();
      }}
      onDrop={(e) => {
        if (!e.dataTransfer.files.length) return;
        e.preventDefault();
        void upload(Array.from(e.dataTransfer.files));
      }}
    >
      <div className="library-tools">
        <Chips options={KINDS} value={kind} onChange={setKind} label="Filter by kind" />
        <Button size="sm" icon={Upload} busy={uploading > 0} onClick={() => fileRef.current?.click()}>
          Upload
        </Button>
        <input
          ref={fileRef}
          type="file"
          multiple
          hidden
          accept="image/png,image/jpeg,image/webp"
          onChange={(e) => {
            void upload(Array.from(e.target.files ?? []));
            e.target.value = '';
          }}
        />
      </div>
      {modelIds.length ? (
        <select className="select select-full mono" value={model} onChange={(e) => setModel(e.target.value)} aria-label="Filter by model">
          <option value="">All models</option>
          {modelIds.map((m) => (
            <option key={m} value={m}>
              {findModel(models.data?.models, m)?.name ?? m}
            </option>
          ))}
        </select>
      ) : null}
      {q.isLoading ? (
        <div className="panel-empty">
          <Spinner />
        </div>
      ) : q.error ? (
        <ErrorNote error={errorFrom(q.error)} onRetry={() => void q.refetch()} />
      ) : shown.length === 0 ? (
        <div className="panel-empty">{all.length ? 'Nothing matches the filter.' : 'No images yet. Upload a photo or drop one here.'}</div>
      ) : (
        <div className="lib-grid">
          {shown.map((a, i) => (
            <button
              key={a.id}
              type="button"
              className="lib-item"
              draggable
              onDragStart={(e) => {
                e.dataTransfer.setData(ASSET_MIME, JSON.stringify(a));
                e.dataTransfer.effectAllowed = 'copy';
              }}
              onClick={() => useUi.getState().openViewer({ assets: shown, index: i, fromLibrary: true })}
              title={a.prompt ?? (a.kind === 'upload' ? 'Upload' : 'Generation')}
            >
              <img src={a.thumb} alt={a.prompt ?? ''} loading="lazy" decoding="async" draggable={false} />
              <span className={`lib-kind ${a.kind}`}>{a.kind === 'upload' ? 'up' : 'gen'}</span>
            </button>
          ))}
        </div>
      )}
      {q.hasNextPage ? (
        <Button size="sm" className="full" busy={q.isFetchingNextPage} onClick={() => void q.fetchNextPage()}>
          Load more
        </Button>
      ) : null}
      <p className="panel-note">Drag an image onto the canvas to make a Reference node.</p>
    </div>
  );
}

function JobRow({ job, nodeTitle, nodeId }: { job: Job; nodeTitle?: string; nodeId?: string }) {
  const models = useModels();
  const [now, setNow] = useState(() => Date.now());
  const live = job.status === 'queued' || job.status === 'running';
  useEffect(() => {
    if (!live) return;
    const t = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(t);
  }, [live]);
  const count = Number(job.params.imageCount ?? 1);
  const res = String(job.params.resolution ?? job.params.resolutionTier ?? '');
  const took = (live ? now : timeMs(job.updatedAt)) - timeMs(job.createdAt);
  return (
    <li className={`job job-${job.status}`}>
      <button type="button" className="job-main" onClick={() => nodeId && useUi.getState().focus(nodeId)} disabled={!nodeId}>
        <span className={`dot ${job.status === 'completed' ? 'dot-ok' : job.status === 'failed' ? 'dot-danger' : 'dot-run'}`} />
        <span className="job-text">
          <span className="job-title">{findModel(models.data?.models, job.model)?.name ?? job.model}</span>
          <span className="job-sub mono">
            {job.mode.replace('2', '→')} · {Number.isFinite(count) ? count : 1}×{res ? ` ${res}` : ''}
            {nodeTitle ? ` · ${nodeTitle}` : ''}
          </span>
          {job.error ? <span className="job-error">{job.error}</span> : null}
        </span>
        <span className="job-meta mono">
          <span>{job.status}</span>
          <span>{formatElapsed(took)}</span>
          {job.credits !== undefined ? <span>{formatCredits(job.credits)} cr</span> : null}
        </span>
      </button>
    </li>
  );
}

function Jobs() {
  const session = useUi((s) => s.sessionJobs);
  const active = useActiveJobs(true);
  const rows = new Map<string, { job: Job; nodeId?: string; nodeTitle?: string }>();
  for (const j of active.data?.jobs ?? []) rows.set(j.id, { job: j });
  for (const s of Object.values(session)) rows.set(s.job.id, { ...rows.get(s.job.id), ...s });
  const list = [...rows.values()].sort((a, b) => b.job.createdAt - a.job.createdAt);
  const running = list.filter((r) => r.job.status === 'queued' || r.job.status === 'running');
  const done = list.filter((r) => !(r.job.status === 'queued' || r.job.status === 'running'));
  return (
    <div className="jobs">
      <div className="side-sub">Running {running.length ? <span className="mono muted">{running.length}</span> : null}</div>
      {running.length ? (
        <ul className="job-list">
          {running.map((r) => (
            <JobRow key={r.job.id} {...r} />
          ))}
        </ul>
      ) : (
        <div className="panel-empty">No jobs running.</div>
      )}
      {done.length ? (
        <>
          <div className="side-sub">This session</div>
          <ul className="job-list">
            {done.map((r) => (
              <JobRow key={r.job.id} {...r} />
            ))}
          </ul>
        </>
      ) : null}
      <p className="panel-note">Jobs keep running on the server if you close the tab; open the workflow again to collect them.</p>
    </div>
  );
}

export function RightPanel() {
  const open = useUi((s) => s.rightOpen);
  const tab = useUi((s) => s.rightTab);
  const activeCount = useUi((s) => Object.values(s.sessionJobs).filter((j) => j.job.status === 'queued' || j.job.status === 'running').length);
  if (!open) return null;
  return (
    <aside className="side side-right" aria-label="Library and jobs">
      <div className="tabs" role="tablist">
        <button type="button" role="tab" aria-selected={tab === 'library'} className={tab === 'library' ? 'is-on' : ''} onClick={() => useUi.getState().setRight(true, 'library')}>
          Library
        </button>
        <button type="button" role="tab" aria-selected={tab === 'jobs'} className={tab === 'jobs' ? 'is-on' : ''} onClick={() => useUi.getState().setRight(true, 'jobs')}>
          Jobs {activeCount ? <span className="mono count-inline">{activeCount}</span> : null}
        </button>
        <span className="spacer" />
        <IconButton icon={X} label="Close panel" onClick={() => useUi.getState().setRight(false)} />
      </div>
      <div className="side-scroll">{tab === 'library' ? <Library /> : <Jobs />}</div>
    </aside>
  );
}

export function assetLabel(a: Asset): string {
  return `${a.kind === 'upload' ? 'Upload' : 'Generation'} · ${formatDate(a.createdAt)}`;
}
