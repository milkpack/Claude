import { ApiError } from './client';

export type ErrorAction = 'connect' | 'reconnect' | 'login' | 'retry' | 'none';

export type ErrorView = { title: string; detail: string; action: ErrorAction; code?: string };

/** One place that turns API failures into what the user sees, branching on the error code. */
export function describeError(err: unknown): ErrorView {
  if (err instanceof ApiError) {
    switch (err.code) {
      case 'openart_not_connected':
        return {
          title: 'OpenArt is not connected',
          detail: 'Generation runs on your OpenArt account. Connect it once and this step can run.',
          action: 'connect',
          code: err.code,
        };
      case 'openart_reconnect':
        return {
          title: 'OpenArt needs to be reconnected',
          detail: 'The OpenArt session expired or was revoked. Reconnect to continue.',
          action: 'reconnect',
          code: err.code,
        };
      case 'claude_not_configured':
        return {
          title: 'Claude is not configured',
          detail:
            'The Director and Review nodes need Claude. Set ANTHROPIC_API_KEY for the server and restart it; everything else keeps working.',
          action: 'none',
          code: err.code,
        };
      case 'unauthenticated':
        return { title: 'Signed out', detail: 'Your session ended. Sign in again.', action: 'login', code: err.code };
      case 'rate_limited':
        return {
          title: 'Too many requests',
          detail: err.message || 'Wait a minute and try again.',
          action: 'retry',
          code: err.code,
        };
      case 'upstream':
        return {
          title: 'The provider returned an error',
          detail: err.message,
          action: 'retry',
          code: err.code,
        };
      case 'not_found':
        return { title: 'Not found', detail: err.message, action: 'none', code: err.code };
      case 'bad_request':
        return { title: 'Request rejected', detail: err.message, action: 'none', code: err.code };
      default:
        return {
          title: err.status === 0 ? 'Server unreachable' : 'Something went wrong',
          detail: err.message,
          action: 'retry',
        };
    }
  }
  if (err instanceof Error) return { title: 'Something went wrong', detail: err.message, action: 'retry' };
  return { title: 'Something went wrong', detail: String(err), action: 'retry' };
}

export function errorCode(err: unknown): string | undefined {
  return err instanceof ApiError ? err.code : undefined;
}
