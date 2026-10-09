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
const project = resolve('examples/native-counter');
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
async function frame(name) {
  const data = await view.evaluate('document.querySelector(".native-frame")?.src.split(",")[1]');
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
  const initial = await snapshot();
  if (!initial.backend.available) blocked(initial.backend.reason);
  const dock = JSON.parse(execFileSync('pwsh', ['-NoProfile','-Command','Get-Process -Name arkdock | Where-Object MainWindowHandle -NE 0 | Select-Object -First 1 Id | ConvertTo-Json -Compress'], { encoding: 'utf8', windowsHide: true }));
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
  const selected = await until('Actual target and ArkTS candidate discovery', (state) => state.target?.process.pid === target.pid && state.phase === 'selectProject' && state.candidates.some((c) => c.project.appRoot.replaceAll('\\','/').endsWith('/examples/native-counter')), 15_000);
  const candidate = selected.candidates.find((c) => c.project.appRoot.replaceAll('\\','/').endsWith('/examples/native-counter'));
  assert.equal(candidate.autoEligible, false);
  const associated = await invoke('select_candidate', { candidateId:candidate.id });
  projectId = associated.project.id; originalTrust = associated.trusted;
  if (!originalTrust) await invoke('set_trust', { projectId, trusted:true });
  let running;
  await check('D1', 'Actual SDK build, Previewer and PNG delivery into ArkDock', async () => {
    running = await until('Build and first native frame', (state) => state.phase === 'running' && state.build.displayedGeneration === state.build.generation);
    assert.equal(running.backend.sdkVersion, '26.0.0.105');
    assert.equal(running.backend.canClick, true);
    currentLog = logFrom(running);
    assert.match(readFileSync(currentLog,'utf8'), /ARKDOCK_NATIVE_REVISION=FIRST/);
    await frame('before-click');
  });
  await check('B4/C1', 'Revoking trust during real async build cancels it; regrant starts a new session', async () => {
    const oldSession = running.sessionId;
    await invoke('refresh_preview');
    await until('Actual build active', (state) => state.phase === 'building' && state.build.buildId !== running.build.buildId, 10_000);
    await invoke('set_trust', {projectId,trusted:false});
    const revoked = await snapshot();
    assert.equal(revoked.trusted,false);
    assert.equal(revoked.phase,'untrusted');
    assert.equal(revoked.sessionId,null);
    await assert.rejects(invoke('refresh_preview'),/TRUST_REQUIRED/);
    await invoke('set_trust', {projectId,trusted:true});
    running = await until('Real rebuild after regrant', (state) => state.phase === 'running' && state.sessionId !== oldSession && state.build.displayedGeneration === state.build.generation);
    currentLog = logFrom(running);
    await until('Regrant image in React', async () => (await view.evaluate('document.querySelector(".native-frame")?.alt'))?.includes(running.build.buildId), 10_000);
  });
  await check('D2', 'Click on ArkDock image changes actual ArkUI component count', async () => {
    const previous = await frame('before-click');
    const point = await view.evaluate('(() => { const image=document.querySelector(".native-frame"); image.scrollIntoView({block:"center"}); const r=image.getBoundingClientRect(); return {x:r.left+r.width/2,y:r.top+r.height*360/640}; })()');
    await view.send('Input.dispatchMouseEvent',{type:'mousePressed',...point,button:'left',clickCount:1});
    await view.send('Input.dispatchMouseEvent',{type:'mouseReleased',...point,button:'left',clickCount:1});
    await until('Actual native count log', () => /ARKDOCK_NATIVE_COUNT=1/.test(readFileSync(currentLog,'utf8')), 10_000);
    await pause(400);
    assert.notEqual(await frame('after-click'), previous);
    await assert.rejects(invoke('click_preview',{buildId:'old-task',generation:0,x:180,y:360}), /STALE_INPUT/);
  });
  await check('D2', 'Editing real .ets triggers watcher, serial rebuild and new native generation', async () => {
    const previousGeneration = running.build.generation;
    const previous = await frame('after-click');
    assert.ok(source.includes('Source revision: FIRST') && source.includes('ARKDOCK_NATIVE_REVISION=FIRST'));
    sourceChanged = true;
    writeFileSync(sourcePath,source.replaceAll('REVISION=FIRST','REVISION=SECOND').replace('Source revision: FIRST','Source revision: SECOND'));
    running = await until('Source rebuild native frame', (state) => state.phase === 'running' && state.build.generation > previousGeneration && state.build.displayedGeneration === state.build.generation);
    currentLog = logFrom(running);
    assert.match(readFileSync(currentLog,'utf8'), /ARKDOCK_NATIVE_REVISION=SECOND/);
    await until('New build image rendered into React', async () => (await view.evaluate('document.querySelector(".native-frame")?.alt'))?.includes(running.build.buildId), 10_000);
    assert.notEqual(await frame('source-updated'),previous);
    await view.evaluate('document.querySelector(".preview").scrollIntoView({block:"start"})');
    const screenshot = await view.send('Page.captureScreenshot',{format:'png'});
    writeFileSync('artifacts/native-arkdock-window.png',Buffer.from(screenshot.data,'base64'));
  });
  await check('C1/C2', 'Actual compile failure preserves stale picture and valid .ets recovers', async () => {
    const valid = readFileSync(sourcePath,'utf8');
    const previous = await frame('source-updated');
    const previousGeneration = running.build.generation;
    writeFileSync(sourcePath, valid.replace('struct Index {','struct Index ??? {'));
    const failed = await until('Real ArkTS compiler failure', (state) => state.phase === 'stale' && state.build.generation > previousGeneration && state.build.error);
    assert.match(failed.build.error,/BUILD_FAILED/);
    assert.equal(failed.build.stale,true);
    assert.equal(await frame('compile-failed-old-picture'),previous);
    const staleScreenshot = await view.send('Page.captureScreenshot',{format:'png'});
    writeFileSync('artifacts/native-compile-failed-window.png',Buffer.from(staleScreenshot.data,'base64'));
    await assert.rejects(invoke('click_preview',{buildId:failed.build.buildId,generation:failed.build.generation,x:180,y:360}), /STALE_INPUT/);
    writeFileSync(sourcePath,valid);
    running = await until('Native recovery after actual source repair',(state) => state.phase === 'running' && state.build.generation > failed.build.generation && !state.build.stale);
    currentLog = logFrom(running);
    assert.match(readFileSync(currentLog,'utf8'),/ARKDOCK_NATIVE_REVISION=SECOND/);
    await until('Recovered build image rendered into React', async () => (await view.evaluate('document.querySelector(".native-frame")?.alt'))?.includes(running.build.buildId), 10_000);
    const recoveredScreenshot = await view.send('Page.captureScreenshot',{format:'png'});
    writeFileSync('artifacts/native-recovered-window.png',Buffer.from(recoveredScreenshot.data,'base64'));
  });
  await check('C2', 'Stop clears session and terminates only the owned Previewer job', async () => {
    const processes = JSON.parse(execFileSync('pwsh',['-NoProfile','-Command','@(Get-CimInstance Win32_Process -Filter "Name=\'Previewer.exe\'" | Select-Object ProcessId,ParentProcessId,CommandLine) | ConvertTo-Json -AsArray'],{encoding:'utf8',windowsHide:true}));
    const owned = processes.filter((p) => p.ParentProcessId === dock.Id);
    assert.equal(owned.length,1,'Exact ArkDock-owned Previewer expected');
    await invoke('stop_preview');
    assert.equal((await snapshot()).sessionId,null);
    await pause(300);
    const live = execFileSync('pwsh',['-NoProfile','-Command',`@(Get-Process -Id ${owned[0].ProcessId} -ErrorAction SilentlyContinue).Count`],{encoding:'utf8',windowsHide:true});
    assert.equal(Number(live.trim()),0);
    for (const external of processes.filter((p) => !owned.includes(p))) {
      assert.equal(Number(execFileSync('pwsh',['-NoProfile','-Command',`@(Get-Process -Id ${external.ProcessId} -ErrorAction SilentlyContinue).Count`],{encoding:'utf8',windowsHide:true}).trim()),1);
    }
  });
} catch (error) {
  checks.push({ id:'D1/D2',status:error.blocked?'BLOCKED':'FAIL',name:error.message });
  console.error(`${error.blocked?'BLOCKED':'FAIL'} ${error.message}`);
  process.exitCode = error.blocked ? 2 : 1;
} finally {
  if (view) {
    try { if (projectId && !originalTrust) await invoke('set_trust',{projectId,trusted:false}); await invoke('detach'); }
    catch (error) { checks.push({id:'C2',status:'FAIL',name:`Driver cleanup: ${error.message}`}); process.exitCode = 1; }
    view.close();
  }
  if (host && host.exitCode === null) host.kill();
  if (sourceChanged) writeFileSync(sourcePath,source);
  mkdirSync('artifacts',{recursive:true});
  const status = checks.some((c) => c.status === 'FAIL') ? 'FAIL' : checks.some((c) => c.status === 'BLOCKED') ? 'BLOCKED' : 'PASS';
  writeFileSync('artifacts/native-smoke.json',`${JSON.stringify({status,capturedAt:new Date().toISOString(),sdkVersion:'26.0.0.105',checks},null,2)}\n`);
  if (currentLog && existsSync(currentLog)) writeFileSync('artifacts/native-previewer.log',readFileSync(currentLog));
}
