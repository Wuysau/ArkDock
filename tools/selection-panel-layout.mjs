// Initial-visibility regression for the explicit SelectionPanel UI fixture only.
// Uses an owned headless profile. Never attaches to ArkDock, user browsers, or Agents.
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve, sep } from 'node:path';

const fixtureUrl = 'http://127.0.0.1:1420/selection-ui-demo.html';
const sizes = [[360, 820], [440, 820], [360, 640]];
const scenarios = ['idle-empty', 'idle-filled', 'pending-inspect', 'confirmed-offer', 'inspect-failure'];
const eventScenarios = ['overlay-pointer', 'overlay-semantic', 'overlay-keyboard'];
const eventsOnly = process.argv.includes('--events-only');
const layoutsOnly = process.argv.includes('--layouts-only');
const evidence = resolve(process.env.ARKDOCK_EVIDENCE_DIR ?? `artifacts/selection-panel-layout-${Date.now()}`);
const report = { capturedAt: new Date().toISOString(), scope: 'UI layout only; no native preview or Agent transport claim',
  url: fixtureUrl, toolPid: process.pid, node: process.version, platform: process.platform,
  sizes, bannerExtraDip: 0, bannerOverlayDip: 44, checks: [], browserErrors: [], browser: null, cleanup: null };
const pause = (duration) => new Promise((done) => setTimeout(done, duration));
let browser, driver, profile, browserLaunchError;
let ownedProcesses = [];

async function until(predicate, message, timeout = 10_000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) { const value = await predicate(); if (value) return value; await pause(30); }
  throw new Error(message);
}

function verifyOwnedListener(port, browserPid) {
  if (process.platform !== 'win32') throw new Error('BLOCKED: this fixture browser ownership check requires Windows');
  // Numeric literals originate from our own child and DevToolsActivePort file.
  const script = `$ErrorActionPreference='Stop'; $fixtureRootPid=${browserPid}; $fixturePort=${port}; ` +
    `$owners=@(Get-NetTCPConnection -State Listen -LocalPort $fixturePort | Select-Object -ExpandProperty OwningProcess -Unique); ` +
    `if($owners.Count -ne 1){throw 'Owned debugger must have one listener'}; $fixtureCurrentPid=[int]$owners[0]; $chain=@(); ` +
    `for($depth=0;$depth -lt 12;$depth++){ $chain += $fixtureCurrentPid; if($fixtureCurrentPid -eq $fixtureRootPid){break}; ` +
    `$fixtureProcess=Get-CimInstance Win32_Process -Filter ('ProcessId = '+$fixtureCurrentPid); ` +
    `if(-not $fixtureProcess){throw 'Owned browser ancestry unavailable'}; $fixtureCurrentPid=[int]$fixtureProcess.ParentProcessId }; ` +
    `if($fixtureCurrentPid -ne $fixtureRootPid){throw 'Debugger listener is not our browser or child'}; ` +
    `$fixtureRoot=Get-CimInstance Win32_Process -Filter ('ProcessId = '+$fixtureRootPid); ` +
    `if(-not $fixtureRoot){throw 'Owned browser lifecycle ended'}; ` +
    `@{listenerPid=[int]$owners[0];browserPid=$fixtureRootPid;chain=@($chain);createdAtTicks=$fixtureRoot.CreationDate.ToUniversalTime().Ticks.ToString()} | ConvertTo-Json -Compress`;
  const result = spawnSync('pwsh', ['-NoProfile', '-Command', script], { windowsHide: true, encoding: 'utf8', timeout: 10_000 });
  if (result.status !== 0) throw new Error(`BLOCKED: private debugger ownership failed: ${result.stderr || result.error?.message}`);
  return JSON.parse(result.stdout);
}

function runProcessCheck(script) {
  const result = spawnSync('pwsh', ['-NoProfile', '-Command', `$ErrorActionPreference='Stop'; ${script}`],
    { windowsHide: true, encoding: 'utf8', timeout: 10_000 });
  if (result.status !== 0) throw new Error(`Owned process verification failed: ${result.stderr || result.error?.message}`);
  return JSON.parse(result.stdout);
}

