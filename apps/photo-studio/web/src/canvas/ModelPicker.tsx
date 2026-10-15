import { useEffect, useRef, useState } from 'react';
import { ChevronDown } from 'lucide-react';
import type { Model } from '../api/types';
import { formatCredits } from '../lib/util';

function modeCredits(m: Model, mode: string | undefined) {
  const modes = m.modes.image ?? [];
  return modes.find((x) => x.mode === mode) ?? modes[0];
}

export function ModelPicker({
  value,
  mode,
  recommended,
  rest,
  onChange,
  autoNote,
}: {
  value: Model | undefined;
  mode: string | undefined;
  recommended: Model[];
  rest: Model[];
  onChange: (id: string) => void;
  /** Shown when the model is not chosen on the node but comes from the Director's plan. */
  autoNote?: string;
}) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const onDown = (e: PointerEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && setOpen(false);
    window.addEventListener('pointerdown', onDown, true);
    window.addEventListener('keydown', onKey);
    return () => {
      window.removeEventListener('pointerdown', onDown, true);
      window.removeEventListener('keydown', onKey);
    };
  }, [open]);

  const item = (m: Model, rec: boolean) => {
    const mc = modeCredits(m, mode);
    return (
      <li key={m.id}>
        <button
          type="button"
          role="option"
          aria-selected={m.id === value?.id}
          className={`model-item ${m.id === value?.id ? 'is-on' : ''} ${mc && !mc.affordable ? 'is-over' : ''}`}
          onClick={() => {
            onChange(m.id);
            setOpen(false);
          }}
        >
          <span className="model-item-top">
            <span className="model-name">{m.name}</span>
            {rec ? <span className="badge">rec</span> : null}
            <span className="mono model-credits">{mc ? `${formatCredits(mc.credits)} cr` : ''}</span>
          </span>
          <span className="model-desc">{m.description}</span>
          {mc && !mc.affordable ? <span className="model-desc warn">Over your balance</span> : null}
        </button>
      </li>
    );
  };

  return (
    <div className="model-picker nodrag" ref={ref}>
      <button type="button" className="model-button" aria-haspopup="listbox" aria-expanded={open} onClick={() => setOpen((v) => !v)}>
        <span className="model-button-name">{value?.name ?? 'Choose a model'}</span>
        {autoNote ? <span className="muted model-auto">{autoNote}</span> : null}
        {mode ? <span className="mono mode-tag">{mode.replace('2', '→')}</span> : null}
        <ChevronDown size={13} aria-hidden />
      </button>
      {open ? (
        <div className="model-pop nowheel" role="listbox" aria-label="Models">
          {recommended.length ? (
            <>
              <div className="pop-label">Recommended</div>
              <ul>{recommended.map((m) => item(m, true))}</ul>
            </>
          ) : null}
          <div className="pop-label">{recommended.length ? 'All models' : 'Models'}</div>
          <ul>{rest.map((m) => item(m, false))}</ul>
        </div>
      ) : null}
    </div>
  );
}
