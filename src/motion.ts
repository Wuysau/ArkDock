export type MotionPreference = 'system' | 'reduce';
export function readMotion(): MotionPreference {
  try { return localStorage.getItem('arkdock:motion') === 'reduce' ? 'reduce' : 'system'; }
  catch { return 'system'; }
}
export function applyMotion(preference: MotionPreference): void {
  document.documentElement.dataset.motion = preference;
  if (preference === 'reduce' || window.matchMedia('(prefers-reduced-motion: reduce)').matches) cancelMotion();
  try { localStorage.setItem('arkdock:motion', preference); } catch { /* Works without storage. */ }
}

export function cancelMotion(): void {
  // Changing a duration token does not interrupt a CSS transition already in flight.
  for (const animation of document.querySelector('.app')?.getAnimations({ subtree: true }) ?? []) animation.cancel();
}