function captureOwnedProcesses(rootPid, createdAtTicks) {
  assert.ok(Number.isInteger(rootPid) && rootPid > 0);
  assert.match(createdAtTicks, /^\d+$/);
  // Batch each owned generation. Avoid one expensive CIM query per renderer.
  return runProcessCheck(`$fixtureFrontier=@(@{pid=${rootPid};createdAtTicks='${createdAtTicks}'}); $fixtureProcesses=@(); ` +
    `for($fixtureDepth=0;$fixtureDepth -lt 12 -and $fixtureFrontier.Count -gt 0;$fixtureDepth++){ ` +
    `if($fixtureProcesses.Count+$fixtureFrontier.Count -gt 64){throw 'Owned browser descendant limit exceeded'}; ` +
    `$fixtureExpected=@{}; foreach($fixtureRecord in $fixtureFrontier){$fixtureExpected[[int]$fixtureRecord.pid]=$fixtureRecord.createdAtTicks}; ` +
    `$fixtureFilter=($fixtureFrontier | ForEach-Object {'ProcessId = '+[int]$_.pid}) -join ' OR '; ` +
    `$fixtureParents=@(Get-CimInstance Win32_Process -Filter $fixtureFilter -Property ProcessId,ParentProcessId,CreationDate); ` +
    `foreach($fixtureParent in $fixtureParents){if($fixtureParent.CreationDate.ToUniversalTime().Ticks.ToString() -ne $fixtureExpected[[int]$fixtureParent.ProcessId]){throw 'Owned parent lifecycle changed'}; ` +
    `$fixtureProcesses += @{pid=[int]$fixtureParent.ProcessId;parentPid=[int]$fixtureParent.ParentProcessId;createdAtTicks=$fixtureParent.CreationDate.ToUniversalTime().Ticks.ToString()} }; ` +
    `if(-not $fixtureParents.Count){break}; $fixtureChildFilter=($fixtureParents | ForEach-Object {'ParentProcessId = '+[int]$_.ProcessId}) -join ' OR '; ` +
    `$fixtureChildren=@(Get-CimInstance Win32_Process -Filter $fixtureChildFilter -Property ProcessId,ParentProcessId,CreationDate); ` +
    `$fixtureVerified=@{}; foreach($fixtureParent in @(Get-CimInstance Win32_Process -Filter $fixtureFilter -Property ProcessId,CreationDate)){ ` +
    `if($fixtureParent.CreationDate.ToUniversalTime().Ticks.ToString() -ne $fixtureExpected[[int]$fixtureParent.ProcessId]){throw 'Owned parent lifecycle changed'}; $fixtureVerified[[int]$fixtureParent.ProcessId]=$fixtureParent.CreationDate }; ` +
    `$fixtureFrontier=@(); foreach($fixtureChild in $fixtureChildren){if($fixtureVerified.ContainsKey([int]$fixtureChild.ParentProcessId) -and $fixtureChild.CreationDate -ge $fixtureVerified[[int]$fixtureChild.ParentProcessId]){ ` +
    `$fixtureFrontier += @{pid=[int]$fixtureChild.ProcessId;createdAtTicks=$fixtureChild.CreationDate.ToUniversalTime().Ticks.ToString()} }} }; ` +
    `if($fixtureFrontier.Count -gt 0){throw 'Owned browser ancestry depth exceeded'}; ` +
    `ConvertTo-Json -InputObject @($fixtureProcesses) -Compress`);
}

