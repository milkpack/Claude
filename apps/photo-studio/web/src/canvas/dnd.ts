import type { Asset } from '../api/types';
import type { NodeKind } from '../graph/types';
import { isKind } from '../graph/types';

export const ASSET_MIME = 'application/x-photo-studio-asset';
export const NODE_MIME = 'application/x-photo-studio-node';

export function readDraggedAsset(dt: DataTransfer): Asset | null {
  const raw = dt.getData(ASSET_MIME);
  if (!raw) return null;
  try {
    const v = JSON.parse(raw) as Partial<Asset>;
    if (typeof v.id === 'string' && typeof v.file === 'string' && typeof v.thumb === 'string') return v as Asset;
  } catch {
    // Not ours (or malformed): ignore the drop.
  }
  return null;
}

export function readDraggedKind(dt: DataTransfer): NodeKind | null {
  const k = dt.getData(NODE_MIME);
  return isKind(k) ? k : null;
}
