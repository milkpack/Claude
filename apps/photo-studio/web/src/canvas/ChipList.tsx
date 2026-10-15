import { useState } from 'react';
import { X } from 'lucide-react';

/** An editable list of short phrases ("change only" / "keep exactly", preserve lists). */
export function ChipList({
  items,
  onChange,
  placeholder,
  label,
  ghost,
}: {
  items: string[];
  onChange: (items: string[]) => void;
  placeholder: string;
  label: string;
  /** Suggestions shown faintly when the list is empty (e.g. the Director's preserve list). */
  ghost?: string[];
}) {
  const [draft, setDraft] = useState('');
  const add = () => {
    const t = draft.trim();
    if (!t) return;
    onChange([...items, t]);
    setDraft('');
  };
  return (
    <div className="chiplist nodrag" role="group" aria-label={label}>
      {items.map((it, i) => (
        <span className="tag" key={`${it}-${i}`}>
          {it}
          <button type="button" aria-label={`Remove ${it}`} onClick={() => onChange(items.filter((_, j) => j !== i))}>
            <X size={10} aria-hidden />
          </button>
        </span>
      ))}
      {!items.length && ghost?.length
        ? ghost.map((g) => (
            <span className="tag is-ghost" key={g} title="From the Director">
              {g}
            </span>
          ))
        : null}
      <input
        className="chiplist-input"
        value={draft}
        placeholder={placeholder}
        aria-label={`Add to ${label}`}
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter') {
            e.preventDefault();
            add();
          } else if (e.key === 'Backspace' && !draft && items.length) onChange(items.slice(0, -1));
        }}
        onBlur={add}
      />
    </div>
  );
}