function checkRecordedProcesses(records) {
  const literal = JSON.stringify(records).replaceAll("'", "''");
  return runProcessCheck(`$fixtureRecords=ConvertFrom-Json '${literal}'; $fixtureExpected=@{}; $fixtureRemaining=@(); ` +
    `foreach($fixtureRecord in $fixtureRecords){$fixtureExpected[[int]$fixtureRecord.pid]=$fixtureRecord}; ` +
    `$fixtureFilter=($fixtureRecords | ForEach-Object {'ProcessId = '+[int]$_.pid}) -join ' OR '; ` +
    `foreach($fixtureProcess in @(Get-CimInstance Win32_Process -Filter $fixtureFilter -Property ProcessId,CreationDate)){ ` +
    `$fixtureRecord=$fixtureExpected[[int]$fixtureProcess.ProcessId]; ` +
    `if($fixtureProcess.CreationDate.ToUniversalTime().Ticks.ToString() -eq $fixtureRecord.createdAtTicks){$fixtureRemaining += $fixtureRecord} }; ` +
    `ConvertTo-Json -InputObject @($fixtureRemaining) -Compress`);
}

async function connectDebugger(port, path) {
  const version = await (await fetch(`http://127.0.0.1:${port}/json/version`, { signal: AbortSignal.timeout(2000) })).json();
  const browserEndpoint = new URL(version.webSocketDebuggerUrl);
  assert.equal(browserEndpoint.pathname, path, 'Debugger browser identity must match owned profile file');
  const pages = await (await fetch(`http://127.0.0.1:${port}/json/list`, { signal: AbortSignal.timeout(2000) })).json();
  const page = pages.find((item) => item.type === 'page' && item.url === 'about:blank');
  assert.ok(page, 'Owned browser must provide its new about:blank page');
  const endpoint = new URL(page.webSocketDebuggerUrl);
  assert.equal(endpoint.hostname, '127.0.0.1'); assert.equal(endpoint.protocol, 'ws:'); assert.equal(Number(endpoint.port), port);
  const socket = new WebSocket(endpoint); const pending = new Map(); let serial = 0;
  await new Promise((done, reject) => {
    const timer = setTimeout(() => { socket.close(); reject(new Error('Owned debugger handshake timed out')); }, 5000);
    socket.onopen = () => { clearTimeout(timer); done(); };
    socket.onerror = () => { clearTimeout(timer); reject(new Error('Owned debugger handshake failed')); };
  });
  socket.onmessage = (event) => {
    const response = JSON.parse(event.data);
    if (response.method === 'Runtime.exceptionThrown') report.browserErrors.push(response.params.exceptionDetails.exception?.description ?? response.params.exceptionDetails.text);
    const request = pending.get(response.id); if (!request) return;
    clearTimeout(request.timer); pending.delete(response.id);
    if (response.error) request.reject(new Error(response.error.message)); else request.resolve(response.result);
  };
  const send = (method, params = {}) => new Promise((done, reject) => {
    const id = ++serial; const timer = setTimeout(() => { pending.delete(id); reject(new Error(`Debugger timeout: ${method}`)); }, 10_000);
    pending.set(id, { resolve: done, reject, timer }); socket.send(JSON.stringify({ id, method, params }));
  });
  const evaluate = async (expression) => {
    const value = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
    if (value.exceptionDetails) throw new Error(value.exceptionDetails.exception?.description ?? value.exceptionDetails.text);
    return value.result.value;
  };
  await send('Page.enable'); await send('Runtime.enable');
  return { send, evaluate, close: () => socket.close() };
}

