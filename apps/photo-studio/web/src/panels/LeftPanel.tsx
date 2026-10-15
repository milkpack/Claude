import { X } from 'lucide-react';
import { NODE_KINDS } from '../graph/types';
import { TEMPLATES } from '../graph/templates';
import { addNodeAtCenter } from '../graph/actions';
import { loadTemplate } from '../graph/sync';
import { NODE_META } from '../canvas/nodeMeta';
import { NODE_MIME } from '../canvas/dnd';
import { PORTS } from '../graph/ports';
import { isNarrow, useUi } from '../state/ui';
import { IconButton } from '../components/ui';

export function LeftPanel() {
  const open = useUi((s) => s.leftOpen);
  if (!open) return null;
  const closeIfNarrow = () => isNarrow() && useUi.getState().setLeft(false);
  return (
    <aside className="side side-left" aria-label="Nodes and templates">
      <div className="side-head">
        <span>Nodes</span>
        <IconButton icon={X} label="Close panel" className="show-narrow" onClick={() => useUi.getState().setLeft(false)} />
      </div>
      <ul className="palette">
        {NODE_KINDS.map((k) => {
          const m = NODE_META[k];
          const Icon = m.icon;
          const types = [...new Set([...PORTS[k].inputs, ...PORTS[k].outputs].map((p) => p.type))];
          return (
            <li key={k}>
              <button
                type="button"
                className="palette-item"
                draggable
                onDragStart={(e) => {
                  e.dataTransfer.setData(NODE_MIME, k);
                  e.dataTransfer.effectAllowed = 'move';
                }}
                onClick={() => {
                  addNodeAtCenter(k);
                  closeIfNarrow();
                }}
                title={`${m.desc}. Click to add, or drag onto the canvas.`}
              >
                <Icon size={15} aria-hidden className="palette-icon" />
                <span className="palette-text">
                  <span className="palette-name">{m.label}</span>
                  <span className="palette-desc">{m.desc}</span>
                </span>
                <span className="palette-types" aria-hidden>
                  {types.map((t) => (
                    <span key={t} className={`type-dot type-${t}`} />
                  ))}
                </span>
              </button>
            </li>
          );
        })}
      </ul>
      <div className="side-head">
        <span>Templates</span>
      </div>
      <ul className="templates">
        {TEMPLATES.map((t, i) => (
          <li key={t.id}>
            <button
              type="button"
              className={`template ${i === 0 ? 'is-main' : ''}`}
              onClick={() => {
                void loadTemplate(t);
                closeIfNarrow();
              }}
            >
              <span className="template-top">
                <span className="template-name">{t.name}</span>
                <span className="mono template-tag">{t.tag}</span>
              </span>
              <span className="template-desc">{t.description}</span>
            </button>
          </li>
        ))}
      </ul>
      <div className="side-foot">
        <span className="legend">
          <span className="type-dot type-text" /> text <span className="type-dot type-image" /> image <span className="type-dot type-spec" /> spec
        </span>
      </div>
    </aside>
  );
}
