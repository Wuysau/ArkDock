import { useLayoutEffect, useState } from 'react';
import type { AppSnapshot, MotionSettings } from './ipc/model.ts';
import { allowsDockRipple, createDockRippleController, targetLifecycleKey } from './dock-ripple.ts';
import type { DockRippleState } from './dock-ripple.ts';

export function DockRipple({ snapshot, motion }: { snapshot: AppSnapshot | null; motion: MotionSettings }) {
  const [controller] = useState(createDockRippleController);
  const [ripple, setRipple] = useState<DockRippleState | null>(null);
  const { reduced, intensity } = motion;
  useLayoutEffect(() => { setRipple(controller.update(snapshot, { reduced, intensity })); }, [controller, snapshot, reduced, intensity]);
  if (!ripple || !allowsDockRipple(motion) || ripple.targetKey !== targetLifecycleKey(snapshot)
    || ripple.bindingId !== snapshot?.dockConfirmation?.bindingId || !['expanded', 'expanding'].includes(snapshot.display.mode)) return null;
  return <div key={ripple.bindingId} className="dock-ripple" data-side={ripple.side} data-binding-id={ripple.bindingId} aria-hidden="true">
    {Array.from({ length: ripple.waves }, (_, index) => <span key={index} className="dock-ripple-wave" style={{ animationDelay: `${index * 100}ms` }} onAnimationEnd={() => {
      if (index === ripple.waves - 1) setRipple(controller.finish(ripple.bindingId));
    }} />)}
  </div>;
}