function geometryExpression() {
  return `(() => {
    const panel=document.querySelector('.selection-panel');
    const names={instruction:'#selection-instruction',agentHeader:'.agent-delivery > header',mode:'.agent-mode',primary:'.agent-delivery .selection-primary',inspect:'.agent-delivery > .composer-actions button'};
    if(document.querySelector('.agent-confirmation'))names.confirm='.agent-confirmation .primary-button';
    const rect=r=>({left:r.left,top:r.top,right:r.right,bottom:r.bottom,width:r.width,height:r.height});
    const controls=Object.entries(names).map(([name,selector])=>{
      const element=document.querySelector(selector);if(!element)return{name,missing:true};
      const bounds=element.getBoundingClientRect();let clip={left:0,top:0,right:innerWidth,bottom:innerHeight};const clipping=[];
      for(let ancestor=element.parentElement;ancestor;ancestor=ancestor.parentElement){
        const style=getComputedStyle(ancestor),box=ancestor.getBoundingClientRect();
        const area={left:box.left+ancestor.clientLeft,top:box.top+ancestor.clientTop,right:box.left+ancestor.clientLeft+ancestor.clientWidth,bottom:box.top+ancestor.clientTop+ancestor.clientHeight};
        if(/auto|scroll|hidden|clip/.test(style.overflowX)){clip.left=Math.max(clip.left,area.left);clip.right=Math.min(clip.right,area.right);}
        if(/auto|scroll|hidden|clip/.test(style.overflowY)){clip.top=Math.max(clip.top,area.top);clip.bottom=Math.min(clip.bottom,area.bottom);}
        if(/auto|scroll|hidden|clip/.test(style.overflowX+' '+style.overflowY))clipping.push({class:ancestor.className,area,scrollTop:ancestor.scrollTop,scrollLeft:ancestor.scrollLeft});
      }
      const hit=document.elementFromPoint(bounds.left+bounds.width/2,bounds.top+bounds.height/2);
      return{name,bounds:rect(bounds),clip,clipping,text:element.textContent,disabled:element.disabled,
        inside:bounds.width>0&&bounds.height>0&&bounds.left>=clip.left-.5&&bounds.top>=clip.top-.5&&bounds.right<=clip.right+.5&&bounds.bottom<=clip.bottom+.5,
        centerHit:hit===element||element.contains(hit),hit:hit?{tag:hit.tagName,class:hit.className}:null};
    });
    const state=window.__ARKDOCK_SELECTION_LAYOUT__;
    return{viewport:{width:innerWidth,height:innerHeight},app:rect(document.querySelector('.app').getBoundingClientRect()),
      inspector:rect(document.querySelector('.inspector-preview').getBoundingClientRect()),panel:rect(panel.getBoundingClientRect()),
      panelScrollTop:panel.scrollTop,controls,activeElement:document.activeElement?.tagName,
      scrolled:[...document.querySelectorAll('*')].filter(e=>e.scrollTop||e.scrollLeft).map(e=>({tag:e.tagName,class:e.className,top:e.scrollTop,left:e.scrollLeft})),
      instruction:document.querySelector('#selection-instruction')?.value,calls:[...state.calls],progress:{...state.progress},
      mode:document.querySelector('.agent-mode')?.textContent,primary:document.querySelector('.selection-primary')?.textContent,
      primaryDisabled:document.querySelector('.selection-primary')?.disabled,inspectDisabled:document.querySelector('.agent-delivery > .composer-actions button')?.disabled,
      confirmation:document.querySelector('.agent-confirmation')?.textContent,notice:document.querySelector('.composer-notice')?.textContent,
      banner:document.querySelector('.selection-demo-banner').textContent};
  })()`;
}

