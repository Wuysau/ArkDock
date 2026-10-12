// D1/D2: actual ArkDock WebView2, actual SDK build/Previewer, real .ets changes.
import assert from 'node:assert/strict';
import { spawn, execFileSync } from 'node:child_process';
import { readFileSync, writeFileSync, mkdirSync, existsSync } from 'node:fs';
import { resolve, join } from 'node:path';
import { createHash } from 'node:crypto';
import { connectWebview } from './webview-driver.mjs';

const checks = [];
const driverDeadline = Date.now() + 15 * 60_000;
const pause = (ms) => new Promise((done) => setTimeout(done, ms));
const project = resolve('../ArkDockTodoDemo');
const sourcePath = join(project, 'entry/src/main/ets/pages/Index.ets');
const source = readFileSync(sourcePath, 'utf8');
let host;
let view;
let originalTrust = false;
let projectId;
let sourceChanged = false;
let currentLog;
const blocked = (message) => { const error = new Error(message); error.blocked = true; throw error; };
const hash = (bytes) => createHash('sha256').update(bytes).digest('hex');
function action(kind, handle, values = {}) {
  const args = ['-NoProfile', '-File', resolve('tests/native/window-actions.ps1'), '-Action', kind, '-Handle', String(handle)];
  for (const [key, value] of Object.entries(values)) args.push(`-${key}`, String(value));
  return JSON.parse(execFileSync('pwsh', args, { encoding: 'utf8', windowsHide: true, timeout: 12_000 }));
}
const snapshot = () => view.evaluate('window.__TAURI__.core.invoke("snapshot")');
async function invoke(command, args = {}) {
  const state = await snapshot();
  return view.evaluate(`window.__TAURI__.core.invoke(${JSON.stringify(command)}, ${JSON.stringify({ ...args, scope: { targetId: state.target?.id ?? null, sessionId: state.sessionId } })})`);
}
async function until(name, test, timeout = 150_000) {
  const deadline = Math.min(Date.now() + timeout, driverDeadline);
  while (Date.now() < deadline) {
    const state = await snapshot();
    if (await test(state)) return state;
    if (['crashed','protocolBlocked','sdkMissing'].includes(state.phase)) throw new Error(`${name}: ${JSON.stringify(state.diagnostics.slice(-4))}`);
    await pause(100);
  }
  throw new Error(`${name}: timed out; ${JSON.stringify(await snapshot())}`);
}
async function check(id, name, execute) {
  await execute(); checks.push({ id, status: 'PASS', name }); console.log(`PASS ${id} ${name}`);
}
async function clickUiControl(expression, name) {
  let point;
  await until(name, async () => {
    point = await view.evaluate(`(() => { const button = ${expression}; if (!button || button.disabled) return null; button.scrollIntoView({block:'center'}); const r = button.getBoundingClientRect(); return {x:r.x+r.width/2,y:r.y+r.height/2}; })()`);
    return Boolean(point);
  }, 10_000);
  await view.send('Input.dispatchMouseEvent',{type:'mousePressed',...point,button:'left',clickCount:1});
  await view.send('Input.dispatchMouseEvent',{type:'mouseReleased',...point,button:'left',clickCount:1});
}
async function trustThroughUi() {
  // After a revocation the trust sheet is not reopened automatically; the empty
  // state offers the primary confirmation entry instead.
  const granted = await clickUiControl('[...document.querySelectorAll("dialog button")].find(button => button.textContent.trim() === "信任此工程")','Actual execution trust confirmation').catch(() => null);
  if (!granted) {
    await clickUiControl('[...document.querySelectorAll("button")].find(button => button.textContent.trim() === "确认工程信任")','Actual trust entry');
    await clickUiControl('[...document.querySelectorAll("dialog button")].find(button => button.textContent.trim() === "信任此工程")','Actual trust grant');
  }
}
async function frame(name) {
  const data = await view.evaluate('(() => {const canvas=document.querySelector("canvas.native-frame");return canvas?.dataset.frameSequence ? canvas.toDataURL("image/png").split(",")[1] : null;})()');
  assert.ok(data, 'Actual native frame must be delivered to the UI');
  const bytes = Buffer.from(data, 'base64');
  writeFileSync(`artifacts/native-${name}.png`, bytes);
  return hash(bytes);
}
function logFrom(state) {
  const receipt = state.diagnostics.filter((d) => d.code === 'NATIVE_RUNTIME' && d.buildId === state.build.buildId && d.message.startsWith('BUILD_COMPLETED:')).at(-1);
  assert.ok(receipt, 'Real completed SDK build receipt required');
  const directory = receipt.message.split('; logs ')[1];
  assert.ok(directory && directory.includes('ArkDock-native'), 'Owned native runtime log required');
  return join(directory, 'previewer.log');
}
try {
  if (process.platform !== 'win32' || !process.env.ARKDOCK_SDK) blocked('Windows and explicit ARKDOCK_SDK required');
  const gui = JSON.parse(execFileSync('pwsh', ['-NoProfile','-Command','ConvertTo-Json -InputObject @(Get-Process -Name deveco64,deveco -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Id)'], { encoding: 'utf8', windowsHide: true }));
  if (gui.length) blocked('Close DevEco Studio GUI before native acceptance; no external process was stopped');
  checks.push({ id: 'D1', status: 'PASS', name: 'DevEco Studio GUI closed' });
  try { view = await connectWebview(); } catch { blocked('Start pnpm dev with ARKDOCK_SDK and WebView2 development debug port 9223 (see runbook)'); }
  // Dev delivers frames through the app-owned binary mailbox; observe its reader for the IPC receipt.
  await view.evaluate(`(async()=>{
    const original=window.__TAURI__, {parseBinaryFrame}=await import('/src/preview/frame-decoder.ts');
    const state=await original.core.invoke('snapshot');
    const trace=window.__todoAuditBinary={latest:null,original};
    window.__TAURI__={...original,core:{...original.core,invoke:async(command,args)=>{
      const value=await original.core.invoke(command,args);
      if(command==='read_preview_frame'&&value instanceof ArrayBuffer&&value.byteLength){
        const frame=parseBinaryFrame(value,state);
        if(frame){let encoded='';const bytes=new Uint8Array(frame.jpeg);for(let i=0;i<bytes.length;i+=16384)encoded+=String.fromCharCode(...bytes.subarray(i,i+16384));trace.latest={...frame,jpeg:undefined,jpegBase64:btoa(encoded)};}
      }
      return value;
    }}};
  })()`);
  const initial = await snapshot();
  if (!initial.backend.available) blocked(initial.backend.reason);
  const dock = JSON.parse(execFileSync('pwsh', ['-NoProfile','-Command','Get-Process -Name arkdock | Where-Object { $_.MainWindowHandle -ne 0 -and $_.Path -eq (Join-Path (Get-Location) "src-tauri/target/debug/arkdock.exe") } | Select-Object -First 1 Id | ConvertTo-Json -Compress'], { encoding: 'utf8', windowsHide: true }));
  if (!dock) blocked('The current workspace debug ArkDock window is required; an installed application is not a test target');
  host = spawn('pwsh', ['-NoProfile','-File',resolve('tests/native/window-host.ps1'),'-ProjectPath',project], { windowsHide: true, stdio: ['ignore','pipe','pipe'] });
  const target = await new Promise((done, reject) => {
    let output = ''; let errors = '';
    const timer = setTimeout(() => reject(new Error(`Fixture startup timeout: ${errors}`)), 10_000);
    host.stderr.on('data', (data) => { errors += data; });
    host.stdout.on('data', (data) => { output += data; if (output.includes('\n')) { clearTimeout(timer); done(JSON.parse(output.split('\n')[0])); } });
    host.once('exit', (code) => { clearTimeout(timer); reject(new Error(`Host exited ${code}`)); });
  });
  const dockHandle = action('find', 0, { ProcessId: dock.Id }).handle;
  await invoke('detach');
  action('restore', dockHandle); await pause(200);
  action('move', target.first, { X:40,Y:400,Width:700,Height:700 });
  action('move', target.second, { X:50,Y:520,Width:400,Height:420 });
  action('move', dockHandle, { X:1000,Y:400,Width:1000,Height:1250 });
  // Bring our target above unrelated desktop windows before placing ArkDock above it.
  action('foreground', target.first); await pause(200);
  action('foreground', dockHandle); await pause(250);
  const before = action('info', dockHandle); const destination = action('info', target.first);
  const button = await view.evaluate('(() => { const r=document.querySelector(".drag").getBoundingClientRect(); return { x:r.x+r.width/2,y:r.y+r.height/2,ratio:devicePixelRatio }; })()');
  const x = Math.round(before.clientX + button.x*button.ratio); const y = Math.round(before.clientY + button.y*button.ratio);
  const dragged = action('drag', dockHandle, { X:x,Y:y,EndX:x+destination.right-before.left,EndY:y+destination.top-before.top,OutlinePid:dock.Id });
  writeFileSync('artifacts/native-drag.json',JSON.stringify({before,destination,button,dragged},null,2));
  const selected = await until('Actual target and ArkTS candidate discovery', (state) => state.target?.process.pid === target.pid && state.phase === 'selectProject' && state.candidates.some((c) => c.project.appRoot.replaceAll('\\','/').endsWith('/ArkDockTodoDemo')), 15_000);
  const candidate = selected.candidates.find((c) => c.project.appRoot.replaceAll('\\','/').endsWith('/ArkDockTodoDemo'));
  assert.equal(candidate.autoEligible, false);
  await clickUiControl(`[...document.querySelectorAll(".candidate-list button")].find(button => button.querySelector("code")?.textContent === ${JSON.stringify(candidate.project.appRoot)})`,'Actual candidate selection');
  const associated = await until('Workspace associated through the actual candidate control', (state) => state.project?.id === candidate.project.id && state.sessionId, 10_000);
  projectId = associated.project.id; originalTrust = associated.trusted;
  if (!originalTrust) await trustThroughUi();
  let running;
  await check('D1', 'Actual SDK build, Previewer and PNG delivery into ArkDock', async () => {
    running = await until('Build and first native frame', (state) => state.phase === 'running' && state.build.displayedGeneration === state.build.generation);
    assert.equal(running.backend.sdkVersion, '26.0.0.105');
    assert.equal(running.backend.canClick, true);
    currentLog = logFrom(running);

    await frame('before-click');
    const initialScreenshot = await view.send('Page.captureScreenshot',{format:'png'});
    writeFileSync('artifacts/native-first-frame-window.png',Buffer.from(initialScreenshot.data,'base64'));
  });

  await check('TODO_CLICK', 'Real Todo status button changes ArkTS count and native pixels', async () => {
    const before = await frame('todo-before-click');
    const point = await view.evaluate('(() => { const r=document.querySelector(".native-frame").getBoundingClientRect(); return {x:r.left+r.width*48/360,y:r.top+r.height*495/640}; })()');
    await view.send('Input.dispatchMouseEvent', {type:'mousePressed', ...point, button:'left', clickCount:1});
    await view.send('Input.dispatchMouseEvent', {type:'mouseReleased', ...point, button:'left', clickCount:1});
    await until('Actual Todo ArkTS state change', () => /ARKDOCK_TODO_COMPLETED=[13]/.test(readFileSync(currentLog, 'utf8')), 120_000);
    await pause(500);
    const after = await frame('todo-after-click');
    assert.notEqual(after, before, 'Actual native pixels must change after the ArkTS handler');
    const current = await snapshot();
    writeFileSync('artifacts/todo-frame-receipt.json', JSON.stringify({
      sessionId: current.sessionId, build: current.build, backend: current.backend,
      beforeSha256: before, afterSha256: after,
      canvasCount: await view.evaluate('document.querySelectorAll("canvas").length'),
      canvasFormat: await view.evaluate('document.querySelector("canvas.native-frame")?.dataset.format'),
      canvasSequence: await view.evaluate('document.querySelector("canvas.native-frame")?.dataset.frameSequence'),
      ipcFrame: await view.evaluate('window.__todoAuditBinary?.latest ?? null'),
      runtimeLog: currentLog,
    }, null, 2));
  });
} catch (error) {
  checks.push({ id:'D1/D2',status:error.blocked?'BLOCKED':'FAIL',name:error.message });
  console.error(`${error.blocked?'BLOCKED':'FAIL'} ${error.message}`);
  process.exitCode = error.blocked ? 2 : 1;
} finally {
  if (view) {
    try { await view.evaluate('(()=>{const trace=window.__todoAuditBinary;if(trace){window.__TAURI__=trace.original;delete window.__todoAuditBinary;}})()'); } catch { /* The owned WebView may already have closed. */ }
    try { if (projectId && !originalTrust) await invoke('set_trust',{projectId,trusted:false}); await invoke('detach'); }
    catch (error) { checks.push({id:'C2',status:'FAIL',name:`Driver cleanup: ${error.message}`}); process.exitCode = 1; }
    view.close();
  }
  if (host && host.exitCode === null) host.kill();
  if (sourceChanged) writeFileSync(sourcePath,source);
  mkdirSync('artifacts',{recursive:true});
  const status = checks.some((c) => c.status === 'FAIL') ? 'FAIL' : checks.some((c) => c.status === 'BLOCKED') ? 'BLOCKED' : 'PASS';
  writeFileSync('artifacts/todo-native-audit.json',`${JSON.stringify({status,capturedAt:new Date().toISOString(),sdkVersion:'26.0.0.105',checks},null,2)}\n`);
  if (currentLog && existsSync(currentLog)) writeFileSync('artifacts/todo-native-previewer.log',readFileSync(currentLog));
}
