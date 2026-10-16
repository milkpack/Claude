import type { DragEvent } from 'react';
import { useRef, useState } from 'react';
import { ClipboardCopy, Cloud, ExternalLink, Paperclip, RotateCw, Send, TriangleAlert, Upload, X } from 'lucide-react';
import { useMe } from '../api/queries';
import { api } from '../api/client';
import type { Task } from '../api/types';
import { AUTO_KINDS, cancelHandoff, createHandoff, fireHandoff, pasteAnswer, uploadResults, useHandoffPoll } from '../graph/handoff';
import { toast } from '../state/ui';
import { copyText, formatDate } from '../lib/util';
import { Button, IconButton } from '../components/ui';

const ANSWER_HINT: Record<string, string> = {
  director: 'Paste Claude’s answer (the ```json block)…',
  review: 'Paste Claude’s review JSON…',
  blender: 'Notes from Claude (optional); drop the renders on the node…',
  photoshop: 'Notes from Claude (optional); drop the edited PNG on the node…',
  sketch: 'Paste the list of issues…',
};

/** Free-text notes from Claude are shown for the image-returning kinds (JSON kinds apply instead). */
const kindShowsText = (k: string) => k === 'blender' || k === 'photoshop' || k === 'sketch';

/**
 * The Claude handoff card shared by Director, Review, Blender, Photoshop and Sketch: create the
 * task, copy it for Claude, open the return page, follow its status, paste or drop the answer.
 */
