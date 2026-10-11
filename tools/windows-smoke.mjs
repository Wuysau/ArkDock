import assert from 'node:assert/strict';
import { spawn, execFileSync } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { connectOwnedWebview as connectWebview } from './webview-driver.mjs';
import { ownedDesktop } from './owned-desktop.mjs';
import { activateOwnedHeader } from './window-input.mjs';

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
  const dock = ownedDesktop();
  host = spawn('pwsh', ['-NoProfile', '-STA', '-File', resolve('tests/native/window-host.ps1')], { windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] });
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
  const activatedTarget=action('info',target.first);
  action('click',target.first,{X:activatedTarget.clientX+100,Y:activatedTarget.clientY+100});
  action('foreground', dockHandle);
  await pause(300);
  await activateOwnedHeader(view, dockHandle, action);
  const before = action('info', dockHandle);
  const button = await view.evaluate('(() => { const r = document.querySelector(".drag").getBoundingClientRect(); return { x: r.x+r.width/2, y: r.y+r.height/2, ratio: devicePixelRatio }; })()');
  const startX = Math.round(before.clientX + button.x * button.ratio);
  const startY = Math.round(before.clientY + button.y * button.ratio);
  const targetBefore = action('info', target.first);
  await view.evaluate('window.__arkdockPointer = []; document.addEventListener("pointerdown", e => window.__arkdockPointer.push({kind:e.type,x:e.clientX,y:e.clientY,target:e.target.className}), {once:true});');
  await view.evaluate(`(async()=>{
    const trace=window.__arkdockDockingSmoke={confirmations:[],ripples:[],animationStarts:[],seen:new Set()};
    const animationStart=event=>{const ripple=event.target.closest?.('.dock-ripple');if(ripple)trace.animationStarts.push({bindingId:ripple.dataset.bindingId,side:ripple.dataset.side,name:event.animationName});};
    document.addEventListener('animationstart',animationStart,true);
    trace.cleanup=()=>{trace.off?.();document.removeEventListener('animationstart',animationStart,true);};
    trace.off=await window.__TAURI__.event.listen('arkdock:state',({payload})=>{
      const confirmation=payload.dockConfirmation;
      if(!confirmation||trace.seen.has(confirmation.bindingId))return;
      trace.seen.add(confirmation.bindingId);trace.confirmations.push(confirmation);
      requestAnimationFrame(()=>requestAnimationFrame(()=>{
        const ripple=document.querySelector('.dock-ripple');
        if(ripple)trace.ripples.push({bindingId:ripple.dataset.bindingId,side:ripple.dataset.side,
          animations:ripple.getAnimations({subtree:true}).map(animation=>({state:animation.playState,progress:animation.effect?.getComputedTiming().progress}))});
      }));
    });
  })()`);
  const motionBefore = await invoke('get_motion_metrics');
  const dragged = action('drag', dockHandle, { X: startX, Y: startY, EndX: startX + targetBefore.frameRight - before.frameLeft, EndY: startY + targetBefore.frameTop - before.frameTop, OutlinePid: dock.Id, HoldMs: 1500, CaptureDir: resolve(evidence, 'particle-frames'), CaptureX: targetBefore.frameRight - 70, CaptureY: targetBefore.frameTop + 140 });
  const motionAfter = await invoke('get_motion_metrics');
  mkdirSync(evidence, { recursive: true });
  writeFileSync(`${evidence}/windows-drag.json`, `${JSON.stringify({before,targetBefore,dragged,motionBefore,motionAfter,targetAtRelease:action('info',target.first)},null,2)}\n`);
  console.log(JSON.stringify({ dockPid: dock.Id, targetPid: target.pid, dpi: before.dpi, pointer: await view.evaluate('window.__arkdockPointer') }));
  await check('A1', '实际鼠标拖动并释放绑定；同进程两个窗口按实际候选选择', async () => {
    const state = await until('binding and discovery', (value) => value.target?.process.pid === target.pid && value.phase === 'selectProject');
    assert.equal(state.target.title, 'ArkDock Windows 验收目标');
    assert.equal(state.phase, 'selectProject');
    await until('confirmed normal pair', (value) => value.target?.id === state.target.id && value.pairLayout?.state === 'paired');
    assertContact(action('info', target.first), action('info', dockHandle));
  });
  const rippleStartsBeforeFollow=await view.evaluate('window.__arkdockDockingSmoke.animationStarts.length');
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
  const rippleState=await snapshot();
  if(rippleState.motion.reduced||['off','light'].includes(rippleState.motion.intensity)){
    checks.push({id:'A1-ripple',status:'SKIPPED',name:'当前减少动画或轻量偏好禁用水波，不能声称播放通过'});
  }else await check('A1-ripple', '真实绑定确认触发水波动画，跟随缩放不重播', async()=>{
    const trace=await view.evaluate('({confirmations:window.__arkdockDockingSmoke.confirmations,ripples:window.__arkdockDockingSmoke.ripples,animationStarts:window.__arkdockDockingSmoke.animationStarts})');
    const matching=trace.ripples.filter(ripple=>ripple.bindingId===rippleState.dockConfirmation?.bindingId);
    assert.equal(matching.length,1);assert.equal(matching[0].side,rippleState.dockConfirmation.side);
    assert.ok(matching[0].animations.some(animation=>animation.state==='running'));
    assert.ok(rippleStartsBeforeFollow>0);assert.equal(trace.animationStarts.length,rippleStartsBeforeFollow,'Following geometry must not start a new ripple animation');
    writeFileSync(`${evidence}/windows-ripple.json`,`${JSON.stringify(trace,null,2)}\n`);
  });
  await check('B1', '当前目标实际选中 UIA 标签给出只读候选，唯一候选仍不自动关联', async () => {
    const state = await until('UIA candidate', (value) => value.candidates.some((item) => item.evidence.some((evidence) => evidence.source === 'uia')));
    const candidates = state.candidates.filter((item) => item.evidence.some((evidence) => evidence.source === 'uia'));
    assert.equal(candidates.length, 1); assert.equal(candidates[0].project.module, 'entry');
    assert.equal(candidates[0].autoEligible, false); assert.equal(state.project, null);
    assert.ok(candidates[0].evidence.some((evidence) => evidence.verifiable && !evidence.activeScope));
    assert.equal(state.phase, 'selectProject'); assert.equal(state.trusted, false);
  });
  await check('A1', '原生磁吸层仅候选期间临时置顶、无激活/点击穿透，释放后隐藏并撤销置顶', async () => {
    // One native alpha surface replaces four static strips; unsupported renderers retain the exact four-strip fallback.
    assert.equal(dragged.outlineDuringDrag.length, motionAfter.frames > motionBefore.frames ? 1 : 4);
    for (const outline of dragged.outlineDuringDrag) { assert.ok(outline.extended & 0x08000000); assert.ok(outline.extended & 0x20); assert.equal(outline.extended & 0x8, dragged.outlineDuringDrag.length === 1 ? 8 : 0); }
    assert.equal(action('info', dockHandle, { OutlinePid: dock.Id }).outlineVisible, 0);
    const layers = JSON.parse(execFileSync('pwsh', ['-NoProfile', '-File', resolve('tests/native/surface-actions.ps1'), '-Action', 'list', '-ProcessId', String(dock.Id)], { encoding: 'utf8', windowsHide: true })).filter(item => item.class.startsWith('ArkDockMagneticParticles-'));
    assert.equal(layers.length, 1); assert.equal(layers[0].visible, false); assert.equal(layers[0].extended & 8, 0);
  });
  if(motionAfter.frames>motionBefore.frames)await check('A1-particles','真实候选期间绘制原生粒子且无新增失败帧',async()=>{assert.equal(motionAfter.failedFrames,motionBefore.failedFrames);});
  else checks.push({id:'A1-particles',status:'BLOCKED',name:'原生粒子未产生帧；四边轮廓降级不能证明粒子渲染'});
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
    action('restore', target.first); action('foreground', target.first);
    const activated = action('info', target.first);
    if (activated.foreground !== target.first) action('click', target.first, { X: activated.clientX + 40, Y: activated.clientY + 40 });
    assert.equal(action('info', target.first).foreground, target.first, 'Owned target activation is a real precondition');
    await pause(500);
    const restored = action('info', dockHandle);
    if (restored.foreground !== target.first && restored.foreground !== dockHandle) throw new Error('BLOCKED: an unrelated foreground application interrupted the focus-preservation test');
    assert.equal(restored.visible, true); assert.equal(restored.foreground, target.first);
    assert.equal((await snapshot()).target.process.pid, target.pid);
  });
  await check('C-bound', '收起保留真实关联与信任状态，目标最小化不隐藏 Orb，展开尊重新的目标几何', async () => {
    const candidate = (await snapshot()).candidates.find(item => item.valid && item.evidence.some(e => e.source === 'uia'));
    assert.ok(candidate);
    await invoke('select_candidate', { candidateId: candidate.id });
    const before = await snapshot();
    assert.equal(before.trusted, false); assert.equal(before.phase, 'untrusted');
    await invoke('collapse_to_orb', { requestId: before.display.requestId + 1 });
    await until('bound Orb', state => state.display.mode === 'orb');
    const orbSurface = () => JSON.parse(execFileSync('pwsh', ['-NoProfile', '-File', resolve('tests/native/surface-actions.ps1'), '-Action', 'list', '-ProcessId', String(dock.Id)], { encoding: 'utf8', windowsHide: true })).find(item => item.title === 'ArkDock Orb');
    assert.equal(action('info', dockHandle).visible, false); assert.equal(orbSurface().visible, true);
    action('minimize', target.first); await pause(300);
    assert.equal(orbSurface().visible, true); assert.equal(action('info', dockHandle).visible, false);
    action('restore', target.first); await pause(250);
    action('move', target.first, { X: 180, Y: 140, Width: 920, Height: 1320 }); await pause(250);
    const userGeometry = action('info', target.first);
    const collapsed = await snapshot();
    for (const key of ['project', 'sessionId', 'sessionRef', 'trusted']) assert.deepEqual(collapsed[key], before[key]);
    assert.equal(collapsed.target.id, before.target.id);
    await invoke('expand_from_orb', { requestId: collapsed.display.requestId + 1 });
    await until('bound expanded', state => state.display.mode === 'expanded' && state.pairLayout.state === 'paired'); await pause(200);
    const resumed = action('info', target.first);
    for (const key of ['frameLeft', 'frameTop', 'frameRight', 'frameBottom']) assert.equal(resumed[key], userGeometry[key], `Latest user target ${key}`);
    assertContact(resumed, action('info', dockHandle)); assert.equal(orbSurface().visible, false);
    const expanded = await snapshot();
    for (const key of ['project', 'sessionId', 'sessionRef', 'trusted']) assert.deepEqual(expanded[key], before[key]);
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
    await activateOwnedHeader(view, dockHandle, action);
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
    writeFileSync(`${evidence}/delay-${maximized}-${detachDelay}-baseline.json`, JSON.stringify({ originalTarget, originalDock, state: await snapshot() }, null, 2));
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
    writeFileSync(`${evidence}/delay-${maximized}-${detachDelay}-restored.json`, JSON.stringify({ restoredTarget, restoredDock, state: await snapshot() }, null, 2));
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
  const blocked = error.message.includes('BLOCKED:');
  checks.push({ status: blocked ? 'BLOCKED' : 'FAIL', name: error.message }); console.error(error);
  process.exitCode = blocked ? 2 : 1;
} finally {
  if (view) {
    try { await view.evaluate('window.__arkdockDockingSmoke?.cleanup?.();delete window.__arkdockDockingSmoke;'); } catch { /* Preserve primary failure. */ }
    try { await invoke('detach'); } catch { /* App may have exited. */ }
    view.close();
  }
  // Only the child process created by this driver is owned; never terminate by name.
  if (host && host.exitCode === null) host.kill();
  mkdirSync(evidence, { recursive: true });
  const status=checks.some(item=>item.status==='FAIL')?'FAIL':checks.some(item=>item.status==='BLOCKED')?'BLOCKED':'PASS';
  writeFileSync(`${evidence}/windows-smoke.json`, `${JSON.stringify({ status, capturedAt: new Date().toISOString(), platform: process.platform, scope: 'A Windows desktop assertions only; no HarmonyOS preview assertions', checks }, null, 2)}\n`);
  process.exitCode=status==='FAIL'?1:status==='BLOCKED'?2:0;
}
