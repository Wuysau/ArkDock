import type { AppSnapshot, DockSide, MotionSettings } from './ipc/model.ts';

type RippleSnapshot = Pick<AppSnapshot, 'revision' | 'target' | 'dockConfirmation' | 'display'>;
export interface DockRippleState {
  bindingId: string;
  targetKey: string;
  side: DockSide;
  waves: 2 | 3;
}

export function targetLifecycleKey(snapshot: Pick<AppSnapshot, 'target'> | null): string {
  const target = snapshot?.target;
  return target ? JSON.stringify([target.id, target.process.pid, target.process.startedAt]) : 'unbound';
}

export function allowsDockRipple(settings: MotionSettings): boolean {
  return !settings.reduced && (settings.intensity === 'standard' || settings.intensity === 'enhanced');
}

/** Only a new accepted native binding is a trigger; finishing waves changes decoration alone. */
export function createDockRippleController() {
  let revision = -1;
  let snapshot: RippleSnapshot | null = null;
  const consumedBindings = new Set<string>();
  let active: DockRippleState | null = null;
  return {
    update(incoming: RippleSnapshot | null, settings: MotionSettings): DockRippleState | null {
      if (incoming && incoming.revision > revision) { snapshot = incoming; revision = incoming.revision; }
      const targetKey = targetLifecycleKey(snapshot);
      const confirmation = snapshot?.dockConfirmation;
      const displayVisible = snapshot?.display.mode === 'expanded' || snapshot?.display.mode === 'expanding';
      const eligible = Boolean(snapshot?.target && confirmation && displayVisible && allowsDockRipple(settings));
      if (!eligible || active?.targetKey !== targetKey || active?.bindingId !== confirmation?.bindingId) active = null;
      if (confirmation && !consumedBindings.has(confirmation.bindingId)) {
        consumedBindings.add(confirmation.bindingId);
        if (eligible) active = { bindingId: confirmation.bindingId, targetKey, side: confirmation.side, waves: settings.intensity === 'enhanced' ? 3 : 2 };
      }
      return active;
    },
    finish(bindingId: string): DockRippleState | null {
      if (active?.bindingId === bindingId) active = null;
      return active;
    },
  };
}