export function TaskCard({
  id,
  kind,
  task,
  applied,
  createLabel = 'Create task',
  acceptsFiles,
}: {
  id: string;
  kind: string;
  task: Task | undefined;
  applied: number | undefined;
  createLabel?: string;
  acceptsFiles?: boolean;
}) {
  const [busy, setBusy] = useState(false);
  const [answer, setAnswer] = useState('');
  const [open, setOpen] = useState(false);
  const [confirmFire, setConfirmFire] = useState(false);
  const fileRef = useRef<HTMLInputElement>(null);
  const me = useMe();
  const cloud = Boolean(me.data?.routine?.configured) && AUTO_KINDS.includes(kind);
  useHandoffPoll(id, task, applied);

  const create = async (auto?: boolean) => {
    setBusy(true);
    await createHandoff(id, { auto });
    setBusy(false);
  };
  const status = task?.status ?? 'none';
  const onDrop = (e: DragEvent) => {
    if (!acceptsFiles || !e.dataTransfer.files.length) return;
    e.preventDefault();
    e.stopPropagation();
    void uploadResults(id, Array.from(e.dataTransfer.files), answer || undefined);
  };

  return (
    <div
      className={`task-card status-${status}`}
      data-testid="task-card"
      onDragOver={(e) => {
        if (acceptsFiles && e.dataTransfer.types.includes('Files')) {
          e.preventDefault();
          e.stopPropagation();
        }
      }}
      onDrop={onDrop}
    >
      <div className="task-head">
        <span className="task-kicker">Handoff to Claude</span>
        <span className={`task-status mono s-${status}`}>{status === 'none' ? 'no task' : status === 'open' ? 'waiting' : status}</span>
      </div>
      {task ? (
        <>
          <p className="task-title" title={task.title}>
            {task.title}
          </p>
          <p className="task-meta mono">
            {task.inputs.length} input{task.inputs.length === 1 ? '' : 's'}
            {task.roles?.length ? ` (${task.roles.join(', ')})` : ''} · {formatDate(task.createdAt)}
            {task.expiresAt && status === 'open' ? ` · links until ${formatDate(task.expiresAt)}` : ''}
          </p>
          {task.run ? (
            <div className={`task-run run-${task.run.state}`}>
              <Cloud size={12} aria-hidden />
              <span>
                {task.run.state === 'failed'
                  ? 'Cloud run failed'
                  : status === 'open'
                    ? `Claude is working in the cloud · since ${formatDate(task.run.at)}`
                    : `Answered in the cloud`}
              </span>
              {task.run.sessionUrl ? (
                <a className="nodrag" href={task.run.sessionUrl} target="_blank" rel="noreferrer">
                  session
                </a>
              ) : null}
            </div>
          ) : null}
          {task.run?.error ? (
            <p className="task-error mono" role="alert">
              {task.run.error}
            </p>
          ) : null}
          {cloud && status === 'open' ? (
            confirmFire ? (
              <div className="task-confirm" role="alertdialog" aria-label="Run again in the cloud?">
                <span>{task.run?.state === 'fired' ? 'The previous cloud run may still be going; running again can produce a second answer.' : 'Send this task to Claude in the cloud again?'}</span>
                <div className="task-actions">
                  <Button size="sm" variant="primary" className="nodrag" onClick={() => { setConfirmFire(false); void fireHandoff(id); }}>
                    Run again
                  </Button>
                  <Button size="sm" variant="ghost" className="nodrag" onClick={() => setConfirmFire(false)}>
                    Cancel
                  </Button>
                </div>
              </div>
            ) : (
              <Button size="sm" variant="ghost" icon={RotateCw} className="nodrag task-new" onClick={() => setConfirmFire(true)}>
                Run again in the cloud
              </Button>
            )
          ) : null}
          {task.result?.error ? (
            <p className="task-error" role="alert">
              <TriangleAlert size={12} aria-hidden /> {task.result.error}
            </p>
          ) : null}
          {task.result?.text && kindShowsText(kind) ? <p className="task-note">{task.result.text}</p> : null}
          {task.result?.files?.length ? (
            <ul className="task-files">
              {task.result.files.map((f) => (
                <li key={f.name}>
                  <a className="nodrag" href={api.taskFileUrl(task.id, f.name)} download={f.name}>
                    <Paperclip size={12} aria-hidden /> {f.name}
                  </a>
                  {f.size ? <span className="mono muted"> {(f.size / 1_048_576).toFixed(1)} MB</span> : null}
                </li>
              ))}
            </ul>
          ) : null}
          <div className="task-actions">
            <Button
              size="sm"
              variant="primary"
              icon={ClipboardCopy}
              className="nodrag"
              onClick={async () => toast((await copyText(task.prompt)) ? 'Task copied: paste it into Claude' : 'Could not copy', 'ok')}
            >
              Copy for Claude
            </Button>
            <a className="btn btn-sm btn-ghost nodrag" href={task.returnUrl} target="_blank" rel="noreferrer">
              <ExternalLink size={13} aria-hidden /> Return page
            </a>
            <span className="spacer" />
            {status === 'open' ? <IconButton icon={X} label="Cancel task" size={13} className="nodrag" onClick={() => void cancelHandoff(id)} /> : null}
          </div>
          {status !== 'open' ? (
            <Button size="sm" variant="ghost" className="nodrag task-new" busy={busy} onClick={() => void create()}>
              New task from current inputs
            </Button>
          ) : null}
        </>
      ) : (
        <>
          <p className="hint">
            {cloud
              ? 'Runs in the cloud: a Claude routine picks up the task and the answer comes back here. You can still copy it into Claude yourself.'
              : 'Builds a complete task you paste into Claude (desktop, Claude Code or a cloud session). The answer comes back here.'}
          </p>
          <Button size="sm" variant="primary" icon={cloud ? Cloud : undefined} className="nodrag full" busy={busy} onClick={() => void create(cloud)}>
            {cloud ? 'Run in the cloud' : createLabel}
          </Button>
          {cloud ? (
            <Button size="sm" variant="ghost" className="nodrag task-new" disabled={busy} onClick={() => void create(false)}>
              Create the task only (copy it yourself)
            </Button>
          ) : null}
        </>
      )}
      <button type="button" className="disclosure nodrag" aria-expanded={open} onClick={() => setOpen((v) => !v)}>
        {open ? '−' : '+'} {acceptsFiles ? 'Paste notes or upload results' : 'Paste answer'}
      </button>
      {open ? (
        <div className="task-answer">
          <textarea
            className="textarea nodrag nowheel mono"
            rows={4}
            value={answer}
            placeholder={ANSWER_HINT[kind] ?? 'Paste Claude’s answer…'}
            aria-label="Claude’s answer"
            onChange={(e) => setAnswer(e.target.value)}
          />
          <div className="task-actions">
            <Button
              size="sm"
              icon={Send}
              className="nodrag"
              disabled={!answer.trim()}
              onClick={async () => {
                if (await pasteAnswer(id, answer)) {
                  setAnswer('');
                  setOpen(false);
                  toast('Answer applied', 'ok');
                }
              }}
            >
              Apply
            </Button>
            {acceptsFiles ? (
              <>
                <Button size="sm" variant="ghost" icon={Upload} className="nodrag" onClick={() => fileRef.current?.click()}>
                  Upload results
                </Button>
                <input
                  ref={fileRef}
                  type="file"
                  multiple
                  hidden
                  accept="image/png,image/jpeg,image/webp"
                  onChange={(e) => {
                    void uploadResults(id, Array.from(e.target.files ?? []), answer || undefined);
                    e.target.value = '';
                  }}
                />
              </>
            ) : null}
          </div>
        </div>
      ) : null}
    </div>
  );
}
