import { useEffect, useRef } from 'react';
import { X } from 'lucide-react';
import { useAssets } from '../api/queries';
import type { Asset } from '../api/types';
import { IconButton, Spinner } from '../components/ui';

/** A compact picker over the asset library, used inside Reference nodes and app mode. */
export function LibraryPicker({ onPick, onClose }: { onPick: (a: Asset) => void; onClose: () => void }) {
  const q = useAssets();
  const ref = useRef<HTMLDivElement>(null);
  const assets = q.data?.pages.flatMap((p) => p.assets) ?? [];
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && onClose();
    const onDown = (e: PointerEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose();
    };
    window.addEventListener('keydown', onKey);
    window.addEventListener('pointerdown', onDown, true);
    return () => {
      window.removeEventListener('keydown', onKey);
      window.removeEventListener('pointerdown', onDown, true);
    };
  }, [onClose]);
  return (
    <div className="picker nodrag nowheel" ref={ref} role="dialog" aria-label="Pick from the library">
      <div className="picker-head">
        <span>Library</span>
        <IconButton icon={X} label="Close" size={13} onClick={onClose} />
      </div>
      {q.isLoading ? (
        <div className="picker-empty">
          <Spinner />
        </div>
      ) : assets.length === 0 ? (
        <div className="picker-empty muted">The library is empty. Upload a photo first.</div>
      ) : (
        <div className="picker-grid">
          {assets.map((a) => (
            <button key={a.id} type="button" className="picker-item" onClick={() => onPick(a)} title={a.prompt ?? (a.kind === 'upload' ? 'Upload' : 'Generation')}>
              <img src={a.thumb} alt="" loading="lazy" draggable={false} />
            </button>
          ))}
          {q.hasNextPage ? (
            <button type="button" className="picker-more" onClick={() => void q.fetchNextPage()}>
              {q.isFetchingNextPage ? <Spinner /> : 'More'}
            </button>
          ) : null}
        </div>
      )}
    </div>
  );
}
