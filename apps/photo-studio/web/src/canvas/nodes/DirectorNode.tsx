import type { NodeProps } from '@xyflow/react';
import { ArrowRight, CircleQuestionMark, Plus } from 'lucide-react';
import type { AppNode, DirectorData } from '../../graph/types';
import { useGraph } from '../../graph/store';
import { useNodeInputs, useRuntime } from '../../graph/hooks';
import { runNode } from '../../graph/run';
import { spawnFromPlan } from '../../graph/actions';
import { findModel, useMe, useModels } from '../../api/queries';
import { Button, Chips, SectionLabel } from '../../components/ui';
import { ChipList } from '../ChipList';
import { NodeShell } from './NodeShell';

const TASKS = ['auto', 'generate', 'edit'] as const;

export function DirectorNode({ id, data, selected }: NodeProps<Extract<AppNode, { type: 'director' }>>) {
  const d = data as DirectorData;
  const ins = useNodeInputs(id);
  const rt = useRuntime(id);
  const models = useModels();
  const me = useMe();
  const res = d.result;
  const update = useGraph.getState().updateData;
  const briefWired = (ins.wires.brief ?? 0) > 0;
  const refs = ins.images.refs ?? [];
  const hasSource = refs.some((r) => r.role === 'edit-source');
  const name = (m: string) => findModel(models.data?.models, m)?.name ?? m;
  const claudeOff = me.data && !me.data.claude.configured;

  return (
    <NodeShell id={id} kind="director" selected={selected} title={d.title} exposed={d.exposed} error={d.error} run="node" done={Boolean(res)}>
      <div className="param">
        <span className="param-label">Task</span>
        <div className="param-control">
          <Chips options={TASKS} value={d.task} onChange={(task) => update(id, { task })} label="Task" />
        </div>
      </div>
      <p className="hint">
        {briefWired ? `Brief connected · ${refs.length} reference${refs.length === 1 ? '' : 's'}` : 'Connect a Brief, and references if any.'}
        {d.task === 'auto' && hasSource ? ' · edit (a source photo is wired)' : ''}
      </p>
      {claudeOff ? (
        <p className="hint warn">Claude is not configured on the server (ANTHROPIC_API_KEY), so the Director cannot run.</p>
      ) : null}

      {!res ? (
        <Button variant="primary" size="sm" className="nodrag full" busy={Boolean(rt)} disabled={Boolean(rt) || !briefWired} onClick={() => void runNode(id)}>
          {rt ? 'Directing' : 'Direct the shot'}
        </Button>
      ) : (
        <>
          <div className="director-cat">
            <span className="param-label">Category</span>
            <span className="badge badge-strong mono">{res.category}</span>
          </div>
          <SectionLabel>Plan</SectionLabel>
          <ol className="plan">
            {res.plan.map((p, i) => (
              <li key={`${p.model}-${i}`} className={i === 0 ? 'is-first' : ''}>
                <div className="plan-top">
                  <span className="mono plan-n">{i + 1}</span>
                  <span className="plan-model">{name(p.model)}</span>
                  <span className="mono mode-tag">{p.mode.replace('2', '→')}</span>
                  <Button size="sm" variant="ghost" icon={Plus} className="nodrag plan-add" onClick={() => spawnFromPlan(id, i)} title="Create a node pre-filled with this step">
                    {hasSource ? 'Edit' : 'Generate'}
                  </Button>
                </div>
                <p className="plan-why">{p.why}</p>
                {i < res.plan.length - 1 ? <ArrowRight size={11} className="plan-arrow" aria-hidden /> : null}
              </li>
            ))}
          </ol>
          <SectionLabel aside={<span className="mono">{(d.prompt ?? res.prompt).length}</span>}>Prompt</SectionLabel>
          <textarea
            className="textarea nodrag nowheel"
            rows={5}
            value={d.prompt ?? res.prompt}
            aria-label="Director prompt"
            onChange={(e) => update(id, { prompt: e.target.value }, 'coalesce')}
          />
          {res.preserve ? (
            <>
              <SectionLabel>Preserve</SectionLabel>
              <ChipList
                items={res.preserve}
                onChange={(preserve) => update(id, { result: { ...res, preserve } })}
                placeholder="add…"
                label="Preserve"
              />
            </>
          ) : null}
          {res.questions?.length ? (
            <div className="questions" role="note">
              <SectionLabel>Questions</SectionLabel>
              <ul>
                {res.questions.map((q) => (
                  <li key={q}>
                    <CircleQuestionMark size={12} aria-hidden /> {q}
                  </li>
                ))}
              </ul>
            </div>
          ) : null}
          <details className="spec nodrag">
            <summary>Shot spec</summary>
            <dl>
              <dt>Use</dt>
              <dd>{res.spec.use}</dd>
              <dt>Format</dt>
              <dd className="mono">
                {res.spec.format.aspectRatio} · {res.spec.format.resolution}
                {res.spec.format.copySpace ? ` · copy ${res.spec.format.copySpace}` : ''}
              </dd>
              {(['subject', 'setting', 'light', 'camera', 'medium', 'retouch', 'composition', 'moment'] as const).map((k) => (
                <span key={k} className="spec-row">
                  <dt>{k}</dt>
                  <dd>{res.spec[k]}</dd>
                </span>
              ))}
            </dl>
          </details>
          {res.notes ? <p className="hint">{res.notes}</p> : null}
        </>
      )}
    </NodeShell>
  );
}
