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
const normalizedPath = (value) => value.replaceAll('\\','/').replace(/^\/\/\?\//,'').toLowerCase();
const sourcePath = join(project, 'entry/src/main/ets/pages/Index.ets');
const source = readFileSync(sourcePath, 'utf8');
const ownedPid = Number(process.env.ARKDOCK_TEST_PID ?? process.env.ARKDOCK_PROBE_PID);
const debugPort = Number(process.env.ARKDOCK_TEST_PORT ?? process.env.ARKDOCK_PROBE_PORT ?? 9223);
const expectedUrl = process.env.ARKDOCK_TEST_URL ?? process.env.ARKDOCK_PROBE_URL ?? 'http://127.0.0.1:1420/';
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
    point = await view.evaluate(`(() => { const button = ${expression}; if (!button || button.disabled) return null; const r = button.getBoundingClientRect(); return {x:r.x+r.width/2,y:r.y+r.height/2}; })()`);
    return Boolean(point);
  }, 10_000);
  await view.send('Input.dispatchMouseEvent',{type:'mousePressed',...point,button:'left',clickCount:1});
  await view.send('Input.dispatchMouseEvent',{type:'mouseReleased',...point,button:'left',clickCount:1});
}
const trustThroughUi = () => clickUiControl('[...document.querySelectorAll("dialog button")].find(button => button.textContent.trim() === "信任此工程")','Actual execution trust confirmation');
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
const nativeCount = () => Number([...readFileSync(currentLog,'utf8').matchAll(/ARKDOCK_NATIVE_COUNT=(\d+)/g)].at(-1)?.[1] ?? 0);
const currentScope = (state) => ({ targetId: state.target?.id ?? null, sessionId: state.sessionId });
try {
  if (process.platform !== 'win32' || !process.env.ARKDOCK_SDK) blocked('Windows and explicit ARKDOCK_SDK required');
  if (!Number.isSafeInteger(ownedPid) || ownedPid <= 0) blocked('ARKDOCK_TEST_PID must identify the dedicated ArkDock process created for this test');
  const gui = JSON.parse(execFileSync('pwsh', ['-NoProfile','-Command','ConvertTo-Json -InputObject @(Get-Process -Name deveco64,deveco -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Id)'], { encoding: 'utf8', windowsHide: true }));
  if (gui.length) blocked('Close DevEco Studio GUI before native acceptance; no external process was stopped');
  checks.push({ id: 'D1', status: 'PASS', name: 'DevEco Studio GUI closed' });
  try { view = await connectWebview(debugPort, 5000, expectedUrl, ownedPid); } catch { blocked('Start the dedicated ArkDock process with ARKDOCK_SDK and configured development debug port (see runbook)'); }
  const initial = await snapshot();
  if (initial.target || initial.sessionId) blocked('Dedicated test process must begin detached; user associations are not modified');
  if (!initial.backend.available) blocked(initial.backend.reason);
  // The existing Windows layout controller may finish an already-accepted detach
  // compensation within three seconds. Do not start a new drag on that old placement.
  await pause(3500);
  const dock = { Id: ownedPid };
  host = spawn('pwsh', ['-NoProfile','-File',resolve('tests/native/window-host.ps1'),'-ProjectPath',project], { windowsHide: true, stdio: ['ignore','pipe','pipe'] });
  const target = await new Promise((done, reject) => {
    let output = ''; let errors = '';
    const timer = setTimeout(() => reject(new Error(`Fixture startup timeout: ${errors}`)), 10_000);
    host.stderr.on('data', (data) => { errors += data; });
    host.stdout.on('data', (data) => { output += data; if (output.includes('\n')) { clearTimeout(timer); done(JSON.parse(output.split('\n')[0])); } });
    host.once('exit', (code) => { clearTimeout(timer); reject(new Error(`Host exited ${code}`)); });
  });
  const dockHandle = action('find', 0, { ProcessId: dock.Id }).handle;
  if (!dockHandle) blocked('Explicitly owned ArkDock window not found');
  action('restore', dockHandle); await pause(200);
  action('move', target.first, { X:40,Y:400,Width:700,Height:700 });
  action('move', target.second, { X:50,Y:520,Width:400,Height:420 });
  action('move', dockHandle, { X:1000,Y:400,Width:1000,Height:1250 });
  // Bring our target above unrelated desktop windows before placing ArkDock above it.
  action('foreground', target.first); await pause(200);
  action('foreground', dockHandle); await pause(250);
  let before = action('info', dockHandle);
  await pause(450);
  const stable = action('info', dockHandle);
  if (JSON.stringify([before.left,before.top,before.right,before.bottom]) !== JSON.stringify([stable.left,stable.top,stable.right,stable.bottom])) {
    action('move', dockHandle, { X:1000,Y:400,Width:1000,Height:1250 }); await pause(3500);
    before = action('info', dockHandle); await pause(450);
    const settled = action('info', dockHandle);
    assert.deepEqual([settled.left,settled.top,settled.right,settled.bottom],[before.left,before.top,before.right,before.bottom],'Owned window placement must settle before native drag');
  } else before = stable;
  const destination = action('info', target.first);
  assert.equal(destination.visible,true,'Owned fixture must be visible');
  assert.equal(destination.minimized,false,'Owned fixture must not be minimized');
  assert.equal(await view.evaluate('!!document.querySelector("[inert]") || !!document.querySelector("dialog")'),false,'Owned drag must not be blocked by a modal');
  const button = await view.evaluate('(() => { const r=document.querySelector(".drag").getBoundingClientRect(); return { x:r.x+r.width/2,y:r.y+r.height/2,ratio:devicePixelRatio }; })()');
  const x = Math.round(before.clientX + button.x*button.ratio); const y = Math.round(before.clientY + button.y*button.ratio);
  const dragged = action('drag', dockHandle, { X:x,Y:y,EndX:x+destination.right-before.left,EndY:y+destination.top-before.top,OutlinePid:dock.Id });
  writeFileSync('artifacts/native-drag.json',JSON.stringify({before,destination,button,dragged},null,2));
  const selected = await until('Actual target and ArkTS candidate discovery', (state) => state.target?.process.pid === target.pid && state.phase === 'selectProject' && state.candidates.some((c) => normalizedPath(c.project.appRoot) === normalizedPath(project)), 15_000);
  const candidate = selected.candidates.find((c) => normalizedPath(c.project.appRoot) === normalizedPath(project));
  assert.equal(candidate.autoEligible, false);
  await clickUiControl(`[...document.querySelectorAll(".candidate-list button")].find(button => button.querySelector("code")?.textContent === ${JSON.stringify(candidate.project.appRoot)})`,'Actual candidate selection');
  const associated = await until('Workspace associated through the actual candidate control', (state) => state.project?.id === candidate.project.id && (state.phase === 'untrusted' || state.sessionId), 10_000);
  projectId = associated.project.id; originalTrust = associated.trusted;
  if (!originalTrust) await trustThroughUi();
  let running;
  await check('D1', 'Actual SDK build, Previewer and binary Canvas delivery into ArkDock', async () => {
    running = await until('Build and first native frame', (state) => state.project?.id === projectId && state.sessionId && state.phase === 'running' && state.build.displayedGeneration === state.build.generation);
    assert.equal(running.backend.sdkVersion, '26.0.0.105');
    assert.equal(running.backend.canClick, true);
    currentLog = logFrom(running);
    assert.match(readFileSync(currentLog,'utf8'), /ARKDOCK_NATIVE_REVISION=FIRST/);
    await until('Native frame actually decoded and displayed', async () => (await view.evaluate('document.querySelector("canvas.native-frame")?.dataset.buildId')) === running.build.buildId, 10000);
    await frame('before-click');
    const initialScreenshot = await view.send('Page.captureScreenshot',{format:'png'});
    writeFileSync('artifacts/native-first-frame-window.png',Buffer.from(initialScreenshot.data,'base64'));
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
    await trustThroughUi();
    running = await until('Real rebuild after regrant', (state) => state.phase === 'running' && state.sessionId !== oldSession && state.build.displayedGeneration === state.build.generation);
    currentLog = logFrom(running);
    await until('Regrant image on Canvas', async () => (await view.evaluate('document.querySelector(".native-frame")?.dataset.buildId')) === running.build.buildId, 10_000);
    assert.equal(await view.evaluate('!!document.querySelector("dialog")'),false,'Trust confirmation must release the native input surface');
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
  await check('IPC1', 'App owns binary mailbox; wrong consumer cannot read or disable; Canvas remains interactive', async () => {
    const expected = await snapshot(), scope = currentScope(expected);
    assert.equal(expected.phase, 'running');
    assert.equal(await view.evaluate('document.querySelector("canvas.native-frame")?.dataset.format'), 'jpeg');
    // Observe the app's existing consumer response. Never enable a competing reader.
    await view.evaluate(`(async()=>{
      const original=window.__TAURI__, {parseBinaryFrame}=await import('/src/preview/frame-decoder.ts');
      const state=await original.core.invoke('snapshot');
      const trace=window.__nativeSmokeBinary={latest:null,legacyEvents:0,original};
      trace.off=await original.event.listen('arkdock:frame',()=>trace.legacyEvents++);
      window.__TAURI__={...original,core:{...original.core,invoke:async(command,args)=>{
        const value=await original.core.invoke(command,args);
        if(command==='read_preview_frame'&&value instanceof ArrayBuffer&&value.byteLength){
          const frame=parseBinaryFrame(value,state);
          if(frame){let encoded='';const bytes=new Uint8Array(frame.jpeg);for(let i=0;i<bytes.length;i+=16384)encoded+=String.fromCharCode(...bytes.subarray(i,i+16384));trace.latest={...frame,jpeg:undefined,jpegBase64:btoa(encoded)};}
        }
        return value;
      }}};
    })()`);
    const protocolError = await view.evaluate(`import('/src/ipc/client.ts').then(async ({readPreviewFrame})=>{try{await readPreviewFrame(${JSON.stringify(scope)},2,'not-the-app-consumer');return null;}catch(error){return error;}})`);
    assert.equal(protocolError?.code,'PROTOCOL_VERSION_UNSUPPORTED');
    assert.equal(protocolError.expectedProtocolVersion,1); assert.equal(protocolError.receivedProtocolVersion,2);
    const staleError = await view.evaluate(`import('/src/ipc/client.ts').then(async ({readPreviewFrame})=>{try{await readPreviewFrame(${JSON.stringify({...scope,sessionId:scope.sessionId+'-stale'})},1,'not-the-app-consumer');return null;}catch(error){return error;}})`);
    assert.equal(staleError?.code,'STALE_REQUEST');
    const consumerError = await view.evaluate(`import('/src/ipc/client.ts').then(async ({readPreviewFrame})=>{try{await readPreviewFrame(${JSON.stringify(scope)},1,'not-the-app-consumer');return null;}catch(error){return error;}})`);
    assert.equal(consumerError?.code,'STALE_FRAME_CONSUMER');
    const disableError = await view.evaluate(`import('/src/ipc/client.ts').then(async ({request})=>{try{await request('disable_binary_preview_frames',{protocolVersion:1,consumerId:'not-the-app-consumer'},${JSON.stringify(scope)});return null;}catch(error){return error;}})`);
    assert.equal(disableError?.code,'STALE_FRAME_CONSUMER');
    // The official engine can stop sending unchanged pixels. Wake it with a real user click,
    // then observe the app's next packet rather than stealing the already-consumed static frame.
    const observedCount=nativeCount();
    const wakePoint=await view.evaluate('(()=>{const e=document.querySelector("canvas.native-frame");e.scrollIntoView({block:"center"});const r=e.getBoundingClientRect();return {x:r.left+r.width/2,y:r.top+r.height*360/640};})()');
    await view.send('Input.dispatchMouseEvent',{type:'mousePressed',...wakePoint,button:'left',clickCount:1});
    await view.send('Input.dispatchMouseEvent',{type:'mouseReleased',...wakePoint,button:'left',clickCount:1});
    await until('App reader observed after actual native input',()=>nativeCount()===observedCount+1,10000);
    let first;
    await until('Actual app-consumed binary JPEG',async()=>{first=await view.evaluate('window.__nativeSmokeBinary.latest');return Boolean(first);},10000);
    for(const [key,value] of Object.entries({protocolVersion:1,workspaceId:expected.project.id,sessionId:expected.sessionId,buildId:expected.build.buildId,generation:expected.build.generation,width:360,height:640,rotation:0,format:'jpeg'})) assert.equal(first[key],value);
    const firstBytes=Buffer.from(first.jpegBase64,'base64');assert.equal(firstBytes.length,first.payloadLength);writeFileSync('artifacts/native-binary-first.jpg',firstBytes);
    const count=nativeCount(), previous=await frame('binary-before-click');
    const point=await view.evaluate('(()=>{const e=document.querySelector("canvas.native-frame");e.scrollIntoView({block:"center"});const r=e.getBoundingClientRect();return {x:r.left+r.width/2,y:r.top+r.height*360/640};})()');
    await view.send('Input.dispatchMouseEvent',{type:'mousePressed',...point,button:'left',clickCount:1});
    await view.send('Input.dispatchMouseEvent',{type:'mouseReleased',...point,button:'left',clickCount:1});
    await until('Actual app-consumer native count delta',()=>nativeCount()===count+1,10000);
    let changed;
    await until('Actual changed JPEG on app-owned reader',async()=>{changed=await view.evaluate('window.__nativeSmokeBinary.latest');return changed?.sequence>first.sequence&&changed.jpegBase64!==first.jpegBase64;},10000);
    writeFileSync('artifacts/native-binary-after-click.jpg',Buffer.from(changed.jpegBase64,'base64'));
    assert.notEqual(await frame('binary-after-click'),previous);
    assert.equal(await view.evaluate('window.__nativeSmokeBinary.legacyEvents'),0,'Binary mode must suppress compatibility events');
    await view.evaluate('(()=>{const trace=window.__nativeSmokeBinary;trace.off();window.__TAURI__=trace.original;delete window.__nativeSmokeBinary;})()');
  }).catch((error)=>{checks.push({id:'IPC1',status:'FAIL',name:error.message});throw error;});
  await check('D2', 'Editing real .ets triggers watcher, serial rebuild and new native generation', async () => {
    const previousGeneration = running.build.generation;
    const previous = await frame('after-click');
    assert.ok(source.includes('Source revision: FIRST') && source.includes('ARKDOCK_NATIVE_REVISION=FIRST'));
    sourceChanged = true;
    writeFileSync(sourcePath,source.replaceAll('REVISION=FIRST','REVISION=SECOND').replace('Source revision: FIRST','Source revision: SECOND'));
    running = await until('Source rebuild native frame', (state) => state.phase === 'running' && state.build.generation > previousGeneration && state.build.displayedGeneration === state.build.generation);
    currentLog = logFrom(running);
    assert.match(readFileSync(currentLog,'utf8'), /ARKDOCK_NATIVE_REVISION=SECOND/);
    await until('New build image rendered on Canvas', async () => (await view.evaluate('document.querySelector(".native-frame")?.dataset.buildId')) === running.build.buildId, 10_000);
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
    await until('Recovered build image rendered on Canvas', async () => (await view.evaluate('document.querySelector(".native-frame")?.dataset.buildId')) === running.build.buildId, 10_000);
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
    try { await view.evaluate('(()=>{const trace=window.__nativeSmokeBinary;if(trace){trace.off();window.__TAURI__=trace.original;delete window.__nativeSmokeBinary;}})()'); if (projectId && !originalTrust) await invoke('set_trust',{projectId,trusted:false}); if (host) await invoke('detach'); }
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
