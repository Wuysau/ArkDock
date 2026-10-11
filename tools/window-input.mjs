// A restored NOACTIVATE surface needs one harmless owned-window activation click before dragging.
export async function activateOwnedHeader(view, handle, action) {
  const info = action('info', handle);
  if (info.foreground === handle) return;
  const point = await view.evaluate('(() => { const r = document.querySelector(".header-status").getBoundingClientRect(); return { x:r.x+r.width/2, y:r.y+r.height/2, ratio:devicePixelRatio }; })()');
  action('click', handle, { X: Math.round(info.clientX + point.x * point.ratio), Y: Math.round(info.clientY + point.y * point.ratio) });
  if (action('info', handle).foreground !== handle) throw new Error('BLOCKED: owned activation did not retain foreground; stop pointer testing while another window is taking focus');
}

// Probe only our known fixture edge; a covered fixture is an environment block,
// not permission to click or alter the covering application.
export function requireOwnedTargetEdge(action, handle, bounds) {
  const probes = [0.25, 0.5, 0.75].map(fraction => action('probe', handle, {
    X: Math.floor(bounds.frameRight - 1),
    Y: Math.floor(bounds.frameTop + (bounds.frameBottom - bounds.frameTop) * fraction),
  }));
  if (bounds.visible && !bounds.minimized && probes.some(probe => probe.owned)) return probes;
  const error = new Error('BLOCKED: the owned target edge is covered or unavailable; no drag was started');
  error.blocked = true;
  throw error;
}