async function load(scenario, width, height) {
  await driver.send('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: 1, mobile: false });
  await driver.send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-reduced-motion', value: 'reduce' }] });
  await driver.send('Page.navigate', { url: `${fixtureUrl}?scenario=${scenario}` });
  await until(() => driver.evaluate(`window.__ARKDOCK_SELECTION_LAYOUT__?.scenario===${JSON.stringify(scenario)}&&document.querySelector('.selection-panel')&&window.__ARKDOCK_SELECTION_LAYOUT__.progress.imageReads===2&&[...document.querySelectorAll('.selection-images img')].length===2&&[...document.querySelectorAll('.selection-images img')].every(i=>i.complete&&i.naturalWidth>0)`), 'Fixture did not settle after image tasks');
  await pause(100);
  assert.deepEqual(await driver.evaluate('window.__ARKDOCK_SELECTION_LAYOUT__.calls'), ['selection_read_image', 'selection_read_image'], 'Mount must not inspect, pair or send to an Agent');
  assert.equal(await driver.evaluate('!!document.querySelector(".agent-confirmation")'), false, 'Confirmation requires explicit inspection');
  if (['pending-inspect', 'confirmed-offer', 'inspect-failure'].includes(scenario)) {
    assert.equal(await driver.evaluate('window.__ARKDOCK_SELECTION_LAYOUT__.requestAssisted()'), true);
    const expected = scenario === 'pending-inspect' ? 'document.querySelector(".selection-primary")?.textContent.trim()==="正在处理…"&&window.__ARKDOCK_SELECTION_LAYOUT__.progress.inspectReads===1'
      : scenario === 'confirmed-offer' ? '!!document.querySelector(".agent-confirmation")' : 'document.querySelector(".composer-notice")?.textContent.includes("TEST fixture cannot confirm")';
    await until(() => driver.evaluate(expected), 'Synthetic inspection state not rendered');
    await pause(100);
  }
}

async function checkScenario(scenario, width, height) {
  const name = `${scenario} ${width}x${height} initial visibility`;
  try {
    await load(scenario, width, height);
    const geometry = await driver.evaluate(geometryExpression()); const failures = [];
    // The initial copy/inspect actions stay visible. A returned offer adds a
    // secondary confirmation below them, reachable through the panel's own scroll.
    for (const control of geometry.controls.filter(control => control.name !== 'confirm')) {
      if (control.missing || !control.inside) failures.push(`${control.name} is clipped or absent`);
      if (control.missing || !control.centerHit) failures.push(`${control.name} center cannot be hit`);
    }
    if (geometry.panelScrollTop !== 0 || geometry.scrolled.length) failures.push('Fixture starts with a scrolled container');
    if (geometry.activeElement !== 'BODY') failures.push('Fixture focused a control before geometry measurement');
    if (geometry.app.width !== width || geometry.app.height !== height) failures.push('App logical size differs from requested size');
    if (geometry.banner.trim() !== 'UI 布局验证 · 非原生预览') failures.push('Explicit non-native banner missing');
    const expectedMode = '由你发送';
    const expectedPrimary = scenario === 'pending-inspect' ? '正在处理…' : '复制修改要求';
    if (geometry.mode !== expectedMode || geometry.primary !== expectedPrimary) failures.push('Capability label is not truthful');
    if (geometry.primaryDisabled !== (scenario === 'idle-empty' || scenario === 'pending-inspect') || geometry.inspectDisabled !== geometry.primaryDisabled) failures.push('Empty/pending action disabled state is incorrect');
    if (Boolean(geometry.confirmation) !== (scenario === 'confirmed-offer')) failures.push('Inspection confirmation state is incorrect');
    if (scenario === 'confirmed-offer' && (!geometry.confirmation.includes('只填写已确认的空输入框') || geometry.controls.find(control=>control.name==='confirm')?.disabled)) failures.push('Explicit fill confirmation is unavailable or misleading');
    if (scenario === 'inspect-failure' && !geometry.notice?.includes('可复制修改要求后手动粘贴')) failures.push('Inspection failure does not explain manual fallback');
    const inspecting = ['pending-inspect', 'confirmed-offer', 'inspect-failure'].includes(scenario);
    if (geometry.progress.inspectReads !== Number(inspecting)) failures.push('Inspection must run once and only after explicit action');
    const sideEffects = geometry.calls.filter((command) => !['selection_read_image', ...(inspecting ? ['agent_assisted_inspect'] : [])].includes(command));
    if (sideEffects.length) failures.push(`Unexpected bridge side effect: ${sideEffects}`);
    const screenshot = join(evidence, `${scenario}-${width}x${height}.png`);
    const image = await driver.send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false });
    writeFileSync(screenshot, Buffer.from(image.data, 'base64'));
    if (scenario === 'idle-empty' || scenario === 'pending-inspect') assert.equal(await driver.evaluate('window.__ARKDOCK_SELECTION_LAYOUT__.requestAssisted()'), false, 'Disabled inspect must refuse another request');
    if (scenario === 'confirmed-offer') {
      let confirmation = geometry.controls.find(control => control.name === 'confirm');
      for (let attempts = 0; attempts < 8 && (!confirmation?.inside || !confirmation.centerHit); attempts++) {
        const point = await driver.evaluate('(()=>{const r=document.querySelector(".selection-panel").getBoundingClientRect();return{x:r.right-18,y:r.bottom-18};})()');
        await driver.send('Input.dispatchMouseEvent', { type: 'mouseWheel', ...point, deltaX: 0, deltaY: 100 });
        await pause(60);
        confirmation = (await driver.evaluate(geometryExpression())).controls.find(control => control.name === 'confirm');
      }
      assert.ok(confirmation?.inside && confirmation.centerHit && !confirmation.disabled, 'Explicit confirmation must be reachable and hit-testable inside the scrollable panel');
      await driver.evaluate('[...document.querySelectorAll(".agent-confirmation button")].find(e=>e.textContent.trim()==="取消").click()');
      await until(()=>driver.evaluate('!document.querySelector(".agent-confirmation")'), 'Explicit offer cancellation did not clear confirmation');
      assert.equal(await driver.evaluate('window.__ARKDOCK_SELECTION_LAYOUT__.calls.some(e=>e==="agent_assisted_fill")'), false, 'Inspect/cancel must not fill a composer');
    }
    const status = failures.length ? 'FAIL' : 'PASS';
    report.checks.push({ status, name, failures, geometry, screenshot });
    console.log(`${status} ${name}${failures.length ? `: ${failures.join('; ')}` : ''}`);
    if (failures.length) process.exitCode = 1;
  } catch (error) {
    report.checks.push({ status: 'FAIL', name, failures: [error.message] }); process.exitCode = 1;
    console.error(`FAIL ${name}: ${error.message}`);
  }
}

