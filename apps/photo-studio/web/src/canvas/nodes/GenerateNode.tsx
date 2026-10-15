import { useEffect, useMemo } from 'react';
import type { NodeProps } from '@xyflow/react';
import { Play } from 'lucide-react';
import type { AppNode, GenData } from '../../graph/types';
import { useGraph } from '../../graph/store';
import { useNodeInputs, useRuntime } from '../../graph/hooks';
import { editPrompt, generatePrompt, keepList, orderedRefs, pickMode, recommendedModels } from '../../graph/evaluate';
import { jobParams, runNode } from '../../graph/run';
import { findModel, useModelForm, useModels } from '../../api/queries';
import { errorCode } from '../../api/errors';
import { Button, ErrorNote, SectionLabel, Spinner, errorFrom } from '../../components/ui';
import { SchemaForm, coerceParams } from '../SchemaForm';
import { ModelPicker } from '../ModelPicker';
import { CostLine } from '../CostLine';
import { ResultsGrid } from '../ResultsGrid';
import { ChipList } from '../ChipList';
import { roleLabel } from '../../graph/evaluate';
import { useUi } from '../../state/ui';
import { NodeShell } from './NodeShell';
import { stableStringify } from '../../lib/util';

type GenNode = Extract<AppNode, { type: 'generate' | 'edit' }>;

