import type { DragEvent } from 'react';
import { useRef, useState } from 'react';
import type { NodeProps } from '@xyflow/react';
import { ImagePlus, Library, Upload } from 'lucide-react';
import type { AppNode, ReferenceData, RefRole } from '../../graph/types';
import { REF_ROLES } from '../../graph/types';
import { useGraph } from '../../graph/store';
import { uploadIntoReference } from '../../graph/actions';
import { useUi } from '../../state/ui';
import { Button, Spinner } from '../../components/ui';
import { LibraryPicker } from '../LibraryPicker';
import { NodeShell } from './NodeShell';
import { ASSET_MIME, readDraggedAsset } from '../dnd';

export function RoleSelect({ value, onChange, label = 'Role' }: { value: RefRole; onChange: (r: RefRole) => void; label?: string }) {
  return (
    <label className="role-select nodrag">
      <span className="sr-only">{label}</span>
      <select value={value} onChange={(e) => onChange(e.target.value as RefRole)} aria-label={label}>
        {REF_ROLES.map((r) => (
          <option key={r.id} value={r.id}>
            {r.label}
          </option>
        ))}
      </select>
    </label>
  );
}

export function ReferenceNode({ id, data, selected }: NodeProps<Extract<AppNode, { type: 'reference' }>>) {
  const d = data as ReferenceData;
  const fileRef = useRef<HTMLInputElement>(null);
  const [picking, setPicking] = useState(false);
  const [over, setOver] = useState(false);
  const src = d.previewUrl ?? d.asset?.thumb;

  const onDrop = (e: DragEvent) => {
    const asset = readDraggedAsset(e.dataTransfer);
    const file = Array.from(e.dataTransfer.files).find((f) => f.type.startsWith('image/'));
    if (!asset && !file) return;
    e.preventDefault();
    e.stopPropagation();
    setOver(false);
    if (asset) useGraph.getState().updateData(id, { asset, error: undefined });
    else if (file) void uploadIntoReference(id, file);
  };

  return (
    <NodeShell id={id} kind="reference" selected={selected} title={d.title} exposed={d.exposed} error={d.error} done={Boolean(d.asset)}>
      <div
        className={`ref-drop ${over ? 'is-over' : ''} ${src ? 'has-image' : ''}`}
        onDragOver={(e) => {
          if (e.dataTransfer.types.includes('Files') || e.dataTransfer.types.includes(ASSET_MIME)) {
            e.preventDefault();
            e.stopPropagation();
            setOver(true);
          }
        }}
        onDragLeave={() => setOver(false)}
        onDrop={onDrop}
      >
        {src ? (
          <button
            type="button"
            className="ref-image nodrag"
            aria-label="Open full size"
            onClick={() => d.asset && useUi.getState().openViewer({ assets: [d.asset], index: 0, fromNode: id })}
          >
            <img src={src} alt={d.title ?? 'Reference'} draggable={false} />
            {d.uploading ? (
              <span className="ref-uploading">
                <Spinner /> Uploading
              </span>
            ) : null}
            {d.asset ? (
              <span className="ref-dims mono">
                {d.asset.width}×{d.asset.height}
              </span>
            ) : null}
          </button>
        ) : (
          <div className="ref-empty">
            <ImagePlus size={20} aria-hidden className="muted" />
            <span>Drop a photo, paste, or</span>
          </div>
        )}
      </div>
      <div className="ref-actions">
        <RoleSelect value={d.role} onChange={(role) => useGraph.getState().updateData(id, { role })} />
        <span className="spacer" />
        <Button size="sm" variant="ghost" icon={Upload} className="nodrag" onClick={() => fileRef.current?.click()}>
          {src ? 'Replace' : 'Upload'}
        </Button>
        <Button size="sm" variant="ghost" icon={Library} className="nodrag" onClick={() => setPicking((v) => !v)} aria-expanded={picking}>
          Library
        </Button>
      </div>
      <input
        ref={fileRef}
        type="file"
        accept="image/png,image/jpeg,image/webp"
        hidden
        onChange={(e) => {
          const f = e.target.files?.[0];
          if (f) void uploadIntoReference(id, f);
          e.target.value = '';
        }}
      />
      {picking ? (
        <LibraryPicker
          onClose={() => setPicking(false)}
          onPick={(asset) => {
            useGraph.getState().updateData(id, { asset, error: undefined });
            setPicking(false);
          }}
        />
      ) : null}
    </NodeShell>
  );
}
