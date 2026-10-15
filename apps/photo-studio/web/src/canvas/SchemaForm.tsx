// Renders a model's parameter form from its JSON Schema: enums become chips or a select,
// bounded integers steppers, booleans toggles. Prompt and references are not shown: the prompt has
// its own editor and references come from wires. autoEnhancePrompt stays off (see jobParams).
import { useId, useState } from 'react';
import { ChevronRight } from 'lucide-react';
import type { JSONSchema } from '../api/types';
import { Chips, Stepper, Toggle } from '../components/ui';
import { humanizeKey } from '../lib/util';

const HIDDEN = new Set(['prompt', 'visualReferences', 'autoEnhancePrompt']);
const ORDER = ['imageCount', 'aspectRatio', 'resolution', 'resolutionTier', 'quality', 'mode', 'resultType', 'seriesAmount', 'thinkingMode'];
export const COMMON = new Set(['imageCount', 'aspectRatio', 'resolution', 'resolutionTier', 'quality', 'resultType', 'seriesAmount', 'mode']);

type Values = Record<string, unknown>;

function isType(p: JSONSchema, t: string): boolean {
  return p.type === t || (Array.isArray(p.type) && p.type.includes(t));
}

export function fieldsOf(schema: JSONSchema | undefined): [string, JSONSchema][] {
  const props = schema?.properties ?? {};
  return Object.entries(props)
    .filter(([k, p]) => !HIDDEN.has(k) && p.const === undefined && !isType(p, 'array') && !isType(p, 'object'))
    .sort(([a], [b]) => {
      const ia = ORDER.indexOf(a);
      const ib = ORDER.indexOf(b);
      return (ia < 0 ? 99 : ia) - (ib < 0 ? 99 : ib);
    });
}

function visible(key: string, values: Values, schema: JSONSchema): boolean {
  const v = (k: string) => values[k] ?? schema.properties?.[k]?.default;
  if (key === 'seriesAmount') return v('resultType') === 'series';
  if (key === 'outputCompression') return v('outputFormat') === 'jpeg' || v('outputFormat') === 'webp';
  if (key === 'customWidth' || key === 'customHeight') return v('lockAspectRatio') === false;
  return true;
}

export function valid(p: JSONSchema, v: unknown): boolean {
  if (v === undefined || v === null || v === '') return false;
  if (Array.isArray(p.enum)) return p.enum.includes(v);
  if (isType(p, 'integer') || isType(p, 'number')) {
    if (typeof v !== 'number' || !Number.isFinite(v)) return false;
    if (isType(p, 'integer') && !Number.isInteger(v)) return false;
    if (typeof p.minimum === 'number' && v < p.minimum) return false;
    if (typeof p.maximum === 'number' && v > p.maximum) return false;
    if (typeof p.exclusiveMinimum === 'number' && v <= p.exclusiveMinimum) return false;
    return true;
  }
  if (isType(p, 'boolean')) return typeof v === 'boolean';
  if (isType(p, 'string')) return typeof v === 'string' && (typeof p.maxLength !== 'number' || v.length <= p.maxLength);
  return true;
}

/** Keeps values the new schema accepts, drops the rest (their defaults apply). */
export function coerceParams(schema: JSONSchema, values: Values): Values {
  const out: Values = {};
  for (const [k, p] of fieldsOf(schema)) if (valid(p, values[k])) out[k] = values[k];
  return out;
}

function fmtEnum(v: unknown): string {
  return String(v).replace(/_/g, ' ');
}

function Control({ k, p, value, onChange }: { k: string; p: JSONSchema; value: unknown; onChange: (v: unknown) => void }) {
  const label = humanizeKey(k);
  const id = useId();
  if (Array.isArray(p.enum)) {
    const opts = p.enum.map(String);
    const cur = value === undefined ? undefined : String(value);
    if (opts.length <= 4) return <Chips options={opts} value={cur} onChange={(v) => onChange(p.enum?.find((x) => String(x) === v))} label={label} format={fmtEnum} />;
    return (
      <select className="select nodrag mono" id={id} aria-label={label} value={cur ?? ''} onChange={(e) => onChange(p.enum?.find((x) => String(x) === e.target.value))}>
        {opts.map((o) => (
          <option key={o} value={o}>
            {fmtEnum(o)}
          </option>
        ))}
      </select>
    );
  }
  if (isType(p, 'boolean')) return <Toggle checked={Boolean(value)} onChange={onChange} label={label} />;
  if (isType(p, 'integer') && typeof p.minimum === 'number' && typeof p.maximum === 'number' && p.maximum - p.minimum <= 16)
    return <Stepper value={typeof value === 'number' ? value : p.minimum} min={p.minimum} max={p.maximum} onChange={onChange} label={label} />;
  if (isType(p, 'integer') || isType(p, 'number'))
    return (
      <input
        className="input nodrag mono"
        type="number"
        aria-label={label}
        value={typeof value === 'number' ? value : ''}
        min={p.minimum ?? (typeof p.exclusiveMinimum === 'number' ? p.exclusiveMinimum + 1 : undefined)}
        max={p.maximum}
        step={isType(p, 'integer') ? 1 : 0.1}
        placeholder="auto"
        onChange={(e) => {
          const n = e.target.value === '' ? undefined : Number(e.target.value);
          onChange(n !== undefined && Number.isFinite(n) ? n : undefined);
        }}
      />
    );
  return (
    <input
      className="input nodrag"
      aria-label={label}
      value={typeof value === 'string' ? value : ''}
      maxLength={p.maxLength}
      onChange={(e) => onChange(e.target.value || undefined)}
    />
  );
}

export function SchemaForm({
  schema,
  values,
  onChange,
  only,
}: {
  schema: JSONSchema;
  values: Values;
  onChange: (key: string, v: unknown) => void;
  /** 'common' for app mode: just the few fields a client would touch. */
  only?: 'common';
}) {
  const [more, setMore] = useState(false);
  const fields = fieldsOf(schema).filter(([k]) => visible(k, values, schema));
  const common = fields.filter(([k]) => COMMON.has(k));
  const advanced = only ? [] : fields.filter(([k]) => !COMMON.has(k));
  const row = ([k, p]: [string, JSONSchema]) => (
    <div className="param" key={k}>
      <span className="param-label">{humanizeKey(k)}</span>
      <div className="param-control">
        <Control k={k} p={p} value={values[k] ?? p.default} onChange={(v) => onChange(k, v)} />
      </div>
    </div>
  );
  return (
    <div className="params">
      {common.map(row)}
      {advanced.length ? (
        <>
          <button type="button" className="disclosure nodrag" aria-expanded={more} onClick={() => setMore((v) => !v)}>
            <ChevronRight size={12} className={more ? 'rot' : ''} aria-hidden />
            More settings <span className="muted mono">{advanced.length}</span>
          </button>
          {more ? advanced.map(row) : null}
        </>
      ) : null}
    </div>
  );
}
