// Actual Windows WebView2 + SDK Inspector. Only uses an explicitly owned task PID.
// This driver neither supplies native frames nor sends any Agent message.
import assert from 'node:assert/strict';
import { spawn, execFileSync } from 'node:child_process';
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { resolve, join } from 'node:path';
import { connectWebview } from './webview-driver.mjs';
import { activateOwnedHeader, requireOwnedTargetEdge } from './window-input.mjs';

const ownedPid = Number(process.env.ARKDOCK_PROBE_PID);
const port = Number(process.env.ARKDOCK_PROBE_PORT ?? 9447);
const url = process.env.ARKDOCK_PROBE_URL ?? 'http://127.0.0.1:1420/';
const project = resolve('examples/native-counter');
const normalizedPath = (path) => path.replaceAll('\\','/').replace(/^\/\/\?\//,'').toLowerCase();
const evidence = resolve(process.env.ARKDOCK_INSPECTOR_EVIDENCE ?? 'artifacts/inspector-ui');
const checks = [];
const report = {capturedAt:new Date().toISOString(),platform:process.platform,sdkVersion:'26.0.0.105',checks};
const pause = (ms) => new Promise((done) => setTimeout(done,ms));
let view, host, dockHandle, originalWindow, projectId, originalTrust = false, nativeLog;
function blocked(message) { const error = new Error(message); error.blocked = true; throw error; }
function action(kind, handle, values = {}) {
  const args=['-NoProfile','-File',resolve('tests/native/window-actions.ps1'),'-Action',kind,'-Handle',String(handle)];
  for(const [key,value] of Object.entries(values)) args.push(`-${key}`,String(value));
  return JSON.parse(execFileSync('pwsh',args,{encoding:'utf8',windowsHide:true,timeout:12000}));
}
// Driver protocol probes are recorded separately from calls made by product UI.
const snapshot = () => view.evaluate('(window.__arkdockInspectorSmoke?.driverInvoke??window.__TAURI__.core.invoke)("snapshot")');
async function invoke(command,args={}) {
  const state=await snapshot();
  return view.evaluate(`(window.__arkdockInspectorSmoke?.driverInvoke??window.__TAURI__.core.invoke)(${JSON.stringify(command)},${JSON.stringify({...args,scope:{targetId:state.target?.id??null,sessionId:state.sessionId}})})`);
}
async function until(name,test,timeout=10000) {
  const deadline=Date.now()+timeout;
  while(Date.now()<deadline) {const result=await test();if(result)return result;await pause(50);}
  throw new Error(`${name}: timeout`);
}
async function clickPoint(point) {
  await view.send('Input.dispatchMouseEvent',{type:'mousePressed',...point,button:'left',clickCount:1});
  await view.send('Input.dispatchMouseEvent',{type:'mouseReleased',...point,button:'left',clickCount:1});
}
async function clickControl(expression) {
  const point=await until('UI control ready',()=>view.evaluate(`(() => {const e=${expression};if(!e||e.disabled||e.closest('[inert],[hidden]'))return null;e.scrollIntoView({block:'center'});const r=e.getBoundingClientRect();if(r.width<=0||r.height<=0||getComputedStyle(e).visibility!=='visible')return null;const point={x:r.x+r.width/2,y:r.y+r.height/2};if(!e.contains(document.elementFromPoint(point.x,point.y)))return null;return point;})()`));
  await clickPoint(point);
}
async function toggleInspector() {
  // The preview-first toolbar is hidden by default. Use its visible UI entry,
  // then close it so floating controls cannot cover the component under test.
  if(!await view.evaluate('document.querySelector(".chrome-top")?.dataset.open==="true"'))
    await clickControl('document.querySelector(".toolbar-trigger")');
  await until('Preview controls expanded',()=>view.evaluate('document.querySelector(".chrome-top")?.dataset.open==="true"'));
  await clickControl('document.querySelector("[data-testid=inspector-toggle]")');
  await clickControl('document.querySelector(".toolbar-trigger")');
  await until('Preview controls collapsed',()=>view.evaluate('document.querySelector(".chrome-top")?.dataset.open==="false"'));
}
async function inspectorAction(testId) {
  if(!await view.evaluate('document.querySelector(".chrome-top")?.dataset.open==="true"'))
    await clickControl('document.querySelector(".toolbar-trigger")');
  await until('Inspector controls expanded',()=>view.evaluate('document.querySelector(".chrome-top")?.dataset.open==="true"'));
  await clickControl(`document.querySelector('[data-testid="${testId}"]')`);
  await clickControl('document.querySelector(".toolbar-trigger")');
  await until('Inspector controls collapsed',()=>view.evaluate('document.querySelector(".chrome-top")?.dataset.open==="false"'));
}
async function assertNoUiMcpMutation() {
  const forbidden=['mcp_submit_task','mcp_retry_task','mcp_revoke_context','context_select_project'];
  const calls=await view.evaluate(`window.__arkdockInspectorSmoke.calls.filter(call=>call.source==='ui'&&${JSON.stringify(forbidden)}.includes(call.command))`);
  assert.deepEqual(calls,[],'Copy and Inspector actions must not submit, retry, revoke or confirm an MCP context');
}
function assertCompactPrompt(prompt,selection) {
  assert.ok(prompt.includes(selection.userInstruction));
  assert.ok(prompt.includes(selection.workspaceName));
  assert.ok(prompt.includes(selection.selectedNode.componentType));
  if(selection.sourceEvidence.relativePath)assert.ok(prompt.includes(selection.sourceEvidence.relativePath),'Candidate source path must remain complete');
  for(const id of [selection.selectionId,selection.workspaceId,selection.sessionId,selection.buildId])
    assert.equal(prompt.includes(id),false,'Human-facing prompt must omit internal transport identities');
  for(const key of ['nodeId','parentId','children','attributes','frameSequence'])
    assert.equal(prompt.includes(`"${key}"`),false,'Human-facing prompt must omit the complete Inspector dump');
  assert.ok(prompt.includes('复制或填入提示词不会附加截图'));
}
async function framePoint(x,y) {
  return view.evaluate(`(() => {const c=document.querySelector('[data-testid="native-canvas"]');c.scrollIntoView({block:'center'});const r=c.getBoundingClientRect();return {x:r.x+r.width*${x}/360,y:r.y+r.height*${y}/640};})()`);
}
function nativeCount() {
  return Number([...readFileSync(nativeLog,'utf8').matchAll(/ARKDOCK_NATIVE_COUNT=(\d+)/g)].at(-1)?.[1]??0);
}
async function check(name,task) {await task();checks.push({status:'PASS',name});console.log(`PASS ${name}`);}
async function capture(name) {
  const result=await view.send('Page.captureScreenshot',{format:'png'});
  writeFileSync(join(evidence,`${name}.png`),Buffer.from(result.data,'base64'));
}
// Native stdio handshake, transport and claim isolation belong to
// src-tauri/tests/mcp_stdio_process.rs; this driver stays within Inspector/UI scope.
async function mcpStatus() {
  return view.evaluate('(window.__arkdockInspectorSmoke?.driverInvoke??window.__TAURI__.core.invoke)("mcp_status",{})');
}
function requireNoMcpClient(status) {
  if(status.contexts.some(context=>context.connected)||status.tasks.length)
    blocked('Dedicated Inspector probe requires no MCP clients or queued tasks; existing contexts and tasks were preserved');
}
function assertMcpConfiguration(status) {
  assert.equal(status.available,true,status.reason);
  assert.ok(status.configuration.command.toLowerCase().endsWith('.exe'),'MCP configuration must use the native executable');
  assert.deepEqual(status.configuration.args,['--mcp']);
  const config=JSON.parse(status.configuration.json);
  assert.deepEqual(config.mcpServers.arkdock,{command:status.configuration.command,args:['--mcp']});
  if(process.env.ARKDOCK_DESKTOP_EXE)assert.equal(normalizedPath(status.configuration.command),normalizedPath(resolve(process.env.ARKDOCK_DESKTOP_EXE)));
}

try {
  mkdirSync(evidence,{recursive:true});
  if(process.platform!=='win32'||!process.env.ARKDOCK_SDK) blocked('Windows and explicit ARKDOCK_SDK required');
  if(!Number.isSafeInteger(ownedPid)||ownedPid<=0) blocked('Explicit task-owned ARKDOCK_PROBE_PID required');
  const deveco=execFileSync('pwsh',['-NoProfile','-Command','@(Get-Process -Name deveco64,deveco -ErrorAction SilentlyContinue).Count'],{encoding:'utf8',windowsHide:true});
  if(Number(deveco.trim())) blocked('DevEco GUI must be closed by its owner');
  try {view=await connectWebview(port,5000,url,ownedPid);} catch(error) {blocked(`Owned debugger unavailable: ${error.message}`);}
  const initial=await snapshot();
  if(initial.target||initial.sessionId) blocked('Dedicated probe must start detached; no user association was changed');
  await check('Generic MCP configuration is readable before docking without starting a client',async()=>{
    const status=await mcpStatus();requireNoMcpClient(status);assertMcpConfiguration(status);
    report.mcpSetup={beforeDocking:true,command:status.configuration.command,args:status.configuration.args,clientStarted:false};
  });
  if(!initial.backend.available) blocked(initial.backend.reason);
  dockHandle=action('find',0,{ProcessId:ownedPid}).handle;
  if(!dockHandle) blocked('Explicit task-owned ArkDock window not found');
  originalWindow=action('info',dockHandle);
  host=spawn('pwsh',['-NoProfile','-File',resolve('tests/native/window-host.ps1'),'-ProjectPath',project],{windowsHide:true,stdio:['ignore','pipe','pipe']});
  const target=await new Promise((done,reject)=>{
    let output='',errors='';const timer=setTimeout(()=>reject(new Error(`Owned fixture startup timeout: ${errors}`)),10000);
    host.stderr.on('data',(data)=>{errors+=data;});host.stdout.on('data',(data)=>{output+=data;if(output.includes('\n')){clearTimeout(timer);done(JSON.parse(output.split('\n')[0]));}});
    host.once('exit',(code)=>{clearTimeout(timer);reject(new Error(`Owned fixture exit ${code}`));});
  });
  action('restore',dockHandle);action('move',target.first,{X:40,Y:400,Width:700,Height:700});
  action('move',target.second,{X:50,Y:520,Width:400,Height:420});action('move',dockHandle,{X:1000,Y:400,Width:1000,Height:1250});
  action('foreground',target.first);await pause(200);action('foreground',dockHandle);await pause(250);
  await activateOwnedHeader(view,dockHandle,action);
  action('raise',target.first,{ProcessId:target.pid});
  action('raise',dockHandle,{ProcessId:ownedPid});
  const before=action('info',dockHandle),destination=action('info',target.first);
  requireOwnedTargetEdge(action,target.first,destination,ownedPid);
  const drag=await view.evaluate('(() => {const r=document.querySelector(".drag").getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2,ratio:devicePixelRatio};})()');
  const x=Math.round(before.clientX+drag.x*drag.ratio),y=Math.round(before.clientY+drag.y*drag.ratio);
  report.drag=action('drag',dockHandle,{X:x,Y:y,EndX:x+destination.right-before.left,EndY:y+destination.top-before.top,OutlinePid:ownedPid});
  const found=await until('Owned fixture discovery',async()=>{const s=await snapshot();return s.target?.process.pid===target.pid&&s.phase==='selectProject'&&s.candidates.some((c)=>normalizedPath(c.project.appRoot)===normalizedPath(project))&&s;},20000);
  const candidate=found.candidates.find((c)=>normalizedPath(c.project.appRoot)===normalizedPath(project));
  await clickControl(`[...document.querySelectorAll('.candidate-list button')].find(b=>b.dataset.candidateId===${JSON.stringify(candidate.id)})`);
  projectId=candidate.project.id;
  const associated=await until('Selected current fixture identity',async()=>{const value=await snapshot();return value.project?.id===projectId&&value.phase!=='selectProject'&&value;});
  report.initialFixtureTrust=associated.trusted;
  report.recoverOriginalUntrusted=process.env.ARKDOCK_PROBE_RESTORE_UNTRUSTED==='1';
  originalTrust=associated.trusted&&!report.recoverOriginalUntrusted;
  if(!associated.trusted) await clickControl('[...document.querySelectorAll("dialog button")].find(b=>b.textContent.trim()==="信任此工程")');
  const running=await until('Real SDK frame',async()=>{const s=await snapshot();if(['protocolBlocked','sdkMissing','crashed'].includes(s.phase))throw new Error(JSON.stringify(s.diagnostics.slice(-2)));return s.phase==='running'&&s.build.displayedGeneration===s.build.generation&&s;},150000);
  const receipt=running.diagnostics.findLast((d)=>d.code==='NATIVE_RUNTIME'&&d.buildId===running.build.buildId&&d.message.startsWith('BUILD_COMPLETED:'));
  const directory=receipt?.message.split('; logs ')[1];assert.ok(directory?.includes('ArkDock-native'));nativeLog=join(directory,'previewer.log');
  await until('Displayed JPEG canvas',()=>view.evaluate('document.querySelector("[data-testid=native-canvas]")?.dataset.format==="jpeg"'));
  // This dedicated app can be reused after a previous fixture detached while
  // local inspect mode stayed active. Begin each measurement in normal mode.
  if(await view.evaluate('document.querySelector("[data-testid=inspector-toggle]")?.getAttribute("aria-pressed")==="true"'))
    await toggleInspector();
  await view.evaluate(`(() => {
    const originalBridge=window.__TAURI__,original=originalBridge.core.invoke;
    const smoke=window.__arkdockInspectorSmoke={trees:[],selections:[],creates:[],calls:[],errors:[]};
    const recorded=async(source,command,args)=>{
      smoke.calls.push({source,command,at:performance.now(),...(command==='selection_prompt'?{detailed:args?.detailed}:{})});
      try {const result=await original(command,args);if(command==='inspector_get_tree')smoke.trees.push(result);if(command==='selection_create'){smoke.selections.push(result);smoke.creates.push(args.draft);}return result;}
      catch(error){smoke.errors.push({source,command,error:String(error)});throw error;}
    };
    smoke.driverInvoke=(command,args)=>recorded('driver',command,args);
    window.__TAURI__={...originalBridge,core:{...originalBridge.core,invoke:(command,args)=>recorded('ui',command,args)}};
    smoke.cleanup=()=>{window.__TAURI__=originalBridge;};
  })()`);
  let tree,selection;
  await check('Actual SDK inspector tree, overlay hover and component pick do not trigger native onClick',async()=>{
    const count=nativeCount();
    await toggleInspector();
    tree=await until('Real Inspector tree and matched component overlay',()=>view.evaluate('(() => {const overlay=document.querySelector("[data-testid=selection-overlay]");return overlay&&!overlay.classList.contains("region-mode")&&window.__arkdockInspectorSmoke.trees.at(-1);})()'));
    assert.equal(tree.workspaceId,running.project.id);assert.equal(tree.buildId,running.build.buildId);
    assert.equal(tree.exactFrameMatch,false);assert.ok(tree.nodes.length>=5);
    const button=tree.nodes.find((node)=>node.componentType==='Button');assert.ok(button?.bounds);
    const point=await framePoint(button.bounds.x+button.bounds.width/2,button.bounds.y+button.bounds.height/2);
    await view.send('Input.dispatchMouseEvent',{type:'mouseMoved',...point});
    await until('Actual hover boundary',()=>view.evaluate('document.querySelector(".selection-outline")?.textContent.includes("Button")'));
    await clickPoint(point);
    selection=await until('Native component selection',()=>view.evaluate('window.__arkdockInspectorSmoke.selections.at(-1)'));
    assert.equal(selection.selectionKind,'component');assert.equal(selection.selectedNode.componentType,'Button');
    assert.equal(selection.sourceEvidence.status,'candidate');assert.ok(selection.sourceEvidence.relativePath?.endsWith('Index.ets'));
    await pause(200);assert.equal(nativeCount(),count);
    assert.equal(await view.evaluate('window.__arkdockInspectorSmoke.calls.filter(c=>c.command==="click_preview").length'),0);
    report.tree=tree;report.componentSelection=selection;await capture('component-selection');
  });
  await check('Original JPEG and crop image identities match selection and decode dimensions',async()=>{
    await until('Selection image previews are allowed by the desktop CSP',()=>view.evaluate('(() => {const images=[...document.querySelectorAll(".selection-images img")];return images.length===2&&images.every(image=>image.complete&&image.naturalWidth>0);})()'));
    report.images=await view.evaluate(`(async()=>{
      const s=await window.__arkdockInspectorSmoke.driverInvoke('snapshot');const scope={targetId:s.target.id,sessionId:s.sessionId};const selected=window.__arkdockInspectorSmoke.selections.at(-1);
      const results=[];for(const crop of [false,true]){
        const raw=await window.__arkdockInspectorSmoke.driverInvoke('selection_read_image',{selectionId:selected.selectionId,crop,scope});
        const bytes=new Uint8Array(raw);const digest=Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256',bytes)),b=>b.toString(16).padStart(2,'0')).join('');
        const bitmap=await createImageBitmap(new Blob([bytes],{type:'image/jpeg'}));
        let binary='';for(const byte of bytes)binary+=String.fromCharCode(byte);
        results.push({crop,sha256:digest,byteLength:bytes.length,width:bitmap.width,height:bitmap.height,base64:btoa(binary)});bitmap.close();
      }return results;
    })()`);
    for(const image of report.images){const metadata=image.crop?selection.crop:selection.screenshot;assert.equal(image.sha256,metadata.sha256);assert.equal(image.byteLength,metadata.byteLength);assert.equal(image.width,metadata.width);assert.equal(image.height,metadata.height);writeFileSync(join(evidence,image.crop?'crop.jpg':'native-preview.jpg'),Buffer.from(image.base64,'base64'));delete image.base64;}
  });
  await check('Orb and Hidden pause Inspector queries while preserving the native runtime',async()=>{
    const queryCount=()=>view.evaluate('window.__arkdockInspectorSmoke.calls.filter(call=>call.command==="inspector_get_tree").length');
    report.hiddenInspection=[];
    for(const [hideCommand,mode,showCommand] of [['collapse_to_orb','orb','expand_from_orb'],['hide_to_tray','hidden','show_from_tray']]){
      const before=await snapshot(),count=nativeCount();
      await invoke(hideCommand,{requestId:before.display.requestId+1});
      await until(`Native ${mode} acknowledged`,async()=>{const state=await snapshot();return state.display.mode===mode;});
      await pause(1000);
      const paused=await queryCount();await pause(1100);
      assert.equal(await queryCount(),paused,'Hidden main surface must stop new Inspector queries');
      const hidden=await snapshot();assert.equal(hidden.sessionId,before.sessionId);assert.deepEqual(hidden.build,before.build);assert.equal(hidden.phase,'running');assert.equal(nativeCount(),count);
      await invoke(showCommand,{requestId:hidden.display.requestId+1});
      await until('Native main surface restored',async()=>{const state=await snapshot();return state.display.mode==='expanded';});
      await until('Inspector polling resumes after visibility restoration',async()=>await queryCount()>paused);
      const restored=await snapshot();assert.equal(restored.sessionId,before.sessionId);assert.deepEqual(restored.build,before.build);assert.equal(nativeCount(),count);
      report.hiddenInspection.push({mode,pausedQueryCount:paused,resumedQueryCount:await queryCount(),sessionPreserved:true});
    }
  });
  await check('Bad build and wrong scope are rejected; compact copy does not submit an MCP task',async()=>{
    const draft=await view.evaluate('window.__arkdockInspectorSmoke.creates.at(-1)');
    await assert.rejects(invoke('selection_create',{draft:{...draft,frame:{...draft.frame,buildId:'wrong-build'}}}),/STALE_SELECTION_FRAME|SELECTION_BUILD_STALE/);
    const state=await snapshot();
    await assert.rejects(view.evaluate(`window.__arkdockInspectorSmoke.driverInvoke('selection_get',${JSON.stringify({selectionId:selection.selectionId,scope:{targetId:state.target.id,sessionId:'wrong-session'}})})`),/SESSION|SCOPE|STALE/);
    await view.evaluate('document.querySelector("#selection-instruction").focus()');
    await view.send('Input.insertText',{text:'Smoke test context only; do not send or modify source.'});
    assert.equal(await view.evaluate('document.querySelector(".selection-technical")?.open'),false,'Technical details must begin collapsed');
    await clickControl('document.querySelector(".selection-technical > summary")');
    await until('Technical details opened explicitly',()=>view.evaluate('document.querySelector(".selection-technical")?.open'));
    await clickControl('[...document.querySelectorAll(".selection-actions button")].find(b=>b.textContent==="保存选择上下文")');
    selection=await until('User requirement saved with selection',async()=>{const value=await invoke('selection_get',{selectionId:selection.selectionId});return value.userInstruction==='Smoke test context only; do not send or modify source.'&&value;});
    const prompt=await invoke('selection_prompt',{selectionId:selection.selectionId});assertCompactPrompt(prompt,selection);
    const explicitCompact=await invoke('selection_prompt',{selectionId:selection.selectionId,detailed:false});assert.equal(explicitCompact,prompt);
    const detailed=await invoke('selection_prompt',{selectionId:selection.selectionId,detailed:true});
    for(const value of [selection.selectionId,selection.workspaceId,selection.sessionId,selection.buildId,selection.userInstruction])assert.ok(detailed.includes(value));
    const evidenceStart=detailed.indexOf('\n{'),evidenceEnd=detailed.indexOf('\n\n源码定位：');
    assert.ok(evidenceStart>=0&&evidenceEnd>evidenceStart,'Detailed export must include the original component JSON section');
    assert.deepEqual(JSON.parse(detailed.slice(evidenceStart+1,evidenceEnd)),selection.selectedNode,'Explicit detailed export preserves the complete Inspector node');
    assert.ok(prompt.length<detailed.length,'Compact copy must be shorter than the full diagnostic export');
    report.promptVerified={defaultCompact:true,explicitCompact:true,detailedRetainsIdentitiesAndNode:true,compactLength:prompt.length,detailedLength:detailed.length};
    await clickControl('document.querySelector(".selection-technical > summary")');
    await until('Technical details collapsed',()=>view.evaluate('document.querySelector(".selection-technical")?.open===false'));
    await until('Read-only MCP status shown in the selection panel',()=>view.evaluate('window.__arkdockInspectorSmoke.calls.some(c=>c.source==="ui"&&c.command==="mcp_status")'));
    assert.deepEqual(await view.evaluate('(() => {const panel=document.querySelector(".agent-delivery");const submit=panel?.querySelector(".selection-primary");const copy=panel?.querySelector(".composer-actions button");return {submitLabel:submit?.textContent.trim(),submitDisabled:submit?.disabled,copyLabel:copy?.textContent.trim(),copyDisabled:copy?.disabled,context:panel?.querySelector("select")?.value};})()'),
      {submitLabel:'提交到 MCP',submitDisabled:true,copyLabel:'复制修改要求',copyDisabled:false,context:''});
    // Exercise the real component and native prompt construction while keeping
    // the user's system clipboard untouched. This proves UI routing only.
    await view.evaluate('(() => {const smoke=window.__arkdockInspectorSmoke;smoke.originalClipboardWrite=navigator.clipboard.writeText;navigator.clipboard.writeText=async(text)=>{smoke.copiedPrompt=text;};})()');
    try {
      await clickControl('document.querySelector(".agent-delivery .composer-actions button")');
      await until('Explicit copy confirmation',()=>view.evaluate('document.querySelector(".agent-delivery")?.textContent.includes("修改要求已复制")'));
      const copied=await view.evaluate('window.__arkdockInspectorSmoke.copiedPrompt');
      assertCompactPrompt(copied,selection);assert.equal(copied,prompt);
      assert.equal(await view.evaluate('window.__arkdockInspectorSmoke.calls.filter(c=>c.source==="ui"&&c.command==="selection_prompt").at(-1)?.detailed'),false);
      await assertNoUiMcpMutation();
      report.copyFallback={nativePromptVerified:true,compactUiRouteVerified:true,noUiMcpMutation:true,systemClipboardWritten:false,agentMessagesSent:0};
      await capture('selection-copy-fallback');
      await clickControl('document.querySelector(".selection-technical > summary")');
      await clickControl('[...document.querySelectorAll(".selection-technical button")].find(b=>b.textContent==="复制详细诊断提示词")');
      await until('Explicit detailed copy confirmation',()=>view.evaluate('document.querySelector(".selection-panel > output")?.textContent.includes("详细诊断提示词已复制")'));
      assert.equal(await view.evaluate('window.__arkdockInspectorSmoke.copiedPrompt'),detailed);
      assert.equal(await view.evaluate('window.__arkdockInspectorSmoke.calls.filter(c=>c.source==="ui"&&c.command==="selection_prompt").at(-1)?.detailed'),true);
      await assertNoUiMcpMutation();
      report.detailedCopyUi={explicitDetailRouteVerified:true,fullPromptPreserved:true,systemClipboardWritten:false};
      await clickControl('document.querySelector(".selection-technical > summary")');
    } finally { await view.evaluate('navigator.clipboard.writeText=window.__arkdockInspectorSmoke.originalClipboardWrite'); }
  });
  await check('No MCP client leaves task submission gated and creates no queued task',async()=>{
    const before=await mcpStatus();requireNoMcpClient(before);assertMcpConfiguration(before);
    await assert.rejects(invoke('mcp_submit_task',{contextId:'ctx-inspector-unconfirmed',selectionId:selection.selectionId}),/MCP_CONTEXT|CONFIRM|UNAVAILABLE/);
    const after=await mcpStatus();assert.deepEqual(after.tasks,before.tasks);assert.deepEqual(after.contexts,before.contexts);
    await assertNoUiMcpMutation();
    report.mcp={configurationVerified:true,noClientStarted:true,unconfirmedSubmitRejected:true,noQueuedTask:true,
      scope:'Inspector UI/configuration only; does not claim native stdio task coverage',
      nativeProtocolTests:'src-tauri/tests/mcp_stdio_process.rs'};
  });
  await check('Native window resize retains accurate component hit coordinates',async()=>{
    await clickControl('document.querySelector("button[aria-label=关闭组件详情]")');
    const before=action('info',dockHandle);action('move',dockHandle,{X:before.left,Y:before.top,Width:640,Height:1000});await pause(200);
    const button=tree.nodes.find((node)=>node.componentType==='Button');
    const point=await framePoint(button.bounds.x+button.bounds.width/2,button.bounds.y+button.bounds.height/2);
    const count=nativeCount(),previous=await view.evaluate('window.__arkdockInspectorSmoke.selections.length');
    await clickPoint(point);await until('Selection after native resize',()=>view.evaluate(`window.__arkdockInspectorSmoke.selections.length>${previous}`));
    const picked=await view.evaluate('window.__arkdockInspectorSmoke.selections.at(-1)');assert.equal(picked.selectedNode.componentType,'Button');assert.equal(nativeCount(),count);
    report.resized=await view.evaluate('({width:innerWidth,height:innerHeight,ratio:devicePixelRatio,canvas:document.querySelector("[data-testid=native-canvas]").getBoundingClientRect().toJSON()})');await capture('resized-hit');
  });
  await check('Explicit region mode stores real crop and Esc restores normal ArkUI click',async()=>{
    await clickControl('document.querySelector("button[aria-label=关闭组件详情]")');
    await inspectorAction('inspector-region');
    const previous=await view.evaluate('window.__arkdockInspectorSmoke.selections.length'),count=nativeCount();
    const start=await framePoint(100,100),end=await framePoint(250,200);
    await view.send('Input.dispatchMouseEvent',{type:'mousePressed',...start,button:'left',clickCount:1});
    await view.send('Input.dispatchMouseEvent',{type:'mouseMoved',...end,button:'left',buttons:1});
    await view.send('Input.dispatchMouseEvent',{type:'mouseReleased',...end,button:'left',clickCount:1});
    await until('Real region crop',()=>view.evaluate(`window.__arkdockInspectorSmoke.selections.length>${previous}`));
    const region=await view.evaluate('window.__arkdockInspectorSmoke.selections.at(-1)');assert.equal(region.selectionKind,'region');assert.equal(region.selectedNode,null);assert.equal(nativeCount(),count);report.region=region;
    await view.send('Input.dispatchKeyEvent',{type:'keyDown',key:'Escape',code:'Escape',windowsVirtualKeyCode:27});
    await view.send('Input.dispatchKeyEvent',{type:'keyUp',key:'Escape',code:'Escape',windowsVirtualKeyCode:27});
    await until('Normal mode restored',()=>view.evaluate('!document.querySelector("[data-testid=selection-overlay]")&&document.querySelector("[data-testid=inspector-toggle]")?.getAttribute("aria-pressed")==="false"'));
    const button=tree.nodes.find((node)=>node.componentType==='Button');await clickPoint(await framePoint(button.bounds.x+button.bounds.width/2,button.bounds.y+button.bounds.height/2));
    await until('Real ArkUI onClick after Esc',()=>nativeCount()===count+1);
    const state=await snapshot();assert.equal(state.sessionId,running.sessionId);assert.equal(state.build.buildId,running.build.buildId);assert.equal(state.build.generation,running.build.generation);await capture('normal-click-restored');
  });
} catch(error) {
  checks.push({status:error.blocked?'BLOCKED':'FAIL',name:error.message});process.exitCode=error.blocked?2:1;
  if(view){try{report.failureSnapshot=await snapshot();report.failureDom=await view.evaluate('document.body.innerText');report.driverErrors=await view.evaluate('window.__arkdockInspectorSmoke?.errors');report.failureTree=await view.evaluate('window.__arkdockInspectorSmoke?.trees.at(-1)');await capture('failure');}catch{/* Keep primary failure. */}}
} finally {
  if(view){
    const cleanup=async(name,task)=>{try{await task();}catch(error){checks.push({status:'FAIL',name:`Owned fixture cleanup ${name}: ${error.message}`});process.exitCode=1;}};
    await cleanup('instrumentation',()=>view.evaluate('window.__arkdockInspectorSmoke?.cleanup()'));
    if(projectId){
      await cleanup('selection',async()=>{const current=await snapshot();if(current.trusted&&current.sessionId)await invoke('selection_clear',{selectionId:null});});
      if(!originalTrust)await cleanup('trust',()=>invoke('set_trust',{projectId,trusted:false}));
    }
    if(host)await cleanup('detach',()=>invoke('detach'));
    view.close();
  }
  if(host&&host.exitCode===null)host.kill();
  if(dockHandle&&originalWindow){
    try {
      const current=action('info',dockHandle);
      if(current.processId===ownedPid)action('move',dockHandle,{X:originalWindow.left,Y:originalWindow.top,Width:originalWindow.right-originalWindow.left,Height:originalWindow.bottom-originalWindow.top});
      else checks.push({status:'FAIL',name:'Owned window lifecycle changed before geometry restore; another window was not moved'});
    } catch(error){checks.push({status:'FAIL',name:`Owned window unavailable during geometry restore: ${error.message}`});}
  }
  report.status=checks.some((check)=>check.status==='FAIL')?'FAIL':checks.some((check)=>check.status==='BLOCKED')?'BLOCKED':'PASS';
  mkdirSync(evidence,{recursive:true});writeFileSync(join(evidence,'report.json'),`${JSON.stringify(report,null,2)}\n`);
  console.log(`${report.status} real SDK Inspector UI: ${JSON.stringify(checks)}`);
}
