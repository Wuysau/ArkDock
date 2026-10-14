import type { ErrorCategory, RecoveryAction } from './model.ts';

/**
 * Frontend view of the structured IPC error contract (`IpcError`).
 *
 * The backend owns the code, category, retryability and the single recovery
 * action. The frontend only reads them: it groups them for display and maps the
 * recovery action to a concrete hint or an affordance the user may activate. The
 * action itself still goes back to the Rust backend, which re-validates the
 * current scope, trust and toolchain before doing anything.
 *
 * `legacy` marks a value that did not carry the contract fields: a pre-contract
 * string, a diagnostics-only transport payload, or the browser UI fixture. Those
 * keep their raw text for display, are never retryable and offer no recovery
 * action, so a legacy payload can never be presented as a success, as authorized
 * or as safe to retry. No display layer matches message text.
 */
export interface AppError {
  /** Backend `ErrorCode` wire value; `INTERNAL` when the payload carried none. */
  code: string;
  category: ErrorCategory;
  message: string;
  retryable: boolean;
  recovery: RecoveryAction;
  details: string | null;
  /** True when the thrown value did not carry the structured contract fields. */
  legacy: boolean;
}

/** Exhaustive by type: a new contract category fails to compile until it is named here. */
export const errorCategoryLabels: Record<ErrorCategory, string> = {
  validation: '请求无效', state: '状态已变化', trust: '执行信任', environment: '环境不支持',
  storage: '本地存储', discovery: '工程发现', toolchain: '原生工具链', transport: '预览通道',
  selection: '选择上下文', agent: 'Agent 投递', internal: '未识别',
};

/** Exhaustive by type: a new contract recovery action fails to compile until it is named here. */
export const recoveryLabels: Record<RecoveryAction, string> = {
  none: '', correctRequest: '请修正请求内容后重试', retryRequest: '后端允许重试该操作',
  awaitCurrentOperation: '请等待当前原生操作结束', refreshSnapshot: '正在重新同步后端状态',
  reselectTarget: '请重新连接目标窗口', reselectProject: '请重新选择当前工程',
  confirmTrust: '请在工程信任中确认后继续', configureSdk: '请在原生工具链中指定 SDK 目录',
  restartPreview: '可重新启动原生预览', rediscover: '可重新识别当前工程',
  readDiagnostics: '请查看运行日志',
};

const recoveryActions = new Set<string>(Object.keys(recoveryLabels));
const errorCategories = new Set<string>(Object.keys(errorCategoryLabels));

/** Codes whose contract meaning the display layer acts on. Everything else is data. */
const staleScopeCodes = new Set<string>(['STALE_REQUEST', 'STALE_TARGET', 'STALE_PROJECT', 'STALE_SESSION']);
const unsupportedCodes = new Set<string>(['PROTOCOL_VERSION_UNSUPPORTED', 'UNSUPPORTED_PLATFORM', 'UNSUPPORTED_ENVIRONMENT']);

const brand = Symbol('arkdock.appError');
interface Branded extends AppError { readonly [brand]: true }

/** Same token shape the backend uses, so prose that merely contains a colon is not a code. */
function codeToken(value: unknown): string | null {
  if (typeof value !== 'string' || value.length === 0 || value.length > 64) return null;
  for (const character of value) {
    const allowed = (character >= 'A' && character <= 'Z') || (character >= '0' && character <= '9') || character === '_';
    if (!allowed) return null;
  }
  return value;
}

function legacy(message: string, token: string | null = null): AppError {
  // A legacy payload keeps its text but carries no category, retryability or
  // recovery: those are contract fields, and inventing them would be a second,
  // incompatible protocol. The leading code token is preserved for diagnostics.
  return { code: token ?? 'INTERNAL', category: 'internal', message, retryable: false, recovery: 'none', details: null, legacy: true, [brand]: true } as Branded;
}

/** A pre-contract string may lead with a contract code token; the match is confined here. */
function legacyCode(message: string): string | null {
  const head = message.split(':', 1)[0]?.trim() ?? '';
  return codeToken(head);
}

export function isAppError(value: unknown): value is AppError {
  return Boolean(value && typeof value === 'object' && (value as Partial<Branded>)[brand] === true);
}

interface RawFailure { code?: unknown; message?: unknown; category?: unknown; retryable?: unknown; recovery?: unknown; details?: unknown }

/**
 * Normalizes any thrown value into the contract shape. Idempotent, so callers may
 * normalize defensively: a value that already carries the contract is returned
 * unchanged, and one that does not is downgraded instead of being guessed at.
 */
export function normalizeError(reason: unknown): AppError {
  if (isAppError(reason)) return reason;
  if (typeof reason === 'string') return legacy(reason, legacyCode(reason));
  if (reason && typeof reason === 'object') {
    const raw = reason as RawFailure;
    const code = codeToken(raw.code);
    const message = typeof raw.message === 'string' && raw.message ? raw.message : code ?? String(reason);
    if (!errorCategories.has(String(raw.category)) || !recoveryActions.has(String(raw.recovery)) || typeof raw.retryable !== 'boolean') {
      return legacy(message, code ?? legacyCode(message));
    }
    return {
      code: code ?? 'INTERNAL', category: raw.category as ErrorCategory, message, retryable: raw.retryable,
      recovery: raw.recovery as RecoveryAction, details: typeof raw.details === 'string' && raw.details ? raw.details : null,
      legacy: false, [brand]: true,
    } as Branded;
  }
  return legacy(String(reason));
}

/** Display text for any failure; the sanitized backend details are appended when present. */
export function describeError(reason: unknown): string {
  if (reason == null) return '';
  const error = normalizeError(reason);
  return error.details ? `${error.message}（${error.details}）` : error.message;
}

/** The backend reported that the request's target or session scope is no longer current. */
export function isStaleScope(reason: unknown): boolean {
  return staleScopeCodes.has(normalizeError(reason).code);
}

/** The backend does not implement this operation, platform or protocol version. */
export function isUnsupportedOperation(reason: unknown): boolean {
  return unsupportedCodes.has(normalizeError(reason).code);
}

/** Only an explicit backend `retryable` flag, never a legacy or unclassified value. */
export function isRetryable(reason: unknown): boolean {
  const error = normalizeError(reason);
  return !error.legacy && error.retryable;
}

export function errorCategoryLabel(error: AppError): string {
  return errorCategoryLabels[error.category];
}
