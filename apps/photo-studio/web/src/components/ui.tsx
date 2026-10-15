import type { ButtonHTMLAttributes, ReactNode } from 'react';
import { forwardRef, useState } from 'react';
import type { LucideIcon } from 'lucide-react';
import { LoaderCircle, Minus, Plus, TriangleAlert } from 'lucide-react';
import { startOpenArtConnect } from '../api/client';
import { describeError } from '../api/errors';
import { qk, queryClient } from '../api/queries';
import type { NodeError } from '../graph/types';
import { toast } from '../state/ui';

type BtnProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: 'primary' | 'default' | 'ghost' | 'danger';
  size?: 'sm' | 'md';
  icon?: LucideIcon;
  busy?: boolean;
};

export const Button = forwardRef<HTMLButtonElement, BtnProps>(function Button(
  { variant = 'default', size = 'md', icon: Icon, busy, children, className, type, ...rest },
  ref,
) {
  return (
    <button ref={ref} type={type ?? 'button'} className={`btn btn-${variant} btn-${size} ${className ?? ''}`} {...rest}>
      {busy ? <LoaderCircle className="spin" size={14} aria-hidden /> : Icon ? <Icon size={14} aria-hidden /> : null}
      {children}
    </button>
  );
});

type IconBtnProps = ButtonHTMLAttributes<HTMLButtonElement> & { icon: LucideIcon; label: string; active?: boolean; size?: number };

export const IconButton = forwardRef<HTMLButtonElement, IconBtnProps>(function IconButton(
  { icon: Icon, label, active, size = 15, className, type, ...rest },
  ref,
) {
  return (
    <button
      ref={ref}
      type={type ?? 'button'}
      aria-label={label}
      title={label}
      aria-pressed={active === undefined ? undefined : active}
      className={`icon-btn ${active ? 'is-active' : ''} ${className ?? ''}`}
      {...rest}
    >
      <Icon size={size} aria-hidden />
    </button>
  );
});

export function Spinner({ size = 14 }: { size?: number }) {
  return <LoaderCircle className="spin" size={size} aria-hidden />;
}

export function Chips<T extends string>({
  options,
  value,
  onChange,
  label,
  format,
}: {
  options: readonly T[];
  value: T | undefined;
  onChange: (v: T) => void;
  label: string;
  format?: (v: T) => string;
}) {
  return (
    <div className="chips nodrag" role="radiogroup" aria-label={label}>
      {options.map((o) => (
        <button
          key={o}
          type="button"
          role="radio"
          aria-checked={o === value}
          className={`chip ${o === value ? 'is-on' : ''}`}
          onClick={() => onChange(o)}
        >
          {format ? format(o) : o}
        </button>
      ))}
    </div>
  );
}

export function Stepper({ value, min, max, onChange, label }: { value: number; min: number; max: number; onChange: (v: number) => void; label: string }) {
  const v = Number.isFinite(value) ? value : min;
  return (
    <div className="stepper nodrag" role="group" aria-label={label}>
      <button type="button" aria-label={`Decrease ${label}`} disabled={v <= min} onClick={() => onChange(Math.max(min, v - 1))}>
        <Minus size={12} aria-hidden />
      </button>
      <output aria-live="polite">{v}</output>
      <button type="button" aria-label={`Increase ${label}`} disabled={v >= max} onClick={() => onChange(Math.min(max, v + 1))}>
        <Plus size={12} aria-hidden />
      </button>
    </div>
  );
}

export function Toggle({ checked, onChange, label }: { checked: boolean; onChange: (v: boolean) => void; label: string }) {
  return (
    <button type="button" role="switch" aria-checked={checked} aria-label={label} className={`toggle nodrag ${checked ? 'is-on' : ''}`} onClick={() => onChange(!checked)}>
      <span className="toggle-knob" />
    </button>
  );
}

export function Field({ label, children, hint, wide }: { label: string; children: ReactNode; hint?: ReactNode; wide?: boolean }) {
  return (
    <div className={`field ${wide ? 'field-wide' : ''}`}>
      <span className="field-label">{label}</span>
      <div className="field-control">{children}</div>
      {hint ? <span className="field-hint">{hint}</span> : null}
    </div>
  );
}

export function SectionLabel({ children, aside }: { children: ReactNode; aside?: ReactNode }) {
  return (
    <div className="section-label">
      <span>{children}</span>
      {aside ? <span className="section-aside">{aside}</span> : null}
    </div>
  );
}

export async function connectOpenArt(): Promise<void> {
  try {
    const r = await startOpenArtConnect();
    if (r === 'connected') {
      await queryClient.invalidateQueries({ queryKey: qk.me });
      await queryClient.invalidateQueries({ queryKey: qk.models });
      toast('OpenArt connected', 'ok');
    }
  } catch (e) {
    toast(describeError(e).detail, 'error');
  }
}

/** A node or panel error, with the call to action its code calls for. */
export function ErrorNote({ error, onRetry, compact }: { error: NodeError; onRetry?: () => void; compact?: boolean }) {
  const [busy, setBusy] = useState(false);
  const action =
    error.code === 'openart_not_connected' ? 'connect' : error.code === 'openart_reconnect' ? 'reconnect' : onRetry && error.code !== 'claude_not_configured' && error.code !== 'bad_request' ? 'retry' : null;
  return (
    <div className={`error-note ${compact ? 'is-compact' : ''}`} role="alert">
      <TriangleAlert size={14} aria-hidden className="error-icon" />
      <div className="error-body">
        <strong>{error.title ?? 'Error'}</strong>
        <span>{error.message}</span>
        {action ? (
          <div className="error-actions">
            {action === 'retry' ? (
              <Button size="sm" onClick={onRetry} className="nodrag">
                Try again
              </Button>
            ) : (
              <Button
                size="sm"
                variant="primary"
                busy={busy}
                className="nodrag"
                onClick={async () => {
                  setBusy(true);
                  await connectOpenArt();
                  setBusy(false);
                }}
              >
                {action === 'connect' ? 'Connect OpenArt' : 'Reconnect OpenArt'}
              </Button>
            )}
          </div>
        ) : null}
      </div>
    </div>
  );
}

export function errorFrom(e: unknown): NodeError {
  const v = describeError(e);
  return { title: v.title, message: v.detail, code: v.code };
}

export function Kbd({ children }: { children: ReactNode }) {
  return <kbd className="kbd">{children}</kbd>;
}

/** A thumbnail with a neutral well behind it while it loads. */
export function Thumb({ src, alt, ratio, className }: { src: string; alt: string; ratio?: number; className?: string }) {
  return (
    <span className={`thumb ${className ?? ''}`} style={ratio ? { aspectRatio: String(ratio) } : undefined}>
      <img src={src} alt={alt} loading="lazy" decoding="async" draggable={false} />
    </span>
  );
}
