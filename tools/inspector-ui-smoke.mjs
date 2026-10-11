// Actual Windows WebView2 + SDK Inspector. Only uses an explicitly owned task PID.
// This driver neither supplies native frames nor sends any Agent message.
import assert from 'node:assert/strict';
import { spawn, execFileSync } from 'node:child_process';
import { readFileSync, writeFileSync, mkdirSync, existsSync } from 'node:fs';
import { resolve, join } from 'node:path';
import { createHash } from 'node:crypto';
import { connectWebview } from './webview-driver.mjs';
import { requireOwnedTargetEdge } from './window-input.mjs';

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
const snapshot = () => view.evaluate('window.__TAURI__.core.invoke("snapshot")');
async function invoke(command,args={}) {
  const state=await snapshot();
  return view.evaluate(`window.__TAURI__.core.invoke(${JSON.stringify(command)},${JSON.stringify({...args,scope:{targetId:state.target?.id??null,sessionId:state.sessionId}})})`);
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
  const point=await until('UI control ready',()=>view.evaluate(`(() => {const e=${expression};if(!e||e.disabled)return null;e.scrollIntoView({block:'center'});const r=e.getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2};})()`));
  await clickPoint(point);
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
async function verifyMcp(selection) {
  if (!process.env.APPDATA) blocked('Explicit Windows APPDATA unavailable; selection store was not scanned');
  const directory=join(process.env.APPDATA,'dev.arkdock.desktop','selection-context-v1');
  if(!existsSync(join(directory,selection.selectionId,'context.json'))) blocked('Expected task selection artifact unavailable in own selection subdirectory; no scan performed');
  const capability=join(evidence,'mcp-grant.json');
  const grant={version:1,directory,workspaceId:selection.workspaceId,sessionId:selection.sessionId,
    selectionIds:[selection.selectionId],allowedTools:['arkdock.get_current_selection','arkdock.get_selection','arkdock.get_selection_context','arkdock.get_selection_screenshot'],expiresAt:Date.now()+60000};
  writeFileSync(capability,`${JSON.stringify(grant,null,2)}\n`);
  const child=spawn(process.execPath,[resolve('tools/selection-provider.mjs'),'--directory',directory,
    '--workspace',selection.workspaceId,'--session',selection.sessionId,'--capability',capability],
  {windowsHide:true,stdio:['pipe','pipe','pipe']});
  let serial=0,buffer='',errors='';const pending=new Map();
  child.stdout.setEncoding('utf8');child.stderr.setEncoding('utf8');
  child.stdin.on('error',()=>{/* The owned provider may exit after a rejected grant. */});
  child.stderr.on('data',(chunk)=>{errors+=chunk;});
  child.stdout.on('data',(chunk)=>{
    buffer+=chunk;let newline;
    while((newline=buffer.indexOf('\n'))>=0){
      const line=buffer.slice(0,newline);buffer=buffer.slice(newline+1);
      let message;try{message=JSON.parse(line);}catch{continue;}
      const entry=pending.get(message.id);if(!entry)continue;pending.delete(message.id);clearTimeout(entry.timer);
      if(message.error)entry.reject(new Error(message.error.message));else entry.done(message.result);
    }
  });
  child.once('error',(error)=>{for(const entry of pending.values()){clearTimeout(entry.timer);entry.reject(error);}pending.clear();});
  child.once('exit',(code)=>{for(const entry of pending.values()){clearTimeout(entry.timer);entry.reject(new Error(`MCP provider exit ${code}: ${errors.trim()}`));}pending.clear();});
  function request(method,params={}) {
    const id=++serial;
    return new Promise((done,reject)=>{
      const timer=setTimeout(()=>{pending.delete(id);reject(new Error(`MCP timeout: ${method}`));},10000);
      pending.set(id,{done,reject,timer});child.stdin.write(`${JSON.stringify({jsonrpc:'2.0',id,method,params})}\n`);
    });
  }
  try {
    const initialized=await request('initialize',{protocolVersion:'2025-06-18',capabilities:{},clientInfo:{name:'arkdock-native-selection-smoke',version:'1'}});
    child.stdin.write(`${JSON.stringify({jsonrpc:'2.0',method:'notifications/initialized'})}\n`);
    const listed=await request('tools/list');assert.equal(listed.tools.length,4);
    const context=await request('tools/call',{name:'arkdock.get_selection_context',arguments:{selectionId:selection.selectionId}});
    assert.equal(context.isError,undefined);assert.equal(context.structuredContent.selectionId,selection.selectionId);
    assert.equal(context.structuredContent.userInstruction,selection.userInstruction);
    const hashes=[];
    for(const crop of [false,true]){
      const result=await request('tools/call',{name:'arkdock.get_selection_screenshot',arguments:{selectionId:selection.selectionId,crop}});
      assert.equal(result.isError,undefined);assert.equal(result.content[0].mimeType,'image/jpeg');
      const bytes=Buffer.from(result.content[0].data,'base64'),metadata=crop?selection.crop:selection.screenshot;
      const hash=createHash('sha256').update(bytes).digest('hex');assert.equal(hash,metadata.sha256);hashes.push({crop,sha256:hash,byteLength:bytes.length});
    }
    const denied=await request('tools/call',{name:'arkdock.get_selection',arguments:{selectionId:'sel-not-authorized'}});
    assert.equal(denied.isError,true);assert.equal(denied.content[0].text,'SELECTION_NOT_AUTHORIZED');
    return {protocolVersion:initialized.protocolVersion,tools:listed.tools.map((tool)=>tool.name),selectionId:selection.selectionId,
      sourceStatus:context.structuredContent.sourceEvidence.status,hashes,deniedOtherSelection:true,agentMessagesSent:0};
  } finally {
    if(!child.stdin.destroyed)child.stdin.end();
    if(child.exitCode===null){await Promise.race([new Promise((done)=>child.once('exit',done)),pause(1000)]);if(child.exitCode===null)child.kill();}
  }
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
  const before=action('info',dockHandle),destination=action('info',target.first);
  requireOwnedTargetEdge(action,target.first,destination,ownedPid);
  const drag=await view.evaluate('(() => {const r=document.querySelector(".drag").getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2,ratio:devicePixelRatio};})()');
  const x=Math.round(before.clientX+drag.x*drag.ratio),y=Math.round(before.clientY+drag.y*drag.ratio);
  report.drag=action('drag',dockHandle,{X:x,Y:y,EndX:x+destination.right-before.left,EndY:y+destination.top-before.top,OutlinePid:ownedPid});
  const found=await until('Owned fixture discovery',async()=>{const s=await snapshot();return s.target?.process.pid===target.pid&&s.phase==='selectProject'&&s.candidates.some((c)=>normalizedPath(c.project.appRoot)===normalizedPath(project))&&s;},20000);
  const candidate=found.candidates.find((c)=>normalizedPath(c.project.appRoot)===normalizedPath(project));
  await clickControl(`[...document.querySelectorAll('.candidate-list button')].find(b=>b.querySelector('code')?.textContent===${JSON.stringify(candidate.project.appRoot)})`);
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
    await clickControl('document.querySelector("[data-testid=inspector-toggle]")');
  await view.evaluate(`(() => {
    const originalBridge=window.__TAURI__,original=originalBridge.core.invoke;
    const smoke=window.__arkdockInspectorSmoke={trees:[],selections:[],creates:[],calls:[],errors:[]};
    window.__TAURI__={...originalBridge,core:{...originalBridge.core,invoke:async(command,args)=>{
      smoke.calls.push({command,at:performance.now()});
      try {const result=await original(command,args);if(command==='inspector_get_tree')smoke.trees.push(result);if(command==='selection_create'){smoke.selections.push(result);smoke.creates.push(args.draft);}return result;}
      catch(error){smoke.errors.push({command,error:String(error)});throw error;}
    }}};smoke.cleanup=()=>{window.__TAURI__=originalBridge;};
  })()`);
  let tree,selection;
  await check('Actual SDK inspector tree, overlay hover and component pick do not trigger native onClick',async()=>{
    const count=nativeCount();
    await clickControl('document.querySelector("[data-testid=inspector-toggle]")');
    tree=await until('Real Inspector tree',()=>view.evaluate('window.__arkdockInspectorSmoke.trees.at(-1)'));
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
      const s=await window.__TAURI__.core.invoke('snapshot');const scope={targetId:s.target.id,sessionId:s.sessionId};const selected=window.__arkdockInspectorSmoke.selections.at(-1);
      const results=[];for(const crop of [false,true]){
        const raw=await window.__TAURI__.core.invoke('selection_read_image',{selectionId:selected.selectionId,crop,scope});
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
  await check('Bad build and wrong scope are rejected; Codex/Claude have no false submission',async()=>{
    const draft=await view.evaluate('window.__arkdockInspectorSmoke.creates.at(-1)');
    await assert.rejects(invoke('selection_create',{draft:{...draft,frame:{...draft.frame,buildId:'wrong-build'}}}),/STALE_SELECTION_FRAME|SELECTION_BUILD_STALE/);
    const state=await snapshot();
    await assert.rejects(view.evaluate(`window.__TAURI__.core.invoke('selection_get',${JSON.stringify({selectionId:selection.selectionId,scope:{targetId:state.target.id,sessionId:'wrong-session'}})})`),/SESSION|SCOPE|STALE/);
    await view.evaluate('document.querySelector("#selection-instruction").focus()');
    await view.send('Input.insertText',{text:'Smoke test context only; do not send or modify source.'});
    await clickControl('[...document.querySelectorAll(".selection-actions button")].find(b=>b.textContent==="保存选择上下文")');
    selection=await until('User requirement saved with selection',async()=>{const value=await invoke('selection_get',{selectionId:selection.selectionId});return value.userInstruction==='Smoke test context only; do not send or modify source.'&&value;});
    const prompt=await invoke('selection_prompt',{selectionId:selection.selectionId});assert.ok(prompt.includes(selection.selectionId));assert.ok(prompt.includes(selection.userInstruction));
    report.promptVerified=true;
    report.adapters=await invoke('agent_session_list');
    assert.equal(report.adapters.adapters.length,2);assert.ok(report.adapters.adapters.every((adapter)=>adapter.status==='UNSUPPORTED'&&!adapter.capabilities.directSend));
    const prepared=await invoke('agent_delivery_prepare',{selectionId:selection.selectionId,bindingId:null});
    const duplicate=await invoke('agent_delivery_prepare',{selectionId:selection.selectionId,bindingId:null});
    assert.equal(prepared.requestId,duplicate.requestId);assert.equal(prepared.state,'UNSUPPORTED');
    // Do not call send: this gate verifies unsupported status without initiating
    // any external submission, even if a future adapter becomes available.
    report.delivery=await invoke('agent_delivery_status',{requestId:prepared.requestId});
    assert.equal(report.delivery.state,'UNSUPPORTED');assert.equal(report.delivery.receiptId,null);
    assert.equal(await view.evaluate('[...document.querySelectorAll(".selection-actions button")].find(b=>b.textContent.includes("发送到当前 Agent"))?.disabled'),true);
  });
  await check('Real saved native context and JPEGs are readable through explicitly scoped stdio MCP',async()=>{
    report.mcp=await verifyMcp(selection);
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
    await clickControl('[...document.querySelectorAll(".inspection-status button")].find(b=>b.textContent==="框选")');
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