async function checkOverlayEvent(scenario) {
  const name = `${scenario} creates exactly one selected context`;
  try {
    await driver.send('Emulation.setDeviceMetricsOverride', { width: 440, height: 820, deviceScaleFactor: 1, mobile: false });
    await driver.send('Page.navigate', { url: `${fixtureUrl}?scenario=${scenario}` });
    await until(() => driver.evaluate(`window.__ARKDOCK_SELECTION_LAYOUT__?.scenario===${JSON.stringify(scenario)}&&!!document.querySelector('[data-testid=selection-overlay]')`), 'Event fixture did not mount');
    const point = await driver.evaluate(`(() => {const r=document.querySelector('[data-testid=selection-overlay]').getBoundingClientRect();return{x:r.left+r.width/2,y:r.top+r.height/2};})()`);
    await driver.send('Input.dispatchMouseEvent', { type: 'mouseMoved', ...point });
    await until(() => driver.evaluate('!!document.querySelector(".selection-outline")'), 'Hover did not produce outline');
    const before = await driver.evaluate('({contexts:window.__ARKDOCK_SELECTION_LAYOUT__.progress.selectedContexts,panel:!!document.querySelector(".selection-panel"),outline:document.querySelector(".selection-outline")?.textContent})');
    assert.equal(before.contexts, 0, 'Hover must not create a selection'); assert.equal(before.panel, false, 'Hover must not open details');
    if (scenario === 'overlay-pointer') {
      await driver.send('Input.dispatchMouseEvent', { type: 'mousePressed', ...point, button: 'left', clickCount: 1 });
      await driver.send('Input.dispatchMouseEvent', { type: 'mouseReleased', ...point, button: 'left', clickCount: 1 });
    } else if (scenario === 'overlay-semantic') {
      // UIA/assistive default activation: detail=0, no preceding pointer sequence.
      await driver.evaluate('document.querySelector("[data-testid=selection-overlay]").click()');
    } else {
      await driver.evaluate('document.querySelector("[data-testid=selection-overlay]").focus({preventScroll:true})');
      await driver.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Enter', code: 'Enter', windowsVirtualKeyCode: 13 });
      await driver.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Enter', code: 'Enter', windowsVirtualKeyCode: 13 });
    }
    await pause(100);
    const after = await driver.evaluate('({contexts:window.__ARKDOCK_SELECTION_LAYOUT__.progress.selectedContexts,selections:window.__ARKDOCK_SELECTION_LAYOUT__.eventSelections,panel:!!document.querySelector(".selection-panel"),calls:window.__ARKDOCK_SELECTION_LAYOUT__.calls,scroll:[...document.querySelectorAll("*")].some(e=>e.scrollTop||e.scrollLeft)})');
    const failures = [];
    if (after.contexts !== 1) failures.push(`Expected one context, received ${after.contexts}`);
    if (!after.panel) failures.push('Committed selection details did not open');
    if (after.selections.some((selected) => selected.nodeId !== 'fixture-center-button')) failures.push('Wrong hit node');
    if (after.calls.some((command) => !['selection_read_image', 'agent_session_list'].includes(command))) failures.push('Unexpected bridge side effect');
    if (after.scroll) failures.push('Event fixture scrolled');
    const screenshot = join(evidence, `${scenario}-440x820.png`);
    const image = await driver.send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false });
    writeFileSync(screenshot, Buffer.from(image.data, 'base64'));
    const status = failures.length ? 'FAIL' : 'PASS'; report.checks.push({ status, name, failures, before, after, screenshot });
    console.log(`${status} ${name}${failures.length ? `: ${failures.join('; ')}` : ''}`); if (failures.length) process.exitCode = 1;
  } catch (error) { report.checks.push({ status: 'FAIL', name, failures: [error.message] }); process.exitCode = 1; console.error(`FAIL ${name}: ${error.message}`); }
}

