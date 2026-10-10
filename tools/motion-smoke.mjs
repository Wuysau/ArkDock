// Real native surfaces + low-frequency counters. Does not manufacture desktop events or ArkUI frames.
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { connectOwnedWebview as connectWebview } from './webview-driver.mjs';
import { ownedDesktop } from './owned-desktop.mjs';

const evidence = process.env.ARKDOCK_EVIDENCE_DIR ?? 'artifacts/magnetic-motion-2026-10-10/native';
const pause = ms => new Promise(done => setTimeout(done, ms));
const checks = [], measurements = [];
let view, dock, mainHandle;
function native(action, handle = 0, values = {}) {
  const args = ['-NoProfile', '-File', resolve('tests/native/surface-actions.ps1'), '-Action', action, '-ProcessId', String(dock.Id), '-Handle', String(handle)];
  for (const [key, value] of Object.entries(values)) args.push(`-${key}`, String(value));
  return JSON.parse(execFileSync('pwsh', args, { encoding: 'utf8', windowsHide: true, timeout: 12_000 }));
}
function windowInfo() {
  return JSON.parse(execFileSync('pwsh', ['-NoProfile', '-File', resolve('tests/native/window-actions.ps1'), '-Action', 'info', '-Handle', String(mainHandle)], { encoding: 'utf8', windowsHide: true, timeout: 12_000 }));
}
function pointerAction(kind, handle, values) {
  const args = ['-NoProfile', '-File', resolve('tests/native/window-actions.ps1'), '-Action', kind, '-Handle', String(handle)];
  for (const [key, value] of Object.entries(values)) args.push(`-${key}`, String(value));
  return JSON.parse(execFileSync('pwsh', args, { encoding: 'utf8', windowsHide: true, timeout: 12_000 }));
}
async function snapshot() { return view.evaluate('window.__TAURI__.core.invoke("snapshot")'); }
async function invoke(command, args = {}) {
  const state = await snapshot();
  return view.evaluate(`window.__TAURI__.core.invoke(${JSON.stringify(command)},${JSON.stringify({ ...args, scope: { targetId: state.target?.id ?? null, sessionId: state.sessionId } })})`);
}
async function change(command) {
  const state = await snapshot();
  return invoke(command, { requestId: state.display.requestId + 1 });
}
async function until(mode) {
  const deadline = Date.now() + 8000;
  while (Date.now() < deadline) {
    const state = await snapshot();
    if (state.display.mode === mode) return state;
    await pause(60);
  }
  throw new Error(`Native display mode ${mode} not confirmed: ${JSON.stringify((await snapshot()).display)}`);
}
function surfaces() { return native('list'); }
function orb() { return surfaces().find(item => item.title === 'ArkDock Orb'); }
function receiver() { return surfaces().find(item => item.title === 'ArkDock Tray Receiver'); }
async function check(id, name, execute) {
  try { await execute(); checks.push({ id, status: 'PASS', name }); console.log(`PASS ${id} ${name}`); }
  catch (error) { checks.push({ id, status: 'FAIL', name, reason: error.message }); console.error(`FAIL ${id} ${error.message}`); }
}
function processSample() {
  return JSON.parse(execFileSync('pwsh', ['-NoProfile', '-Command',
    `$p=Get-Process -Id ${dock.Id}; @{time=[DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds();cpuMs=$p.TotalProcessorTime.TotalMilliseconds;workingSet=$p.WorkingSet64;privateBytes=$p.PrivateMemorySize64;handles=$p.HandleCount} | ConvertTo-Json -Compress`],
  { encoding: 'utf8', windowsHide: true, timeout: 12_000 }));
}
async function sample(label) {
  const first = processSample();
  const before = await invoke('get_motion_metrics');
  await pause(2000);
  const last = processSample();
  const after = await invoke('get_motion_metrics');
  measurements.push({ label, first, last, singleCoreCpuPercent: (last.cpuMs - first.cpuMs) / (last.time - first.time) * 100, before, after });
  return { before, after };
}
try {
  mkdirSync(evidence, { recursive: true });
  assert.equal(process.platform, 'win32', 'Windows desktop required');
  dock = ownedDesktop();
  view = await connectWebview();
  mainHandle = JSON.parse(execFileSync('pwsh', ['-NoProfile', '-File', resolve('tests/native/window-actions.ps1'), '-Action', 'find', '-Handle', '0', '-ProcessId', String(dock.Id)], { encoding: 'utf8', windowsHide: true })).handle;
  assert.ok(mainHandle, 'Owned main window required');
  await change('show_from_tray'); await until('expanded');
  await check('A-idle', 'No candidate means no particle frames or render wakes', async () => {
    const { before, after } = await sample('expanded-without-drag');
    assert.equal(after.frames, before.frames); assert.equal(after.wakeCount, before.wakeCount);
  });
  const original = await snapshot();
  await check('C-collapse', 'Native circular Orb replaces main and retains workspace/build identity', async () => {
    await change('collapse_to_orb'); const state = await until('orb');
    assert.equal(windowInfo().visible, false);
    const surface = orb(); assert.ok(surface?.visible);
    assert.ok(Math.abs((surface.right - surface.left) * 96 / surface.dpi - 58) <= 1);
    assert.equal(surface.extended & 0x8, 0x8, 'Visible Orb remains reachable above the foreground agent');
    assert.ok(surface.extended & 0x8000000, 'Orb is NOACTIVATE');
    const probe = native('probe', surface.handle);
    measurements.push({ label: 'orb-hit-test', surface, probe });
    assert.equal(probe.centerWindow, surface.handle, 'Circle receives center pointer');
    assert.notEqual(probe.cornerWindow, surface.handle, 'Transparent corner passes pointer through');
    assert.equal(probe.mouseActivate, 3, 'MA_NOACTIVATE');
    for (const key of ['target', 'project', 'sessionId', 'sessionRef', 'trusted']) assert.deepEqual(state[key], original[key], `Display change preserves ${key}`);
    native('capture', surface.handle, { OutputPath: resolve(evidence, 'native-orb.png') });
  });
  await check('C-idempotent', 'Repeated current display request is idempotent and old request is rejected', async () => {
    const state = await snapshot();
    await invoke('collapse_to_orb', { requestId: state.display.requestId });
    assert.equal((await snapshot()).display.mode, 'orb');
    await assert.rejects(invoke('expand_from_orb', { requestId: state.display.requestId - 1 }));
    assert.equal((await snapshot()).display.mode, 'orb');
  });
  await check('C-expand', 'Native Orb menu expands main without creating a second main surface', async () => {
    native('menu', orb().handle, { MenuId: 1 }); await until('expanded');
    assert.equal(windowInfo().visible, true); assert.equal(orb().visible, false);
    assert.equal(orb().extended & 0x8, 0, 'Hidden Orb relinquishes topmost status');
    assert.ok(windowInfo().clientWidth * 96 / windowInfo().dpi >= 360);
    const image = await view.send('Page.captureScreenshot', { format: 'png' });
    writeFileSync(resolve(evidence, 'native-expanded.png'), Buffer.from(image.data, 'base64'));
  });
  await check('C-pointer', 'Real pointer drag chooses nearest edge without activation; click expands without a drag', async () => {
    await change('collapse_to_orb'); await until('orb'); await pause(350);
    const first = orb(), work = windowInfo();
    assert.equal(native('probe', first.handle).centerWindow, first.handle, 'Pointer must hit the owned Orb before input');
    const foreground = windowInfo().foreground;
    const drag = pointerAction('drag', first.handle, {
      X: Math.round((first.left + first.right) / 2), Y: Math.round((first.top + first.bottom) / 2),
      EndX: work.workLeft + 120, EndY: work.workTop + Math.round((work.workBottom - work.workTop) / 3),
    });
    await pause(250); const moved = orb();
    assert.equal((await snapshot()).display.mode, 'orb', 'Dragging does not expand');
    assert.equal(moved.left, work.workLeft, 'Left docking is flush with the current work-area edge');
    assert.ok(moved.top >= work.workTop && moved.bottom <= work.workBottom);
    assert.equal(windowInfo().foreground, foreground, 'NOACTIVATE Orb drag does not steal focus');
    measurements.push({ label: 'real-orb-drag', first, moved, drag });
    assert.equal(native('probe', moved.handle).centerWindow, moved.handle);
    pointerAction('click', moved.handle, { X: Math.round((moved.left + moved.right) / 2), Y: Math.round((moved.top + moved.bottom) / 2) });
    await until('expanded'); assert.equal(windowInfo().visible, true); assert.equal(orb().visible, false);
  });
  await check('D-hide', 'Confirmed tray registration permits hiding both surfaces and restoring Orb', async () => {
    assert.equal((await snapshot()).display.trayAvailable, true);
    await change('hide_to_tray'); await until('hidden');
    assert.equal(windowInfo().visible, false); assert.equal(orb().visible, false);
    native('menu', receiver().handle, { MenuId: 2 }); await until('orb'); assert.equal(orb().visible, true);
  });
  await check('D-recover', 'Tray restores full main and re-registers after TaskbarCreated', async () => {
    await change('hide_to_tray'); await until('hidden');
    native('taskbar-created', receiver().handle);
    const deadline = Date.now() + 8000;
    while (Date.now() < deadline && !(await snapshot()).display.trayAvailable) await pause(80);
    assert.equal((await snapshot()).display.trayAvailable, true);
    native('menu', receiver().handle, { MenuId: 1 }); await until('expanded');
    assert.equal(windowInfo().visible, true); assert.equal(orb().visible, false);
  });
  await check('C-auto-hide', 'Auto-hide leaves a clickable exposed Orb and disabling restores it', async () => {
    await invoke('set_orb_auto_hide', { enabled: true }); await change('collapse_to_orb'); await until('orb');
    const work = windowInfo();
    pointerAction('cursor', orb().handle, { X: Math.round((work.workLeft + work.workRight) / 2), Y: work.workTop + 100 });
    // Native idle policy owns this delay; no delay is used to infer successful display transitions.
    await pause(4000); const hidden = orb(); measurements.push({ label: 'orb-idle-auto-hide', surface: hidden });
    await invoke('set_orb_auto_hide', { enabled: false }); await pause(250);
    const revealed = orb(); assert.ok(revealed?.visible);
    assert.notEqual(hidden.left, revealed.left, 'Idle edge hide actually moves the native Orb');
    await sample('orb-with-auto-hide-disabled');
    await change('expand_from_orb'); await until('expanded');
  });
  checks.push({ id: 'A-drag', status: 'SKIPPED', name: 'Particles while real dragging are covered by separate windows/joint driver and metrics evidence' });
  checks.push({ id: 'C-multi-DPI', status: 'BLOCKED', name: 'Cross-monitor DPI requires additional physical display, not browser emulation' });
  checks.push({ id: 'E-native-preview', status: 'SKIPPED', name: 'Actual SDK build/frame/input covered by separate verify:native gate' });
} catch (error) {
  checks.push({ status: 'FAIL', name: error.message }); console.error(error);
} finally {
  if (view) {
    try { await invoke('set_orb_auto_hide', { enabled: false }); await change('show_from_tray'); await until('expanded'); } catch (error) { checks.push({ status: 'FAIL', name: `Recovery cleanup: ${error.message}` }); }
    view.close();
  }
  const status = checks.some(item => item.status === 'FAIL') ? 'FAIL' : checks.some(item => item.status === 'BLOCKED') ? 'BLOCKED' : 'PASS';
  mkdirSync(evidence, { recursive: true });
  writeFileSync(resolve(evidence, 'motion-smoke.json'), `${JSON.stringify({ capturedAt: new Date().toISOString(), status, scope: 'Actual owned Windows surfaces; tray menu callback injection is distinct from taskbar mouse interaction; no ArkUI fixture frames', dock, checks, measurements }, null, 2)}\n`);
  process.exitCode = status === 'FAIL' ? 1 : status === 'BLOCKED' ? 2 : 0;
}
