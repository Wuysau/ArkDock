/**
 * Frontend normalization of native IPC failures.
 *
 * The Rust IPC layer owns error codes, authorization and recovery semantics.
 * This module only reads what the backend actually sent; it never invents a
 * parallel wire contract. Until the structured contract lands, the opaque
 * legacy tokens that already exist in this repository are downgraded into the
 * same closed set, once, here:
 *
 * - `STALE_REQUEST` (`src-tauri/src/ipc/mod.rs`) rejects a superseded target or
 *   preview session and becomes `staleScope`.
 * - `PROTOCOL_VERSION_UNSUPPORTED` (the structured `PreviewProtocolError` code)
 *   becomes `unsupported`, as does an unknown-command legacy reply.
 *
 * Every other failure stays `unknown` with its raw text preserved for display.
 * Unknown failures are never presented as success, as authorized, or as safe to
 * retry automatically, and no caller decides a business branch from message
 * text again. `unauthorized`, `sdkMissing` and `retryable` are declared so the
 * display layer can consume them the moment the backend reports them.
 */
export type AppErrorKind =
  /** The request scope (target lifecycle or preview session) is no longer current. */
  | 'staleScope'
  /** The backend refused this action for the current session or user. */
  | 'unauthorized'
  /** A required toolchain (HarmonyOS SDK) is missing or unverified. */
  | 'sdkMissing'
  /** The backend or protocol does not implement this operation. */
  | 'unsupported'
  /** A transient failure the backend reported as explicitly retryable by the user. */
  | 'retryable'
  /** Not classified. Never treated as success and never retried automatically. */
  | 'unknown';

/** Recovery affordances the backend explicitly allows. Only the IPC layer fills these. */
export type AppErrorRecovery = 'rediscover' | 'reselectProject' | 'retry' | 'configureSdk' | 'reconnect';

export interface AppError {
  kind: AppErrorKind;
  /** Backend code when the backend sent one. Recorded for display and diagnostics only. */
  code: string | null;
  /** Text safe to display. Legacy strings are preserved verbatim. */
  message: string;
  /** Only what the backend explicitly reported; never guessed from message text. */
  recovery: readonly AppErrorRecovery[];
  /** True when only an opaque legacy string was available. */
  legacy: boolean;
}

const brand = Symbol('arkdock.appError');
interface Branded extends AppError { readonly [brand]: true }

/** Codes verified in this repository. The table grows only from real backend values. */
const verifiedCodes: Readonly<Record<string, AppErrorKind>> = {
  STALE_REQUEST: 'staleScope',
  PROTOCOL_VERSION_UNSUPPORTED: 'unsupported',
};

/** The single legacy message pattern kept: a backend without the binary frame commands. */
const unsupportedCommand = /(?:command.*not found|unknown command)/i;
const recoveries = new Set<AppErrorRecovery>(['rediscover', 'reselectProject', 'retry', 'configureSdk', 'reconnect']);

function create(init: { kind: AppErrorKind; code: string | null; message: string; recovery?: readonly AppErrorRecovery[]; legacy?: boolean }): AppError {
  return { kind: init.kind, code: init.code, message: init.message, recovery: init.recovery ?? [], legacy: init.legacy ?? false, [brand]: true } as Branded;
}

export function isAppError(value: unknown): value is AppError {
  return Boolean(value && typeof value === 'object' && (value as Partial<Branded>)[brand] === true);
}

function legacyToken(message: string): string | null {
  for (const token of Object.keys(verifiedCodes)) {
    if (message === token || message.startsWith(`${token}:`) || message.startsWith(`${token} `)) return token;
  }
  return null;
}

function classify(message: string, code: string | null): AppErrorKind {
  const token = code ?? legacyToken(message);
  if (token && verifiedCodes[token]) return verifiedCodes[token];
  return unsupportedCommand.test(message) ? 'unsupported' : 'unknown';
}

function readRecovery(value: unknown): readonly AppErrorRecovery[] {
  if (!Array.isArray(value)) return [];
  return value.filter((item): item is AppErrorRecovery => typeof item === 'string' && recoveries.has(item as AppErrorRecovery));
}

/**
 * Normalizes any thrown value into the closed `AppError` set. Idempotent, so
 * callers may normalize defensively without losing the original classification.
 */
export function normalizeError(reason: unknown): AppError {
  if (isAppError(reason)) return reason;
  if (typeof reason === 'string') return create({ kind: classify(reason, null), code: legacyToken(reason), message: reason, legacy: true });
  if (reason && typeof reason === 'object') {
    const record = reason as { code?: unknown; message?: unknown; recovery?: unknown };
    const code = typeof record.code === 'string' && record.code ? record.code : null;
    const message = typeof record.message === 'string' && record.message ? record.message : code ?? String(reason);
    return create({ kind: classify(message, code), code, message, recovery: readRecovery(record.recovery), legacy: code === null });
  }
  return create({ kind: 'unknown', code: null, message: String(reason) });
}

/** Display text for any failure; readable even when the throw site was not an Error. */
export function describeError(reason: unknown): string {
  return reason == null ? '' : normalizeError(reason).message;
}

/** A superseded scope may be re-issued against the latest confirmed state. */
export function isStaleScope(reason: unknown): boolean {
  return normalizeError(reason).kind === 'staleScope';
}
