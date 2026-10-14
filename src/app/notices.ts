import { useCallback, useState } from 'react';
import type { AppError } from '../ipc/errors.ts';
import { normalizeError } from '../ipc/errors.ts';

/**
 * The single notice surface. `report` carries a native failure (normalized once,
 * here) and `notify` carries a local success/info message; they are independent,
 * exactly as the previous inline state was, so a failure never hides behind a
 * stale success and an unknown failure never reads as success.
 */
export interface Notices {
  error: AppError | null;
  feedback: string | null;
  report: (reason: unknown) => void;
  notify: (message: string | null) => void;
  clear: () => void;
  clearError: () => void;
}

export function useNotices(): Notices {
  const [error, setError] = useState<AppError | null>(null);
  const [feedback, setFeedback] = useState<string | null>(null);
  const report = useCallback((reason: unknown) => { setError(normalizeError(reason)); }, []);
  const notify = useCallback((message: string | null) => { setFeedback(message); }, []);
  const clear = useCallback(() => { setError(null); setFeedback(null); }, []);
  const clearError = useCallback(() => { setError(null); }, []);
  return { error, feedback, report, notify, clear, clearError };
}