export function GenerateNode({ id, data, selected, type }: NodeProps<GenNode>) {
  const d = data as GenData;
  const kind = type === 'edit' ? 'edit' : 'generate';
  const ins = useNodeInputs(id);
  const rt = useRuntime(id);
  const models = useModels();
  const refs = useMemo(() => orderedRefs(kind, ins), [kind, ins]);
  const modelId = d.model || ins.spec?.plan[0]?.model;
  const model = findModel(models.data?.models, modelId);
  const mode = pickMode(model, kind, refs.length > 0);
  const form = useModelForm(model?.id, mode);
  const schema = form.data;
  const update = useGraph.getState().updateData;

  // Keep only parameters the (new) schema accepts.
  useEffect(() => {
    if (!schema) return;
    const next = coerceParams(schema, d.params);
    if (stableStringify(next) !== stableStringify(d.params)) update(id, { params: next }, 'none');
  }, [schema, d.params, id, update]);

  const { recommended, rest } = useMemo(
    () =>
      recommendedModels(models.data?.models ?? [], {
        plan: ins.spec?.plan.map((p) => p.model),
        category: ins.spec?.category ?? d.category,
        needI2I: kind === 'edit',
      }),
    [models.data, ins.spec, d.category, kind],
  );

  const prompt = kind === 'edit' ? editPrompt(d, ins, refs) : generatePrompt(d, ins);
  const instruction = kind === 'edit' ? (prompt as ReturnType<typeof editPrompt>).instruction : prompt.text;
  const costParams = useMemo(() => {
    const p = schema ? jobParams(schema, d.params, '') : { ...d.params };
    delete p.prompt;
    return p;
  }, [schema, d.params]);
  const maxRefs = schema?.properties?.visualReferences?.maxItems;
  const expected = Number(costParams.resultType === 'series' ? costParams.seriesAmount : costParams.imageCount) || 1;
  const running = Boolean(rt);
  const hasSource = refs.some((r) => r.isSource);
  const modelsErr = models.error ? errorFrom(models.error) : null;

  const promptEditor = (label: string, placeholder: string) =>
    prompt.source === 'local' ? (
      <textarea
        className="textarea nodrag nowheel"
        rows={kind === 'edit' ? 2 : 4}
        value={d.prompt}
        placeholder={placeholder}
        aria-label={label}
        onChange={(e) => update(id, { prompt: e.target.value }, 'coalesce')}
      />
    ) : (
      <div className="wired-text nowheel" title="Comes in on a wire. Disconnect it to write your own.">
        <span className={`wired-from type-${prompt.source === 'spec' ? 'spec' : 'text'}`}>{prompt.source === 'spec' ? 'from Director' : 'from wire'}</span>
        {instruction ? <p>{instruction}</p> : <p className="muted">Waiting for upstream text — run it first.</p>}
      </div>
    );

  return (
    <NodeShell
      id={id}
      kind={kind}
      selected={selected}
      title={d.title}
      exposed={d.exposed}
      error={d.error}
      run="node"
      done={d.runs.length > 0}
      phase={rt?.phase}
    >
      {modelsErr && (modelsErr.code === 'openart_not_connected' || modelsErr.code === 'openart_reconnect') ? (
        <ErrorNote error={modelsErr} compact />
      ) : (
        <ModelPicker
          value={model}
          mode={mode}
          recommended={recommended}
          rest={rest}
          autoNote={!d.model && model ? 'plan' : undefined}
          onChange={(m) => update(id, { model: m })}
        />
      )}

      {kind === 'edit' ? (
        <>
          <SectionLabel>Change only</SectionLabel>
          <ChipList items={d.change ?? []} onChange={(change) => update(id, { change })} placeholder="add a change…" label="Change only" />
          <SectionLabel>Instruction</SectionLabel>
          {promptEditor('Instruction', 'Anything else the edit should do (optional)')}
          <SectionLabel>Keep exactly</SectionLabel>
          <ChipList
            items={d.keep ?? []}
            onChange={(keep) => update(id, { keep })}
            placeholder="add what must not change…"
            label="Keep exactly"
            ghost={(d.keep ?? []).length ? undefined : keepList({ ...d, keep: [] }, ins)}
          />
          <details className="prompt-preview nodrag">
            <summary>Prompt sent</summary>
            <p>{prompt.text || '—'}</p>
          </details>
        </>
      ) : (
        <>
          <SectionLabel aside={prompt.text ? <span className="mono">{prompt.text.length}</span> : null}>Prompt</SectionLabel>
          {promptEditor('Prompt', 'Describe the photograph, or connect a Brief or a Director.')}
        </>
      )}

      <SectionLabel aside={<span className="mono">{refs.length}{typeof maxRefs === 'number' ? ` / ${maxRefs}` : ''}</span>}>References</SectionLabel>
      {refs.length ? (
        <div className="ref-strip">
          {refs.map((r) => (
            <button
              type="button"
              key={r.asset.id}
              className="ref-chip nodrag"
              title={`${r.label}: ${roleLabel(r.role)}`}
              onClick={() => useUi.getState().openViewer({ assets: refs.map((x) => x.asset), index: refs.indexOf(r) })}
            >
              <img src={r.asset.thumb} alt="" draggable={false} />
              <span className="mono">{r.label}</span>
              <span className="ref-role">{roleLabel(r.role)}</span>
            </button>
          ))}
        </div>
      ) : (
        <p className={`hint ${kind === 'edit' ? 'warn' : ''}`}>
          {kind === 'edit' ? 'Connect the photo to edit into source.' : 'None wired: text to image.'}
        </p>
      )}
      {kind === 'edit' && refs.length > 0 && !hasSource ? <p className="hint warn">No source photo connected.</p> : null}

      {schema ? (
        <SchemaForm schema={schema} values={d.params} onChange={(k, v) => update(id, { params: { ...d.params, [k]: v } })} />
      ) : form.isLoading ? (
        <div className="params-loading">
          <Spinner /> <span className="muted">Loading the form…</span>
        </div>
      ) : form.error && errorCode(form.error) !== 'openart_not_connected' ? (
        <ErrorNote error={errorFrom(form.error)} compact onRetry={() => void form.refetch()} />
      ) : null}

      <div className="run-row">
        <CostLine model={model?.id} mode={mode} params={costParams} />
        <Button variant="primary" size="sm" icon={running ? undefined : Play} busy={running} className="nodrag" disabled={running || !model} onClick={() => void runNode(id)}>
          {running ? 'Running' : 'Run'}
        </Button>
      </div>

      <ResultsGrid id={id} d={d} running={running} expected={expected} />
    </NodeShell>
  );
}
