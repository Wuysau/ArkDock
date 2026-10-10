// A restored NOACTIVATE surface needs one harmless owned-window activation click before dragging.
export async function activateOwnedHeader(view, handle, action) {
  const info = action('info', handle);
  if (info.foreground === handle) return;
  const point = await view.evaluate('(() => { const r = document.querySelector(".header-status").getBoundingClientRect(); return { x:r.x+r.width/2, y:r.y+r.height/2, ratio:devicePixelRatio }; })()');
  action('click', handle, { X: Math.round(info.clientX + point.x * point.ratio), Y: Math.round(info.clientY + point.y * point.ratio) });
  if (action('info', handle).foreground !== handle) throw new Error('BLOCKED: owned activation did not retain foreground; stop pointer testing while another window is taking focus');
}
