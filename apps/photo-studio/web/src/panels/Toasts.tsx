import { X } from 'lucide-react';
import { useUi } from '../state/ui';

export function Toasts() {
  const toasts = useUi((s) => s.toasts);
  return (
    <div className="toasts" aria-live="polite" role="status">
      {toasts.map((t) => (
        <div key={t.id} className={`toast toast-${t.tone}`}>
          <span>{t.text}</span>
          <button type="button" aria-label="Dismiss" onClick={() => useUi.getState().dismissToast(t.id)}>
            <X size={12} aria-hidden />
          </button>
        </div>
      ))}
    </div>
  );
}
