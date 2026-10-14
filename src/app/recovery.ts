import type { AppError } from '../ipc/errors.ts';
import { isRetryable, recoveryLabels } from '../ipc/errors.ts';

/**
 * An affordance this UI can actually perform for the backend's recovery action.
 * Every intent still goes through the Rust backend, which re-validates the scope,
 * trust and toolchain before doing anything.
 */
export type RecoveryIntent = 'refreshSnapshot' | 'rediscover' | 'openProjects' | 'openTrust' | 'configureSdk' | 'restartPreview' | 'openDiagnostics';

export const recoveryIntentLabels: Record<RecoveryIntent, string> = {
  refreshSnapshot: '重新同步', rediscover: '重新识别', openProjects: '选择工程', openTrust: '确认工程信任',
  configureSdk: '指定 SDK 目录', restartPreview: '重新启动预览', openDiagnostics: '查看运行日志',
};

/**
 * Exhaustive by type over the contract vocabulary. `null` means the hint is the
 * only affordance: a correct-request, retry, reselect-target or
 * await-current-operation recovery is a user action on the original control, and
 * the frontend never re-issues it automatically. A legacy payload carries no
 * action at all.
 */
export function recoveryIntent(error: AppError): RecoveryIntent | null {
  if (error.legacy) return null;
  switch (error.recovery) {
    case 'refreshSnapshot': return 'refreshSnapshot';
    case 'rediscover': return 'rediscover';
    case 'reselectProject': return 'openProjects';
    case 'confirmTrust': return 'openTrust';
    case 'configureSdk': return 'configureSdk';
    case 'restartPreview': return 'restartPreview';
    case 'readDiagnostics': return 'openDiagnostics';
    case 'none': case 'correctRequest': case 'retryRequest': case 'awaitCurrentOperation': case 'reselectTarget': return null;
  }
}

export interface RecoveryOffer { intent: RecoveryIntent; label: string }
export interface RecoveryAvailability {
  connected: boolean;
  target: boolean;
  project: boolean;
  trusted: boolean;
  refreshAvailable: boolean;
}

/** What this snapshot can actually offer, so a button never promises a dead action. */
function available(intent: RecoveryIntent, state: RecoveryAvailability): boolean {
  switch (intent) {
    case 'refreshSnapshot': return state.connected;
    case 'rediscover': return state.connected && state.target;
    case 'openProjects': return state.connected;
    case 'openTrust': return state.project && !state.trusted;
    case 'configureSdk': return state.connected;
    case 'restartPreview': return state.refreshAvailable;
    case 'openDiagnostics': return true;
  }
}

export function recoveryOffer(error: AppError | null, state: RecoveryAvailability): RecoveryOffer | null {
  if (!error) return null;
  const intent = recoveryIntent(error);
  if (!intent || !available(intent, state)) return null;
  return { intent, label: recoveryIntentLabels[intent] };
}

/** The concrete hint for the recovery action; empty when the contract named none. */
export function recoveryText(error: AppError | null): string {
  if (!error) return '';
  const hint = error.legacy ? '' : recoveryLabels[error.recovery];
  if (!hint) return '';
  return isRetryable(error) ? `${hint}（后端标记为可重试）` : hint;
}
