import assert from 'node:assert/strict';
import { spawn, execFileSync } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { connectWebview } from './webview-driver.mjs';

const checks = [];
const evidence = process.env.ARKDOCK_EVIDENCE_DIR ?? 'artifacts';
const pause = (ms) => new Promise((done) => setTimeout(done, ms));
let host;
let view;
function action(kind, handle, values = {}) {
  const args = ['-NoProfile', '-File', resolve('tests/native/window-actions.ps1'), '-Action', kind, '-Handle', String(handle)];
  for (const [key, value] of Object.entries(values)) args.push(`-${key}`, String(value));
  return JSON.parse(execFileSync('pwsh', args, { encoding: 'utf8', windowsHide: true, timeout: 12_000 }));
}
async function snapshot() { return view.evaluate('window.__TAURI__.core.invoke("snapshot")'); }
async function invoke(command, args = {}) {
  const state = await snapshot();
  return view.evaluate(`window.__TAURI__.core.invoke(${JSON.stringify(command)}, ${JSON.stringify({ ...args, scope: { targetId: state.target?.id ?? null, sessionId: state.sessionId } })})`);
}
async function until(name, predicate) {
  const deadline = Date.now() + 7000;
  while (Date.now() < deadline) { const state = await snapshot(); if (await predicate(state)) return state; await pause(100); }
  throw new Error(`${name}: timed out; snapshot=${JSON.stringify(await snapshot())}`);
}
function assertContact(target, dock) {
  assert.equal(target.frameRight, dock.frameLeft, 'Visible frames must have a zero seam');
  assert.equal(target.frameTop, dock.frameTop, 'Visible top edges must match');
  assert.equal(target.frameBottom, dock.frameBottom, 'Visible bottom edges must match');
  const widthDip = dock.clientWidth * 96 / dock.dpi;
  assert.ok(widthDip >= 360 && widthDip <= 560, `Dock client width ${widthDip} DIP is within supported bounds`);
  for (const [name, value] of [['target', target], ['Dock', dock]]) {
    assert.ok(value.frameLeft >= value.workLeft && value.frameTop >= value.workTop, `${name} visible origin stays within workarea`);
    assert.ok(value.frameRight <= value.workRight && value.frameBottom <= value.workBottom, `${name} visible extent stays within workarea`);
    assert.equal(value.owner, 0, `${name} remains an independent top-level window`);
  }
  assert.equal(dock.extended & 0x8, 0, 'Dock is not globally topmost');
}
async function check(id, name, execute) {
  await execute(); checks.push({ id, status: 'PASS', name }); console.log(`PASS ${id} ${name}`);
}
try {
  if (process.platform !== 'win32') throw new Error('Windows desktop required');
  view = await connectWebview();
  const dock = JSON.parse(execFileSync('pwsh', ['-NoProfile', '-Command', 'Get-Process -Name arkdock | Where-Object MainWindowHandle -NE 0 | Select-Object -First 1 Id,@{Name="MainWindowHandle";Expression={$_.MainWindowHandle.ToInt64()}} | ConvertTo-Json -Compress'], { encoding: 'utf8', windowsHide: true }));
  host = spawn('pwsh', ['-NoProfile', '-File', resolve('tests/native/window-host.ps1')], { windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] });
  const target = await new Promise((done, reject) => {
    let output = ''; let errors = '';
    const timer = setTimeout(() => reject(new Error(`Fixture startup timeout: ${errors}`)), 10_000);
    host.stderr.on('data', (chunk) => { errors += chunk; });
    host.stdout.on('data', (chunk) => { output += chunk; if (output.includes('\n')) { clearTimeout(timer); try { done(JSON.parse(output.split('\n')[0])); } catch (error) { reject(error); } } });
    host.on('exit', (code) => { clearTimeout(timer); reject(new Error(`Fixture exited ${code}: ${errors}`)); });
  });
  const dockHandle = action('find', 0, { ProcessId: dock.Id }).handle;
  assert.ok(dockHandle, 'ArkDock independent top-level main window must exist');
  await invoke('detach');
  await until('detached', (state) => state.target === null);
  action('restore', dockHandle); await pause(250);
  action('move', target.first, { X: 120, Y: 100, Width: 1000, Height: 1320 });
  action('move', target.second, { X: 50, Y: 520, Width: 400, Height: 420 });
  action('move', dockHandle, { X: 1800, Y: 100, Width: 906, Height: 1320 });
  action('foreground', target.first);
  action('foreground', dockHandle);
  await pause(300);
  const before = action('info', dockHandle);
  const button = await view.evaluate('(() => { const r = document.querySelector(".drag").getBoundingClientRect(); return { x: r.x+r.width/2, y: r.y+r.height/2, ratio: devicePixelRatio }; })()');
  const startX = Math.round(before.clientX + button.x * button.ratio);
  const startY = Math.round(before.clientY + button.y * button.ratio);
  const targetBefore = action('info', target.first);
  await view.evaluate('window.__arkdockPointer = []; document.addEventListener("pointerdown", e => window.__arkdockPointer.push({kind:e.type,x:e.clientX,y:e.clientY,target:e.target.className}), {once:true});');
  const dragged = action('drag', dockHandle, { X: startX, Y: startY, EndX: startX + targetBefore.frameRight - before.frameLeft, EndY: startY + targetBefore.frameTop - before.frameTop, OutlinePid: dock.Id });
  mkdirSync(evidence, { recursive: true });
  writeFileSync(`${evidence}/windows-drag.json`, `${JSON.stringify({before,targetBefore,dragged,targetAtRelease:action('info',target.first)},null,2)}\n`);
  console.log(JSON.stringify({ dockPid: dock.Id, targetPid: target.pid, dpi: before.dpi, pointer: await view.evaluate('window.__arkdockPointer') }));
  await check('A1', '实际鼠标拖动并释放绑定；同进程两个窗口按实际候选选择', async () => {
    const state = await until('binding and discovery', (value) => value.target?.process.pid === target.pid && value.phase === 'selectProject');
    assert.equal(state.target.title, 'ArkDock Windows 验收目标');
    assert.equal(state.phase, 'selectProject');
    await until('confirmed normal pair', (value) => value.target?.id === state.target.id && value.pairLayout?.state === 'paired');
    assertContact(action('info', target.first), action('info', dockHandle));
  });
  await check('A1', '目标移动和缩放驱动外置跟随', async () => {
    const old = action('info', dockHandle);
    const identity = (await snapshot()).target;
    action('move', target.first, { X: 80, Y: 90, Width: 560, Height: 570 });
    await pause(500);
    const moved = action('info', dockHandle);
    assert.ok(['left','top','right','bottom'].some((key) => moved[key] !== old[key]), 'Actual Dock geometry changes with the target');
    const currentTarget = action('info', target.first);
    assertContact(currentTarget, moved);
    const state = await snapshot();
    assert.equal(state.target.id, identity.id);
    assert.deepEqual(state.target.process, identity.process);
    assert.equal(state.target.process.pid, target.pid);
  });
  await check('B1', '当前目标实际选中 UIA 标签给出只读候选，唯一候选仍不自动关联', async () => {
    const state = await until('UIA candidate', (value) => value.candidates.some((item) => item.evidence.some((evidence) => evidence.source === 'uia')));
    const candidates = state.candidates.filter((item) => item.evidence.some((evidence) => evidence.source === 'uia'));
    assert.equal(candidates.length, 1); assert.equal(candidates[0].project.module, 'entry');
    assert.equal(candidates[0].autoEligible, false); assert.equal(state.project, null);
    assert.ok(candidates[0].evidence.some((evidence) => evidence.verifiable && !evidence.activeScope));
    assert.equal(state.phase, 'selectProject'); assert.equal(state.trusted, false);
  });
  await check('A1', '候选四边轮廓真实可见、无激活/点击穿透、无全局置顶，释放后隐藏', async () => {
    assert.equal(dragged.outlineDuringDrag.length, 4);
    for (const outline of dragged.outlineDuringDrag) { assert.ok(outline.extended & 0x08000000); assert.ok(outline.extended & 0x20); assert.equal(outline.extended & 0x8, 0); }
    assert.equal(action('info', dockHandle, { OutlinePid: dock.Id }).outlineVisible, 0);
  });
  await check('A3', '点击实际 ArkDock 后保持可见与自身焦点', async () => {
    const identity = (await snapshot()).target.id;
    const current = action('info', dockHandle);
    const point = await view.evaluate('(() => { const r = document.querySelector(".state-strip").getBoundingClientRect(); return { x: r.x+r.width/2, y: r.y+r.height/2, ratio: devicePixelRatio }; })()');
    action('click', dockHandle, { X: Math.round(current.clientX + point.x * point.ratio), Y: Math.round(current.clientY + point.y * point.ratio) });
    await pause(200);
    const focused = action('info', dockHandle);
    assert.equal(focused.visible, true); assert.equal(focused.foreground, dockHandle);
    assert.equal((await snapshot()).target.id, identity, 'Readonly focus click never starts a new binding drag');
  });
  await check('A2', '目标最小化隐藏伴侣；恢复不夺走目标焦点', async () => {
    action('minimize', target.first); await pause(500);
    assert.equal(action('info', dockHandle).visible, false);
    action('restore', target.first); action('foreground', target.first); await pause(500);
    const restored = action('info', dockHandle);
    assert.equal(restored.visible, true); assert.equal(restored.foreground, target.first);
    assert.equal((await snapshot()).target.process.pid, target.pid);
  });
  await check('A4', '并排布局须明确同意；解除恢复未改动的矩形', async () => {
    const oldTarget = action('info', target.first); const oldDock = action('info', dockHandle);
    await assert.rejects(invoke('arrange_side_by_side', { consent: false }));
    assert.equal(action('info', target.first).left, oldTarget.left);
    await invoke('arrange_side_by_side', { consent: true });
    await until('layout applied', (state) => state.dockHint?.includes('Consent layout applied'));
    await invoke('detach'); await until('layout detached', (state) => !state.target); await pause(250);
    const restoredTarget = action('info', target.first); const restoredDock = action('info', dockHandle);
    for (const key of ['left','top','right','bottom']) { assert.equal(restoredTarget[key], oldTarget[key]); assert.equal(restoredDock[key], oldDock[key]); }
  });
  async function rebind() {
    action('restore', target.first); await pause(200);
    action('move', target.first, { X: 120, Y: 100, Width: 1000, Height: 1320 });
    // Each repetition needs a real drag distance, even if the last restore left it adjacent.
    action('move', dockHandle, { X: 1800, Y: 100, Width: 906, Height: 1320 });
    action('foreground', target.first);
    action('foreground', dockHandle); await pause(200);
    const own = action('info', dockHandle); const destination = action('info', target.first);
    const grab = await view.evaluate('(() => { const r = document.querySelector(".drag").getBoundingClientRect(); return { x:r.x+r.width/2, y:r.y+r.height/2, ratio:devicePixelRatio }; })()');
    const x = Math.round(own.clientX + grab.x*grab.ratio); const y = Math.round(own.clientY + grab.y*grab.ratio);
    action('drag', dockHandle, { X:x, Y:y, EndX:x+destination.frameRight-own.frameLeft, EndY:y+destination.frameTop-own.frameTop });
    await until('rebound', (state) => state.target?.process.pid === target.pid);
    await until('rebound pair confirmed', (state) => state.pairLayout?.state === 'paired');
    assertContact(action('info', target.first), action('info', dockHandle));
  }
  await check('A4', '最大化目标同意并排；解除恢复最大化与焦点', async () => {
    await rebind(); action('maximize', target.first); await pause(350);
    assert.equal(action('info', target.first).maximized, true);
    await invoke('arrange_side_by_side', { consent: true });
    await until('maximized layout applied', (state) => state.dockHint?.includes('Consent layout applied'));
    assert.equal(action('info', target.first).maximized, false);
    action('foreground', dockHandle); await invoke('detach'); await pause(350);
    assert.equal(action('info', target.first).maximized, true);
    assert.equal(action('info', dockHandle).foreground, dockHandle);
  });
  await check('A4', '用户后来改变布局，解除不会覆盖该布局', async () => {
    await rebind(); await invoke('arrange_side_by_side', { consent: true });
    await until('layout applied for change', (state) => state.dockHint?.includes('Consent layout applied'));
    action('move', target.first, { X:100, Y:500, Width:650, Height:650 }); await pause(250);
    const changed = action('info', target.first);
    await invoke('detach'); await pause(250);
    const after = action('info', target.first);
    for (const key of ['left','top','right','bottom']) assert.equal(after[key], changed[key]);
  });
  for (const maximized of [true, false]) for (const detachDelay of [0, 1100]) await check('A4', `${maximized ? '最大化' : '普通'}目标队列延迟时 ${detachDelay} ms 后解除，迟到布局得到安全补偿`, async () => {
    await rebind(); if (maximized) action('maximize', target.first); await pause(350);
    const originalTarget = action('info', target.first); const originalDock = action('info', dockHandle);
    const paused = new Promise((done, reject) => {
      const timer = setTimeout(() => reject(new Error('Target pause acknowledgement timed out')), 3000);
      const listener = (chunk) => { if (chunk.toString().includes('pauseReady')) { clearTimeout(timer); host.stdout.off('data', listener); done(); } };
      host.stdout.on('data', listener);
    });
    action('pause', target.first); await paused;
    await invoke('arrange_side_by_side', { consent: true });
    await pause(detachDelay);
    await invoke('detach');
    await until('pending arrangement detached', (state) => state.target === null);
    await pause(2400);
    const restoredTarget = action('info', target.first); const restoredDock = action('info', dockHandle);
    assert.equal(restoredTarget.maximized, maximized);
    if (!maximized) for (const key of ['left','top','right','bottom']) {
      assert.equal(restoredTarget[key], originalTarget[key], `target ${key}`);
      assert.equal(restoredDock[key], originalDock[key], `dock ${key}`);
    }
    assert.equal((await snapshot()).target, null);
  });
  await check('A2', '关闭实际目标后解除绑定，清空会话且伴侣可见', async () => {
    await rebind(); action('close', target.first);
    const closed = await until('closed target', (state) => state.target === null);
    assert.equal(closed.sessionId, null); assert.equal(closed.project, null);
    assert.equal(action('info', dockHandle).visible, true);
  });
  const image = await view.send('Page.captureScreenshot', { format: 'png' });
  mkdirSync(evidence, { recursive: true });
  writeFileSync(`${evidence}/windows-smoke.png`, Buffer.from(image.data, 'base64'));
  checks.push({ id: 'A3', status: 'BLOCKED', name: '跨 DPI 双屏、负坐标：本机只有单块显示器' });
  checks.push({ id: 'A2', status: 'SKIPPED', name: '高权限差异、快速 HWND 复用压力：本驱动未执行' });
} catch (error) {
  checks.push({ status: 'FAIL', name: error.message }); console.error(error);
  process.exitCode = 1;
} finally {
  if (view) { try { await invoke('detach'); } catch { /* App may have exited. */ } view.close(); }
  // Only the child process created by this driver is owned; never terminate by name.
  if (host && host.exitCode === null) host.kill();
  mkdirSync(evidence, { recursive: true });
  writeFileSync(`${evidence}/windows-smoke.json`, `${JSON.stringify({ capturedAt: new Date().toISOString(), platform: process.platform, scope: 'A Windows desktop assertions only; no HarmonyOS preview assertions', checks }, null, 2)}\n`);
}
