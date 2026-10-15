import type { FormEvent } from 'react';
import { useState } from 'react';
import { api } from '../api/client';
import { describeError } from '../api/errors';
import { qk, queryClient } from '../api/queries';
import { Button } from '../components/ui';

export function Login() {
  const [password, setPassword] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!password) return;
    setBusy(true);
    setError(null);
    try {
      const me = await api.login(password);
      queryClient.setQueryData(qk.me, me);
    } catch (err) {
      const v = describeError(err);
      setError(v.code === 'unauthenticated' ? 'That password is not right.' : v.detail);
      setPassword('');
    } finally {
      setBusy(false);
    }
  };
  return (
    <main className="login">
      <form className="login-card" onSubmit={submit}>
        <span className="brand">
          <span className="brand-mark" aria-hidden />
          <span className="brand-name">Photo Studio</span>
        </span>
        <p className="muted">A private studio. Sign in with the owner password set on the server.</p>
        <label className="login-field">
          <span>Password</span>
          <input
            type="password"
            autoComplete="current-password"
            autoFocus
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            aria-invalid={Boolean(error)}
            aria-describedby={error ? 'login-error' : undefined}
          />
        </label>
        {error ? (
          <p className="login-error" id="login-error" role="alert">
            {error}
          </p>
        ) : null}
        <Button type="submit" variant="primary" busy={busy} disabled={!password}>
          Sign in
        </Button>
      </form>
    </main>
  );
}
