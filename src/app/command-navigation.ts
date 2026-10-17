import type { AppSnapshot, CommandArgs } from '../ipc/model.ts';
import { responseStillCurrent } from '../request-state.ts';
import type { Panel } from './navigation.ts';

interface CommandNavigation {
  snapshot: () => AppSnapshot | null;
  receive: (snapshot: AppSnapshot) => void;
  revision: () => number;
  open: (panel: Panel | null) => void;
}

/** Snapshot commands share one navigation policy across every UI entry point. */
export async function runSnapshotCommand(
  command: keyof CommandArgs,
  request: () => Promise<AppSnapshot>,
  navigation: CommandNavigation,
  registrationId?: string,
): Promise<boolean> {
  const revision = navigation.revision();
  // Native moving can finish on the same target, so a target-identity change
  // cannot be the only event that dismisses a previously open setup sheet.
  if (command === 'begin_drag') navigation.open(null);
  const result = await request();
  navigation.receive(result);
  const current = navigation.snapshot();
  if (revision !== navigation.revision() || !responseStillCurrent(result, current)) return false;
  if (command === 'context_select_project') {
    if (!registrationId || current?.contextSource?.boundRegistrationId !== registrationId) return false;
    // Both the workspace strip and the MCP sheet use this command. Advance
    // from setup only after the backend confirms the requested registration.
    navigation.open(!current.project ? 'projects' : !current.trusted ? 'trust' : null);
  }
  return true;
}
