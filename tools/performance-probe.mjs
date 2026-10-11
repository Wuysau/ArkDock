// Actual Windows WebView2 + SDK counter. Requires the caller's explicitly owned ArkDock PID.
import assert from 'node:assert/strict';
import { spawn, execFileSync } from 'node:child_process';
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { resolve, join } from 'node:path';
import { connectWebview } from './webview-driver.mjs';
import { requireOwnedTargetEdge } from './window-input.mjs';

const phase = process.argv[2] ?? 'baseline';
if (!['baseline', 'optimized', 'stress'].includes(phase)) throw new Error('Use baseline, optimized or stress');
const ownedPid = Number(process.env.ARKDOCK_PROBE_PID);
const port = Number(process.env.ARKDOCK_PROBE_PORT ?? 9447);
const url = process.env.ARKDOCK_PROBE_URL ?? 'http://127.0.0.1:1420/';
const diagnostics = process.env.ARKDOCK_PROBE_DEBUG === '1';
const project = resolve('examples/native-counter');
const sourcePath = join(project, 'entry/src/main/ets/pages/Index.ets');
const source = readFileSync(sourcePath, 'utf8');
const pause = (ms) => new Promise((done) => setTimeout(done, ms));
const checks = [];
const report = { phase, capturedAt: new Date().toISOString(), platform: process.platform, sdkVersion: '26.0.0.105', checks };
let view, host, projectId, originalTrust = false, currentLog;
let sourceChanged = false;
function blocked(message) { const error = new Error(message); error.blocked = true; throw error; }
function action(kind, handle, values = {}) {
  const args = ['-NoProfile', '-File', resolve('tests/native/window-actions.ps1'), '-Action', kind, '-Handle', String(handle)];
  for (const [key, value] of Object.entries(values)) args.push(`-${key}`, String(value));
  return JSON.parse(execFileSync('pwsh', args, { encoding: 'utf8', windowsHide: true, timeout: 12000 }));
}
const snapshot = () => view.evaluate('window.__TAURI__.core.invoke("snapshot")');
async function invoke(command, args = {}) {
  const state = await snapshot();
  return view.evaluate(`window.__TAURI__.core.invoke(${JSON.stringify(command)},${JSON.stringify({ ...args, scope: { targetId: state.target?.id ?? null, sessionId: state.sessionId } })})`);
}
async function until(name, test, timeout = 10000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) { const result = await test(); if (result) return result; await pause(50); }
  throw new Error(`${name}: timeout`);
}
async function clickControl(expression) {
  const point = await until('UI control ready', () => view.evaluate(`(() => {const e=${expression};if(!e||e.disabled)return null;const r=e.getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2};})()`));
  await view.send('Input.dispatchMouseEvent', { type: 'mousePressed', ...point, button: 'left', clickCount: 1 });
  await view.send('Input.dispatchMouseEvent', { type: 'mouseReleased', ...point, button: 'left', clickCount: 1 });
}
function percentile(values, fraction) {
  if (!values.length) return null;
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.max(0, Math.ceil(sorted.length * fraction) - 1)];
}
const summary = (values) => ({ samples: values.length, p50Ms: percentile(values, 0.5), p95Ms: percentile(values, 0.95) });
async function capturedInputScope() {
  const evaluated = await view.send('Runtime.evaluate', { expression: 'window.__arkdockPerfHandler', returnByValue: false, objectGroup: 'arkdock-perf' });
  const properties = await view.send('Runtime.getProperties', { objectId: evaluated.result.objectId, ownProperties: true });
  const scopeList = properties.internalProperties?.find((entry) => entry.name === '[[Scopes]]')?.value.objectId;
  if (!scopeList) return [];
  const scopes = await view.send('Runtime.getProperties', { objectId: scopeList, ownProperties: true });
  const result = [];
  for (const scope of scopes.result.filter((entry) => entry.value?.description?.startsWith('Closure'))) {
    const values = await view.send('Runtime.getProperties', { objectId: scope.value.objectId, ownProperties: true });
    for (const entry of values.result.filter((item) => ['interactionMode', 'inputPending', 'pendingRef', 'stateRef', 'bridge', 'clickNative'].includes(item.name))) {
      const item = { name: entry.name, value: entry.value?.value, description: entry.value?.description };
      if (entry.value?.objectId && ['inputPending', 'pendingRef'].includes(entry.name)) {
        const ref = await view.send('Runtime.getProperties', { objectId: entry.value.objectId, ownProperties: true }); item.current = ref.result.find((field) => field.name === 'current')?.value?.value;
      }
      result.push(item);
    }
  }
  await view.send('Runtime.releaseObjectGroup', { objectGroup: 'arkdock-perf' });
  return result;
}
try {
  if (process.platform !== 'win32' || !process.env.ARKDOCK_SDK) blocked('Windows and explicit ARKDOCK_SDK required');
  if (!Number.isSafeInteger(ownedPid) || ownedPid <= 0) blocked('ARKDOCK_PROBE_PID must identify only the ArkDock instance created for this task');
  const deveco = execFileSync('pwsh', ['-NoProfile', '-Command', '@(Get-Process -Name deveco64,deveco -ErrorAction SilentlyContinue).Count'], { encoding: 'utf8', windowsHide: true });
  if (Number(deveco.trim())) blocked('DevEco GUI must be closed by its owner; no external process was stopped');
  try { view = await connectWebview(port, 5000, url, ownedPid); } catch (error) { blocked(`Owned ArkDock debugger unavailable: ${error.message}`); }
  const initial = await snapshot();
  if (initial.target || initial.sessionId) blocked('Dedicated probe ArkDock must start detached; no existing user association was changed');
  if (!initial.backend.available) blocked(initial.backend.reason);
  // Honor the native runbook's bounded pending placement compensation after a prior detach.
  await pause(3500);
  const dockHandle = action('find', 0, { ProcessId: ownedPid }).handle;
  if (!dockHandle) blocked('Explicitly owned ArkDock window not found');
  host = spawn('pwsh', ['-NoProfile', '-File', resolve('tests/native/window-host.ps1'), '-ProjectPath', project], { windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] });
  const target = await new Promise((done, reject) => {
    let output = '', errors = '';
    const timer = setTimeout(() => reject(new Error(`Owned fixture startup timeout: ${errors}`)), 10000);
    host.stderr.on('data', (data) => { errors += data; });
    host.stdout.on('data', (data) => { output += data; if (output.includes('\n')) { clearTimeout(timer); done(JSON.parse(output.split('\n')[0])); } });
    host.once('exit', (code) => { clearTimeout(timer); reject(new Error(`Owned fixture exit ${code}`)); });
  });
  action('restore', dockHandle);
  action('move', target.first, { X: 40, Y: 400, Width: 700, Height: 700 });
  action('move', target.second, { X: 50, Y: 520, Width: 400, Height: 420 });
  action('move', dockHandle, { X: 1000, Y: 400, Width: 1000, Height: 1250 });
  action('foreground', target.first); await pause(200);
  action('foreground', dockHandle); await pause(250);
  let before = action('info', dockHandle);
  await pause(450);
  const stable = action('info', dockHandle);
  if (JSON.stringify([before.left,before.top,before.right,before.bottom]) !== JSON.stringify([stable.left,stable.top,stable.right,stable.bottom])) {
    action('move', dockHandle, { X:1000,Y:400,Width:1000,Height:1250 }); await pause(3500);
    before = action('info', dockHandle); await pause(450);
    const settled = action('info', dockHandle);
    assert.deepEqual([settled.left,settled.top,settled.right,settled.bottom],[before.left,before.top,before.right,before.bottom]);
  } else before = stable;
  const destination = action('info', target.first);
  requireOwnedTargetEdge(action,target.first,destination,ownedPid);
  assert.equal(destination.visible,true); assert.equal(destination.minimized,false);
  const drag = await view.evaluate('(() => {const r=document.querySelector(".drag").getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2,ratio:devicePixelRatio};})()');
  const x = Math.round(before.clientX + drag.x * drag.ratio), y = Math.round(before.clientY + drag.y * drag.ratio);
  report.drag = action('drag', dockHandle, { X: x, Y: y, EndX: x + destination.right - before.left, EndY: y + destination.top - before.top, OutlinePid: ownedPid });
  const found = await until('Owned project discovery', async () => { const s = await snapshot(); return s.target?.process.pid === target.pid && s.phase === 'selectProject' && s.candidates.some((c) => c.project.appRoot.replaceAll('\\', '/').endsWith('/examples/native-counter')) && s; }, 20000);
  const candidate = found.candidates.find((c) => c.project.appRoot.replaceAll('\\', '/').endsWith('/examples/native-counter'));
  await clickControl(`[...document.querySelectorAll(".candidate-list button")].find(b=>b.querySelector("code")?.textContent===${JSON.stringify(candidate.project.appRoot)})`);
  const associated = await snapshot(); projectId = candidate.project.id; originalTrust = associated.trusted;
  if (!originalTrust) await clickControl('[...document.querySelectorAll("dialog button")].find(b=>b.textContent.trim()==="信任此工程")');
  let running = await until('Real SDK frame', async () => { const s = await snapshot(); if (['protocolBlocked', 'sdkMissing', 'crashed'].includes(s.phase)) throw new Error(s.backend.reason + JSON.stringify(s.diagnostics.slice(-2))); return s.phase === 'running' && s.sessionId && s.build.displayedGeneration === s.build.generation && s; }, 150000);
  assert.equal(running.backend.sdkVersion, '26.0.0.105');
  const receipt = running.diagnostics.findLast((d) => d.code === 'NATIVE_RUNTIME' && d.buildId === running.build.buildId && d.message.startsWith('BUILD_COMPLETED:'));
  const directory = receipt?.message.split('; logs ')[1];
  assert.ok(directory?.includes('ArkDock-native')); currentLog = join(directory, 'previewer.log');
  if (phase === 'stress') {
    const previousGeneration = running.build.generation;
    assert.ok(source.includes('console.info(\'ARKDOCK_NATIVE_REVISION=FIRST\');'));
    const timerSource = source.replace('console.info(\'ARKDOCK_NATIVE_REVISION=FIRST\');', `console.info('ARKDOCK_NATIVE_REVISION=FIRST');\n    setInterval(() => {\n      this.count += 1;\n      console.info(\`ARKDOCK_NATIVE_FRAME_STRESS=\${this.count}\`);\n    }, 50);`);
    sourceChanged = true; writeFileSync(sourcePath, timerSource);
    running = await until('Real timer source rebuild', async () => {
      const state = await snapshot();
      if (state.build.generation > previousGeneration && state.build.error) throw new Error(`Actual SDK timer build failed: ${state.build.error}`);
      return state.phase === 'running' && state.build.generation > previousGeneration && state.build.displayedGeneration === state.build.generation && state;
    }, 150000);
    const timerReceipt = running.diagnostics.findLast((d) => d.code === 'NATIVE_RUNTIME' && d.buildId === running.build.buildId && d.message.startsWith('BUILD_COMPLETED:'));
    currentLog = join(timerReceipt.message.split('; logs ')[1], 'previewer.log');
    await until('Real ArkUI timer executing', () => /ARKDOCK_NATIVE_FRAME_STRESS=/.test(readFileSync(currentLog, 'utf8')));
  }
  await until('Actual visible native surface', () => view.evaluate('!!document.querySelector(".native-frame") && document.querySelector("[data-testid=native-screen]")?.disabled === false'));
  await view.send('Performance.enable');
  const beforeMetrics = await view.send('Performance.getMetrics');
  await view.evaluate(`(async () => {
    const perf = window.__arkdockPerf = {clicks:[],invokes:[],frames:[],presented:[],mutations:[],mode:${JSON.stringify(phase)}};
    const now=()=>performance.now();
    const originalBridge=window.__TAURI__,original=originalBridge.core.invoke;
    const tracedInvoke=async function(command,args) {
      const start=now();const record={command,start,args:command==='click_preview'?args:undefined};
      if(command==='click_preview'||command==='read_preview_frame')perf.invokes.push(record);
      try {const result=await original(command,args);record.end=now();return result;}catch(error){record.end=now();record.error=String(error);throw error;}
    };
    window.__TAURI__={...originalBridge,core:{...originalBridge.core,invoke:tracedInvoke}};
    const click=(event)=>{if(event.target.closest('[data-testid="native-screen"]')){const r=document.querySelector('.native-frame').getBoundingClientRect();perf.clicks.push({at:now(),epoch:Date.now(),clientX:event.clientX,clientY:event.clientY,detail:event.detail,bounds:{left:r.left,top:r.top,width:r.width,height:r.height}});}};
    const presented=(event)=>{
      const detail=event.detail;
      requestAnimationFrame(()=>{const canvas=document.querySelector('canvas.native-frame');if(canvas?.dataset.frameSequence===String(detail.sequence)&&canvas?.dataset.buildId===detail.buildId)perf.presented.push({...detail,at:now()});});
    };
    document.addEventListener('click',click,true);window.addEventListener('arkdock:frame-presented',presented);
    const off=await window.__TAURI__.event.listen('arkdock:frame',({payload:f})=>{
      const record={sessionId:f.sessionId,buildId:f.buildId,generation:f.generation,sequence:f.sequence,capturedAt:f.capturedAt,received:now(),receivedEpoch:Date.now(),payloadBytes:f.pngBase64.length};perf.frames.push(record);
      const src='data:image/png;base64,'+f.pngBase64;
      requestAnimationFrame(async()=>{const image=document.querySelector('img.native-frame');if(!image||image.src!==src)return;const start=now();try{await image.decode();}catch{return;}if(image.src!==src)return;record.decodeMs=now()-start;requestAnimationFrame(()=>{if(image.src===src)perf.presented.push({...record,at:now(),decodeMs:record.decodeMs,format:'png'});});});
    });
    const root=document.querySelector('.frame-viewport');
    const observer=new MutationObserver((records)=>{for(const r of records)if(r.type==='childList'&&(Array.from(r.addedNodes).some(n=>n.nodeType===1&&(n.matches?.('.device-shell')||n.querySelector?.('.device-shell')))||Array.from(r.removedNodes).some(n=>n.nodeType===1&&(n.matches?.('.device-shell')||n.querySelector?.('.device-shell')))))perf.mutations.push({at:now(),kind:'device-shell-remount'});});
    observer.observe(root,{childList:true,subtree:true});
    perf.cleanup=()=>{off();observer.disconnect();document.removeEventListener('click',click,true);window.removeEventListener('arkdock:frame-presented',presented);window.__TAURI__=originalBridge;};
  })()`);
  const startingCount = Number([...readFileSync(currentLog, 'utf8').matchAll(/ARKDOCK_NATIVE_COUNT=(\d+)/g)].at(-1)?.[1] ?? 0);
  if (phase === 'stress') {
    await until('New stress frame displayed', () => view.evaluate(`document.querySelector('canvas.native-frame')?.dataset.buildId===${JSON.stringify(running.build.buildId)}`));
    await view.evaluate('(()=>{window.__arkdockStressSurface=document.querySelector("canvas.native-frame");return true;})()');
    report.resizeSamples = [];
    const stressStartedAt = Date.now();
    const density = (await view.evaluate('devicePixelRatio')) || 1;
    action('move', target.first, { X: 40, Y: 0, Width: 700, Height: Math.round(800 * density) });
    for (const heightDip of [640, 720, 800]) {
      const currentWindow = action('info', dockHandle);
      action('move', dockHandle, { X: currentWindow.left, Y: 0, Width: Math.round(500 * density), Height: Math.round(heightDip * density) });
      await pause(1100);
      const sample = await view.evaluate(`(() => {const c=document.querySelector('canvas.native-frame'),r=c.getBoundingClientRect();return {sameNode:c===window.__arkdockStressSurface,width:r.width,height:r.height,backingWidth:c.width,backingHeight:c.height,sequence:Number(c.dataset.frameSequence),buildId:c.dataset.buildId,status:c.dataset.renderStatus,dpi:devicePixelRatio};})()`);
      assert.equal(sample.sameNode, true); assert.equal(sample.status, 'displayed'); assert.equal(sample.buildId, running.build.buildId);
      assert.ok(sample.backingWidth > 1 && sample.backingHeight > 1);
      sample.requestedHeightDip = heightDip; sample.actualWindow = action('info', dockHandle); report.resizeSamples.push(sample);
    }
    const samples = await view.evaluate('window.__arkdockPerf.presented');
    assert.ok(samples.length >= 20, `Actual high-frequency ArkUI frames required; received ${samples.length}`);
    assert.ok(samples.every((sample) => sample.buildId === running.build.buildId && sample.generation === running.build.generation));
    assert.ok(samples.every((sample, index) => index === 0 || sample.sequence > samples[index - 1].sequence));
    assert.ok(new Set(report.resizeSamples.map((sample) => `${sample.width}:${sample.height}`)).size > 1, 'Actual owned ArkDock resizing must change its canvas rectangle');
    report.stress = { timerIntervalMs: 50, durationMs: Date.now() - stressStartedAt, presentedSamples: samples.length, sequenceStrictlyIncreasing: true, canvasIdentityPreserved: true, finalPresenterDiscarded: samples.at(-1).discarded };
    const sequenceBeforeMinimize = report.resizeSamples.at(-1).sequence;
    await invoke('minimize_window');
    // Native visibility is observed at a bounded rate; WebView2's DOM hidden flag
    // can remain false while the actual Windows window is iconic.
    await pause(900);
    const hiddenStart = await view.evaluate('({hidden:document.hidden,presented:window.__arkdockPerf.presented.length})');
    const nativeHiddenStart = action('info', dockHandle);
    await pause(550);
    const hiddenEnd = await view.evaluate('({hidden:document.hidden,presented:window.__arkdockPerf.presented.length})');
    const nativeHiddenEnd = action('info', dockHandle);
    action('restore', dockHandle); action('foreground', dockHandle);
    await until('New native frame after owned window restore', () => view.evaluate(`Number(document.querySelector('canvas.native-frame')?.dataset.frameSequence)>${sequenceBeforeMinimize} && !document.hidden`));
    report.minimizeResume = { nativeHiddenStart, nativeHiddenEnd, hiddenAtStart: hiddenStart.hidden, hiddenAtEnd: hiddenEnd.hidden, presentedWhileHidden: hiddenEnd.presented - hiddenStart.presented,
      resumed: await view.evaluate('document.querySelector("canvas.native-frame")===window.__arkdockStressSurface && document.querySelector("canvas.native-frame")?.dataset.renderStatus==="displayed"') };
    assert.equal(nativeHiddenStart.minimized, true, 'Actual owned native window must be minimized');
    assert.equal(nativeHiddenEnd.minimized, true, 'Owned native window must remain minimized until restore');
    assert.equal(hiddenEnd.presented, hiddenStart.presented, 'Hidden preview must pause presentation');
    assert.equal(report.minimizeResume.resumed, true, 'Restore must reuse Canvas and resume current native frames');
  } else for (let index = 0; index < 12; index++) {
    await until('Native input surface available', () => view.evaluate('document.querySelector("[data-testid=native-screen]")?.disabled === false'));
    const previous = await view.evaluate('window.__arkdockPerf.presented.length');
    const point = await view.evaluate('(() => {const e=document.querySelector(".native-frame");e.scrollIntoView({block:"center"});const r=e.getBoundingClientRect();return {x:r.left+r.width/2,y:r.top+r.height*360/640};})()');
    if (diagnostics) {
    report.lastClickDiagnostics = await view.evaluate(`(async() => {
      const canvas=document.querySelector('.native-frame'),button=document.querySelector('[data-testid=native-screen]');
      const key=Object.keys(canvas).find(k=>k.startsWith('__reactFiber'));let fiber=key?canvas[key]:null,store=null,app=null,stage=null;
      while(fiber){store??=fiber.memoizedProps?.store;if(fiber.type?.name==='App')app=fiber;if(fiber.type?.name==='PreviewStage')stage=fiber;fiber=fiber.return;}
      const displayed=store?.getDisplayed?.();
      window.__arkdockPerfHandler=stage?.memoizedProps.onClick;
      const {deriveUiState,frameMetadataValid,mapFramePoint}=await import('/src/view-state.ts');
      const hooks=[];let hook=app?.memoizedState;
      while(hook){const value=hook.memoizedState;if(value&&typeof value==='object'&&'current' in value){const current=value.current;hooks.push({ref:current===null?null:typeof current==='object'?current.sessionId?{sessionId:current.sessionId,phase:current.phase,build:current.build}:current.getMode?{controllerMode:current.getMode()}:typeof current:current});}else hooks.push({value:typeof value==='object'?(value?.phase?{phase:value.phase}:typeof value):value});hook=hook.next;}
      const r=canvas.getBoundingClientRect();const p=${JSON.stringify(point)};
      const propsKey=Object.keys(button).find(k=>k.startsWith('__reactProps'));
      return {point:p,bounds:{left:r.left,top:r.top,width:r.width,height:r.height},disabled:button.disabled,reactDisabled:button[propsKey]?.disabled,displayed:displayed?{sessionId:displayed.sessionId,buildId:displayed.buildId,generation:displayed.generation,sequence:displayed.sequence,width:displayed.width,height:displayed.height,jpegArrayBuffer:displayed.jpeg instanceof ArrayBuffer,jpegLength:displayed.jpeg?.byteLength,payloadLength:displayed.payloadLength}:null,canvas:canvas.outerHTML,mode:document.querySelector('[aria-label="选择 ArkUI 元素"]')?.getAttribute('aria-pressed'),hit:document.elementFromPoint(p.x,p.y)?.outerHTML.slice(0,220),inert:!!button.closest('[inert]'),appHooks:hooks,stageSnapshot:stage?.memoizedProps.snapshot,metadataValid:displayed&&frameMetadataValid(displayed),derived:stage&&deriveUiState(stage.memoizedProps.snapshot,displayed),mapped:displayed&&mapFramePoint(displayed,r,p.x,p.y),surfaceHandler:button[propsKey]?.onClick.toString(),stageHandler:stage?.memoizedProps.onClick.toString()};
    })()`);
    report.actualInputClosure = await capturedInputScope();
    }
    await view.send('Input.dispatchMouseEvent', { type: 'mousePressed', ...point, button: 'left', clickCount: 1 });
    await view.send('Input.dispatchMouseEvent', { type: 'mouseReleased', ...point, button: 'left', clickCount: 1 });
    if (diagnostics) await pause(150);
    if (diagnostics && !await view.evaluate('window.__arkdockPerf.invokes.some(x=>x.command==="click_preview")')) {
      report.directHandlerDiagnostic = await view.evaluate(`(() => {const button=document.querySelector('[data-testid=native-screen]');const key=Object.keys(button).find(k=>k.startsWith('__reactProps'));button[key].onClick({currentTarget:button,detail:1,clientX:${point.x},clientY:${point.y}});const canvas=button.querySelector('canvas');const fk=Object.keys(canvas).find(k=>k.startsWith('__reactFiber'));let f=canvas[fk],stage=null;while(f){if(f.type?.name==='PreviewStage')stage=f;f=f.return;}stage.memoizedProps.onClick(stage.memoizedProps.store.getDisplayed(),180,360);return window.__arkdockPerf.invokes.filter(x=>x.command==='click_preview');})()`);
      throw new Error('Actual pointer click did not invoke native input; direct handler diagnostic recorded separately');
    }
    await until('Real ArkUI click response', () => Number([...readFileSync(currentLog, 'utf8').matchAll(/ARKDOCK_NATIVE_COUNT=(\d+)/g)].at(-1)?.[1] ?? 0) >= startingCount + index + 1);
    await until('Native response displayed', () => view.evaluate(`window.__arkdockPerf.presented.length>${previous}`));
    await pause(60);
  }
  const timeline = await view.evaluate('(() => {const {cleanup,...data}=window.__arkdockPerf;return data;})()');
  const afterMetrics = await view.send('Performance.getMetrics');
  const clicks = timeline.clicks.map((click) => {
    const command = timeline.invokes.find((item) => item.command === 'click_preview' && item.start >= click.at);
    const frame = timeline.presented.find((item) => item.at > (command?.start ?? Infinity) && item.capturedAt >= click.epoch);
    return { clickAt: click.at, inputToCommandMs: command ? command.start - click.at : null, commandRoundTripMs: command?.end ? command.end - command.start : null, inputToPresentedMs: frame ? frame.at - click.at : null, receiveToPresentedMs: frame?.received != null ? frame.at - frame.received : null, decodeMs: frame?.decodeMs ?? null };
  });
  report.measurements = Object.fromEntries(['inputToCommandMs', 'commandRoundTripMs', 'inputToPresentedMs', 'receiveToPresentedMs', 'decodeMs'].map((key) => [key, summary(clicks.map((c) => c[key]).filter(Number.isFinite))]));
  if (phase !== 'stress') assert.equal(report.measurements.inputToPresentedMs.samples, 12, 'All real clicks need a matched display timing sample');
  report.clicks = clicks; report.timeline = timeline;
  const get = (metrics, name) => metrics.metrics.find((item) => item.name === name)?.value ?? null;
  report.webviewMetrics = { taskDurationDeltaSeconds: get(afterMetrics, 'TaskDuration') - get(beforeMetrics, 'TaskDuration'), jsHeapUsedBeforeBytes: get(beforeMetrics, 'JSHeapUsedSize'), jsHeapUsedAfterBytes: get(afterMetrics, 'JSHeapUsedSize') };
  report.nativeRenderInternalMs = null; report.nativeCpuPercent = null;
  report.notes = ['capturedAt is Rust receive time, not an engine render timestamp', 'DOM decode + requestAnimationFrame measures browser scheduling; it does not measure monitor photon latency', 'TaskDuration is WebView task CPU time, not Previewer or total process CPU', 'Unexposed native internal render timing and CPU percentage remain unmeasured'];
  const end = await snapshot();
  assert.equal(end.sessionId, running.sessionId); assert.equal(end.build.buildId, running.build.buildId); assert.equal(end.build.generation, running.build.generation);
  assert.equal(timeline.mutations.length, 0, 'Ordinary click must preserve device shell');
  checks.push({ status: 'PASS', name: phase === 'stress' ? 'Actual 50ms ArkUI state timer, >=20 frames, strictly increasing sequence, owned ArkDock window resizing and stable Canvas identity' : '12 actual ArkUI clicks, matching native logs and displayed frames; session/build retained; no device-shell remount' });
  const screenshot = await view.send('Page.captureScreenshot', { format: 'png' });
  mkdirSync('artifacts', { recursive: true }); writeFileSync(`artifacts/preview-performance-${phase}.png`, Buffer.from(screenshot.data, 'base64'));
} catch (error) {
  if (view) { try { report.failureTimeline = await view.evaluate('(() => {const {cleanup,...data}=window.__arkdockPerf??{};return data;})()'); report.failureSnapshot = await snapshot(); report.failureDom = await view.evaluate('document.body.innerText'); } catch { /* Report the primary failure. */ } }
  checks.push({ status: error.blocked ? 'BLOCKED' : 'FAIL', name: error.message }); process.exitCode = error.blocked ? 2 : 1;
} finally {
  if (view) {
    try { await view.evaluate('window.__arkdockPerf?.cleanup()'); if (sourceChanged) await invoke('stop_preview'); if (projectId && !originalTrust) await invoke('set_trust', { projectId, trusted: false }); if (host) await invoke('detach'); }
    catch (error) { checks.push({ status: 'FAIL', name: `Owned probe cleanup: ${error.message}` }); process.exitCode = 1; }
    view.close();
  }
  if (sourceChanged) writeFileSync(sourcePath, source);
  if (host && host.exitCode === null) host.kill();
  report.status = checks.some((c) => c.status === 'FAIL') ? 'FAIL' : checks.some((c) => c.status === 'BLOCKED') ? 'BLOCKED' : 'PASS';
  mkdirSync('artifacts', { recursive: true }); writeFileSync(`artifacts/preview-performance-${phase}.json`, `${JSON.stringify(report, null, 2)}\n`);
  console.log(`${report.status} preview performance ${phase}: ${JSON.stringify(report.measurements ?? checks)}`);
}
