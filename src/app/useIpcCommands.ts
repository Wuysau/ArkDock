import { useCallback, useRef, useState } from 'react';
import type { MutableRefObject } from 'react';
import type { AppSnapshot, CommandArgs } from '../ipc/model.ts';
import type { UiBridge } from '../ipc/client.ts';
import { isStaleScope } from '../ipc/errors.ts';
import { currentScope, responseStillCurrent, sameTarget, scopeStillCurrent } from '../request-state.ts';
import { runSnapshotCommand } from './command-navigation.ts';
import type { DesktopSession } from './useDesktopSession.ts';
import type { Notices } from './notices.ts';
import type { Panel } from './navigation.ts';

export interface IpcCommands {
  /** Command name while a request is in flight; null when idle. */
  pending: string | null;
  cancelling: boolean;
  /** Synchronous in-flight guard, read by input dispatch in the same tick. */
  pendingRef: MutableRefObject<boolean>;
  /** True while an association cancellation owns the binding. */
  cancelRequested: MutableRefObject<boolean>;
  /** Last dispatched command, used to suppress duplicate lifecycle feedback. */
  activeCommand: MutableRefObject<string | null>;
  act: <K extends keyof CommandArgs>(command: K, args: Omit<CommandArgs[K], 'scope'>) => Promise<boolean>;
  cancelAssociation: () => Promise<void>;
}

/**
 * Request lifecycle for every snapshot command: one request at a time, the
 * current scope attached, late replies rejected, and Stop/Restart/project
 * switches invalidating anything that resolves afterwards. Failure text is
 * normalized by `notices`, and only a superseded scope is ever re-issued.
 */
export function useIpcCommands({ bridge, session, notices, navigationRevision, openPanel }: {
  bridge: UiBridge;
  session: DesktopSession;
  notices: Notices;
  navigationRevision: MutableRefObject<number>;
  openPanel: (panel: Panel | null) => void;
}): IpcCommands {
  const { stateRef, receive } = session;
  // Destructured so the request callbacks below stay stable across renders.
  const { report, notify, clear, clearError } = notices;
  const [pending, setPending] = useState<string | null>(null);
  const [cancelling, setCancelling] = useState(false);
  const pendingRef = useRef(false);
  const cancelRequested = useRef(false);
  const activeCommand = useRef<string | null>(null);

  const act = useCallback(async <K extends keyof CommandArgs>(command: K, args: Omit<CommandArgs[K], 'scope'>): Promise<boolean> => {
    if (pendingRef.current) return false;
    pendingRef.current = true;
    activeCommand.current = command;
    setPending(command);
    clear();
    const scope = currentScope(stateRef.current);
    try {
      const registrationId = command === 'context_select_project' && 'registrationId' in args
        && typeof args.registrationId === 'string' ? args.registrationId : undefined;
      return await runSnapshotCommand(command, () => bridge.request(command, args, scope), {
        snapshot: () => stateRef.current, receive, revision: () => navigationRevision.current, open: openPanel,
      }, registrationId);
    } catch (reason) {
      if (scopeStillCurrent(stateRef.current, scope)) report(reason);
      return false;
    } finally {
      pendingRef.current = false;
      activeCommand.current = null;
      setPending(null);
    }
  }, [bridge, receive, stateRef, clear, report, navigationRevision, openPanel]);

  const cancelAssociation = useCallback(async () => {
    if (cancelRequested.current) return;
    cancelRequested.current = true;
    setCancelling(true);
    openPanel(null);
    clearError();
    const target = stateRef.current?.target ?? null;
    const scope = currentScope(stateRef.current);
    try {
      let result: AppSnapshot;
      try {
        result = await bridge.request('detach', {}, scope);
      } catch (reason) {
        // A completed discovery may create a session before the queued detach.
        // Only this exact target lifecycle is re-issued, and only for a scope the
        // backend itself reported as superseded.
        if (!isStaleScope(reason)) throw reason;
        const latest = await bridge.request('snapshot', {});
        receive(latest);
        if (!sameTarget(target, latest.target)) return;
        result = await bridge.request('detach', {}, currentScope(latest));
      }
      receive(result);
      if (responseStillCurrent(result, stateRef.current)) notify('关联已取消，窗口已解除绑定。');
    } catch (reason) {
      if (sameTarget(target, stateRef.current?.target)) report(reason);
    } finally {
      cancelRequested.current = false;
      setCancelling(false);
    }
  }, [bridge, receive, stateRef, openPanel, clearError, notify, report]);

  return { pending, cancelling, pendingRef, cancelRequested, activeCommand, act, cancelAssociation };
}
