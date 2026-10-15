import { useEffect, useState } from 'react';

/** JSON with sorted object keys, so equal values give equal strings (cache keys, query keys). */
export function stableStringify(value: unknown): string {
  return JSON.stringify(sortKeys(value));
}

function sortKeys(v: unknown): unknown {
  if (Array.isArray(v)) return v.map(sortKeys);
  if (v && typeof v === 'object') {
    const out: Record<string, unknown> = {};
    for (const k of Object.keys(v as Record<string, unknown>).sort()) {
      const x = (v as Record<string, unknown>)[k];
      if (x !== undefined) out[k] = sortKeys(x);
    }
    return out;
  }
  return v;
}

/** Short, fast, non-cryptographic hash (FNV-1a), for cache keys. */
export function hash(s: string): string {
  let h = 0x811c9dc5;
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return (h >>> 0).toString(36);
}

export function uid(prefix = ''): string {
  const raw =
    typeof crypto !== 'undefined' && 'randomUUID' in crypto
      ? crypto.randomUUID().replace(/-/g, '')
      : Math.random().toString(36).slice(2) + Date.now().toString(36);
  return prefix + raw.slice(0, 10);
}

export function useDebounced<T>(value: T, ms: number): T {
  const [v, setV] = useState(value);
  useEffect(() => {
    const t = window.setTimeout(() => setV(value), ms);
    return () => window.clearTimeout(t);
  }, [value, ms]);
  return v;
}

const nf = new Intl.NumberFormat('en-US');
export function formatCredits(n: number | undefined | null): string {
  if (n === undefined || n === null || !Number.isFinite(n)) return '—';
  return nf.format(Math.round(n));
}

export function formatElapsed(ms: number): string {
  if (!Number.isFinite(ms) || ms < 0) return '0:00';
  const s = Math.floor(ms / 1000);
  const m = Math.floor(s / 60);
  const r = s % 60;
  return `${m}:${r.toString().padStart(2, '0')}`;
}

export function formatDate(ts: number): string {
  if (!Number.isFinite(ts)) return '—';
  const ms = ts < 1e12 ? ts * 1000 : ts; // seconds or milliseconds
  return new Date(ms).toLocaleString(undefined, {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  });
}

export function timeMs(ts: number): number {
  return ts < 1e12 ? ts * 1000 : ts;
}

export function humanizeKey(key: string): string {
  const special: Record<string, string> = {
    imageCount: 'Count',
    aspectRatio: 'Aspect',
    resolutionTier: 'Resolution',
    outputFormat: 'Format',
    outputCompression: 'Compression',
    lockAspectRatio: 'Lock aspect',
    thinkingMode: 'Thinking',
    resultType: 'Result',
    seriesAmount: 'Frames',
    customWidth: 'Width',
    customHeight: 'Height',
  };
  const s = special[key];
  if (s) return s;
  const spaced = key.replace(/([a-z0-9])([A-Z])/g, '$1 $2').replace(/[_-]+/g, ' ');
  return spaced.charAt(0).toUpperCase() + spaced.slice(1).toLowerCase();
}

export async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

export function downloadUrl(url: string, filename: string): void {
  const a = document.createElement('a');
  a.href = url;
  a.download = filename;
  a.rel = 'noopener';
  document.body.appendChild(a);
  a.click();
  a.remove();
}

export function assetFilename(a: { id: string; file: string }): string {
  const m = /\.(png|jpe?g|webp)(?:$|\?)/i.exec(a.file);
  return `photo-studio-${a.id}.${m?.[1]?.toLowerCase() ?? 'png'}`;
}

/** Per-viewer conveniences only (theme, last workflow, panel state); storage may be blocked. */
export const prefs = {
  get(key: string): string | null {
    try {
      return window.localStorage.getItem(`ps.${key}`);
    } catch {
      return null;
    }
  },
  set(key: string, value: string | null): void {
    try {
      if (value === null) window.localStorage.removeItem(`ps.${key}`);
      else window.localStorage.setItem(`ps.${key}`, value);
    } catch {
      // Storage blocked (private window, preview): the preference simply isn't remembered.
    }
  },
};

export function isEditableTarget(t: EventTarget | null): boolean {
  if (!(t instanceof HTMLElement)) return false;
  if (t.isContentEditable) return true;
  const tag = t.tagName;
  if (tag === 'TEXTAREA' || tag === 'SELECT') return true;
  if (tag === 'INPUT') {
    const type = (t as HTMLInputElement).type;
    return !['button', 'checkbox', 'radio', 'range', 'submit', 'reset', 'file'].includes(type);
  }
  return false;
}

export const isMac = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
export const modKey = isMac ? '⌘' : 'Ctrl';

export const ACCEPTED_IMAGE_TYPES = ['image/png', 'image/jpeg', 'image/webp'];
export const MAX_UPLOAD_BYTES = 30 * 1024 * 1024;

export function validateUpload(file: File): string | null {
  if (!ACCEPTED_IMAGE_TYPES.includes(file.type)) return `${file.name || 'File'}: only PNG, JPEG and WebP images can be uploaded.`;
  if (file.size > MAX_UPLOAD_BYTES) return `${file.name || 'File'} is larger than 30 MB.`;
  return null;
}

export function clamp(n: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, n));
}