function linksInOwnedTree(path) {
  return readdirSync(path, { withFileTypes: true }).some((item) => {
    const child = join(path, item.name); const stat = lstatSync(child);
    return stat.isSymbolicLink() || stat.isDirectory() && linksInOwnedTree(child);
  });
}

try {
  mkdirSync(evidence, { recursive: true });
  if (eventsOnly && layoutsOnly) throw new Error('Choose only one partial test mode; both flags would run no checks');
  const response = await fetch(fixtureUrl, { signal: AbortSignal.timeout(2000) });
  assert.equal(response.status, 200, 'Local Vite 1420 must serve explicit fixture HTML');
  assert.ok((await response.text()).includes('/tests/ui/selection-panel.tsx'), 'Response must be explicit SelectionPanel fixture');
  const executable = ['C:/Program Files/Google/Chrome/Application/chrome.exe', 'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe'].find(existsSync);
  if (!executable) throw new Error('BLOCKED: no installed Chromium browser');
  profile = mkdtempSync(join(tmpdir(), 'arkdock-selection-layout-owned-'));
  browser = spawn(executable, ['--headless=new', '--remote-debugging-address=127.0.0.1', '--remote-debugging-port=0',
    `--user-data-dir=${profile}`, '--no-first-run', '--no-default-browser-check', '--disable-extensions', '--disable-background-networking', 'about:blank'],
  { windowsHide: true, stdio: 'ignore' });
  browser.on('error', (error) => { browserLaunchError = error.message; });
  assert.ok(browser.pid, 'Owned headless browser PID unavailable');
  const active = await until(() => { if (browserLaunchError) throw new Error(`BLOCKED: owned browser launch failed: ${browserLaunchError}`); try {
    const [rawPort, path] = readFileSync(join(profile, 'DevToolsActivePort'), 'utf8').trim().split(/\r?\n/); const port = Number(rawPort);
    return Number.isInteger(port) && port > 0 && port <= 65535 && path?.startsWith('/devtools/browser/') && { port, path };
  } catch { return null; } }, 'Owned browser did not write DevToolsActivePort');
  const ownership = verifyOwnedListener(active.port, browser.pid);
  report.browser = { executable, pid: browser.pid, profile, port: active.port, ownership };
  driver = await connectDebugger(active.port, active.path);
  ownedProcesses = captureOwnedProcesses(browser.pid, ownership.createdAtTicks);
  if (!eventsOnly) for (const [width, height] of sizes) for (const scenario of scenarios) await checkScenario(scenario, width, height);
  if (!layoutsOnly) for (const scenario of eventScenarios) await checkOverlayEvent(scenario);
  if (report.browserErrors.length) { report.checks.push({ status: 'FAIL', name: 'No browser runtime exceptions', failures: report.browserErrors }); process.exitCode = 1; }
} catch (error) {
  const status = error.message.startsWith('BLOCKED:') ? 'BLOCKED' : 'FAIL';
  report.checks.push({ status, name: 'Private fixture setup', failures: [error.message] }); process.exitCode = status === 'BLOCKED' ? 2 : 1;
  console.error(`${status} ${error.message}`);
} finally {
  let processCleanupError = null;
  if (browser?.exitCode === null && browser.signalCode === null && report.browser?.ownership.createdAtTicks) {
    try {
      const latest = captureOwnedProcesses(browser.pid, report.browser.ownership.createdAtTicks);
      ownedProcesses = [...new Map([...ownedProcesses, ...latest].map((value) => [`${value.pid}:${value.createdAtTicks}`, value])).values()];
    } catch (error) { processCleanupError = error.message; }
  }
  if (driver) { try { await driver.send('Browser.close'); } catch { /* Owned browser may close before replying. */ } driver.close(); }
  if (browser?.exitCode === null && browser.signalCode === null) { try { await until(() => browser.exitCode !== null || browser.signalCode !== null, 'Owned browser exit delayed', 1500); } catch { browser.kill(); } }
  if (browser?.exitCode === null && browser.signalCode === null) { try { await until(() => browser.exitCode !== null || browser.signalCode !== null, 'Owned browser did not exit', 1500); } catch { /* Retain profile and report cleanup failure. */ } }
  let remaining = [];
  if (ownedProcesses.length) {
    try {
      remaining = checkRecordedProcesses(ownedProcesses);
      if (remaining.length) { await pause(500); remaining = checkRecordedProcesses(ownedProcesses); }
    } catch (error) { processCleanupError = error.message; }
  }
  report.cleanup = { browserPid: browser?.pid ?? null, browserExited: !browser || browser.exitCode !== null || browser.signalCode !== null,
    exitCode: browser?.exitCode ?? null, signalCode: browser?.signalCode ?? null, ownedProcesses, remaining, processCleanupError, launchError: browserLaunchError ?? null,
    profileRemoved: false, retainedProfile: profile ?? null };
  if (profile && resolve(profile).startsWith(`${resolve(tmpdir())}${sep}arkdock-selection-layout-owned-`) && report.cleanup.browserExited && !remaining.length && !processCleanupError) {
    await pause(200);
    try { if (!lstatSync(profile).isSymbolicLink() && !linksInOwnedTree(profile)) { rmSync(profile, { recursive: true, force: true }); report.cleanup.profileRemoved = true; report.cleanup.retainedProfile = null; } }
    catch { /* Browser cache may still be locked; retain exact owned profile. */ }
  }
  if (!report.cleanup.browserExited || remaining.length || processCleanupError) { report.checks.push({ status: 'FAIL', name: 'Owned browser cleanup', failures: [processCleanupError ?? `Owned browser processes remain: ${remaining.map((value) => value.pid)}`] }); process.exitCode = 1; }
  report.status = report.checks.some((check) => check.status === 'FAIL') ? 'FAIL' : report.checks.some((check) => check.status === 'BLOCKED') ? 'BLOCKED' : 'PASS';
  mkdirSync(evidence, { recursive: true }); writeFileSync(join(evidence, 'report.json'), `${JSON.stringify(report, null, 2)}\n`);
  console.log(`${report.status} SelectionPanel layout. Evidence: ${evidence}. Cleanup: ${JSON.stringify(report.cleanup)}`);
}
