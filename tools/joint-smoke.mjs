// Real Windows joint-layout checks. Owns only its fixture windows; no native-preview fixtures.
import assert from 'node:assert/strict';
import {spawn,execFileSync} from 'node:child_process';
import {mkdirSync,writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {connectWebview} from './webview-driver.mjs';
import {activateOwnedHeader,requireOwnedTargetEdge} from './window-input.mjs';
const evidence=process.env.ARKDOCK_EVIDENCE_DIR ?? 'artifacts/joint-layout-2026-10-09';
const ownedPid=Number(process.env.ARKDOCK_TEST_PID ?? process.env.ARKDOCK_PROBE_PID);
const debugPort=Number(process.env.ARKDOCK_TEST_PORT ?? process.env.ARKDOCK_PROBE_PORT ?? 9223);
const checks=[],measurements=[];
const pause=ms=>new Promise(done=>setTimeout(done,ms));
let view,host,dockHandle,dockPid,target;
function action(kind,handle,values={}) {
  const args=['-NoProfile','-File',resolve('tests/native/window-actions.ps1'),'-Action',kind,'-Handle',String(handle)];
  for(const [key,value] of Object.entries(values))args.push(`-${key}`,String(value));
  return JSON.parse(execFileSync('pwsh',args,{encoding:'utf8',windowsHide:true,timeout:12_000}));
}
const snapshot=()=>view.evaluate('window.__TAURI__.core.invoke("snapshot")');
async function invoke(command,args={}) {
  const state=await snapshot();
  return view.evaluate(`window.__TAURI__.core.invoke(${JSON.stringify(command)},${JSON.stringify({...args,scope:{targetId:state.target?.id??null,sessionId:state.sessionId}})})`);
}
async function until(name,predicate,timeout=15_000) {
  const deadline=Date.now()+timeout;
  while(Date.now()<deadline){const state=await snapshot();if(await predicate(state))return state;await pause(100);}
  throw new Error(`${name} timeout; ${JSON.stringify(await snapshot())}`);
}
const bounds=()=>({target:action('info',target.first),dock:action('info',dockHandle)});
function joined(value) {
  assert.equal(value.target.frameRight,value.dock.frameLeft,'DWM visible frames have a zero seam');
  assert.equal(value.target.frameTop,value.dock.frameTop,'Visible top boundaries match');
  assert.equal(value.target.frameBottom,value.dock.frameBottom,'Visible bottom boundaries match');
  assert.equal(value.dock.owner,0,'ArkDock remains an independent top-level window');
  assert.equal(value.target.owner,0,'Target remains an independent top-level window');
  assert.equal(value.dock.extended&8,0,'ArkDock is not made globally topmost');
  const width=value.dock.clientWidth/(value.dock.dpi/96);
  assert.ok(width>=360&&width<=560,`Actual Dock client width ${width} DIP`);
}
async function paired(name='paired') {
  const state=await until(name,async state=>{
    if(state.pairLayout?.state!=='paired')return false;
    const value=bounds();return value.target.frameRight===value.dock.frameLeft&&value.target.frameTop===value.dock.frameTop&&value.target.frameBottom===value.dock.frameBottom;
  });
  const value=bounds();joined(value);measurements.push({name,state:state.pairLayout,...value});return value;
}
async function screenshot(name) {const result=await view.send('Page.captureScreenshot',{format:'png'});writeFileSync(`${evidence}/${name}.png`,Buffer.from(result.data,'base64'));}
async function check(id,name,execute){await execute();checks.push({id,status:'PASS',name});console.log(`PASS ${id} ${name}`);}
async function closePanel(){if(await view.evaluate('!!document.querySelector("dialog")')){await view.send('Input.dispatchKeyEvent',{type:'keyDown',key:'Escape',code:'Escape',windowsVirtualKeyCode:27});await view.send('Input.dispatchKeyEvent',{type:'keyUp',key:'Escape',code:'Escape',windowsVirtualKeyCode:27});await pause(100);}}
async function bind(alreadyMaximized=false) {
  await invoke('detach');await until('detached',state=>!state.target);await closePanel();
  action('restore',dockHandle);
  action('restore',target.first);
  const normalDeadline=Date.now()+5000;
  while(action('info',target.first).maximized&&Date.now()<normalDeadline)await pause(100);
  assert.equal(action('info',target.first).maximized,false,'Fixture normal placement is acknowledged before setting the initial test rectangle');
  action('move',target.first,{X:120,Y:100,Width:1000,Height:1320});
  if(alreadyMaximized)action('maximize',target.first);
  action('move',target.second,{X:60,Y:500,Width:400,Height:400});
  action('move',dockHandle,{X:1800,Y:100,Width:906,Height:1320});
  action('foreground',target.first);action('foreground',dockHandle);await pause(200);
  await activateOwnedHeader(view,dockHandle,action);
  action('raise',target.first,{ProcessId:target.pid});
  action('raise',dockHandle,{ProcessId:dockPid});
  const own=action('info',dockHandle),destination=action('info',target.first);
  requireOwnedTargetEdge(action,target.first,destination,dockPid);
  const button=await view.evaluate('(()=>{const r=document.querySelector(".drag").getBoundingClientRect();return{x:r.x+r.width/2,y:r.y+r.height/2,ratio:devicePixelRatio}})()');
  const x=Math.round(own.clientX+button.x*button.ratio),y=Math.round(own.clientY+button.y*button.ratio);
  action('drag',dockHandle,{X:x,Y:y,EndX:x+destination.frameRight-own.frameLeft,EndY:y+destination.frameTop-own.frameTop,OutlinePid:dockPid});
  await until('actual target binding',state=>state.target?.process.pid===target.pid);if(alreadyMaximized)await until('already maximized binding',state=>state.pairLayout?.state==='maximized');else await paired();await closePanel();
}
try {
  mkdirSync(evidence,{recursive:true});
  assert.ok(Number.isSafeInteger(ownedPid)&&ownedPid>0,'Explicit task-owned ArkDock PID required');
  view=await connectWebview(debugPort,5000,process.env.ARKDOCK_TEST_URL ?? 'http://127.0.0.1:1420/',ownedPid);
  assert.equal((await snapshot()).target,null,'Dedicated test instance must begin detached');
  dockPid=ownedPid;dockHandle=action('find',0,{ProcessId:dockPid}).handle;assert.ok(dockHandle);
  host=spawn('pwsh',['-NoProfile','-File',resolve('tests/native/window-host.ps1')],{windowsHide:true,stdio:['ignore','pipe','pipe']});
  target=await new Promise((done,reject)=>{
    let output='',errors='';const timer=setTimeout(()=>reject(new Error(`Fixture startup ${errors}`)),10000);
    host.stderr.on('data',data=>{errors+=data;});host.stdout.on('data',data=>{output+=data;if(output.includes('\n')){clearTimeout(timer);done(JSON.parse(output.split('\n')[0]));}});
    host.once('exit',code=>{clearTimeout(timer);reject(new Error(`Owned fixture exited ${code}`));});
  });
  await check('J0','Unbound native resize beside another window never creates a binding',async()=>{
    await invoke('detach');action('restore',dockHandle);action('move',target.first,{X:120,Y:100,Width:1000,Height:1320});
    action('move',dockHandle,{X:1120,Y:100,Width:906,Height:1320});action('foreground',dockHandle);await pause(100);
    const before=action('info',dockHandle);action('drag',dockHandle,{X:before.right-2,Y:before.bottom-2,EndX:before.right+78,EndY:before.bottom+38});await pause(200);
    assert.equal((await snapshot()).target,null);assert.equal((await snapshot()).pairLayout.state,'unbound');
  });
  await check('J1','Actual header drag creates full-contact equal-height independent pair',async()=>{
    await bind();const value=await paired();assert.equal(value.dock.clientWidth/(value.dock.dpi/96),440);
    assert.equal((await snapshot()).trusted,false,'Window association never authorizes project execution');await screenshot('joint-docked');
  });
  await check('J1b','Binding an already Windows-maximized target immediately reserves the real right pane',async()=>{
    await bind(true);const value=bounds();joined(value);assert.equal(value.target.maximized,true);assert.equal(value.dock.frameRight,value.target.workRight);assert.equal(value.target.frameLeft,value.target.workLeft);
    await invoke('restore_pair');await paired('already-maximized target restore');
    await bind();
  });
  await check('J2','Target external resize updates real Dock height while retaining preferred width',async()=>{
    const before=bounds();const identity=(await snapshot()).target.id;
    action('move',target.first,{X:before.target.left-120,Y:before.target.top,Width:before.target.right-before.target.left+120,Height:before.target.bottom-before.target.top+60});
    const after=await paired('target outer-left resized');assert.equal(after.dock.clientWidth/(after.dock.dpi/96),440);assert.equal((await snapshot()).target.id,identity);assert.ok(after.target.frameLeft<before.target.frameLeft);assert.equal(after.target.frameRight,before.target.frameRight);
  });
  await check('J3','Real Dock outer-corner resize grows Agent remainder without re-association',async()=>{
    await closePanel();const before=bounds(),identity=(await snapshot()).target.id;
    action('foreground',dockHandle);action('drag',dockHandle,{X:before.dock.right-2,Y:before.dock.bottom-2,EndX:before.dock.right+118,EndY:before.dock.bottom+38});
    const after=await paired('Dock pointer resized');assert.equal((await snapshot()).target.id,identity);assert.equal(after.target.frameLeft,before.target.frameLeft);assert.equal(after.dock.clientWidth/(after.dock.dpi/96),440);assert.ok(after.target.frameRight>before.target.frameRight);
  });
  let normal;
  await check('J4','Focus-safe header maximize confirms actual shared workarea and pending acknowledgment',async()=>{
    normal=bounds();action('foreground',dockHandle);const foregroundBefore=action('info',dockHandle).foreground;const reply=await invoke('maximize_pair');assert.equal(reply.pairLayout.state,'maximizing','Accepted request remains pending until actual geometry matches');
    await until('maximized',state=>state.pairLayout?.state==='maximized');const value=bounds();joined(value);
    assert.equal(value.target.maximized,false,'No individual native maximize is fabricated by the focus-safe group command');assert.equal(value.target.frameLeft,value.target.workLeft);assert.equal(value.target.frameTop,value.target.workTop);assert.equal(value.dock.frameRight,value.target.workRight);assert.equal(value.dock.frameBottom,value.target.workBottom);
    assert.equal(value.dock.foreground,foregroundBefore,'Shared maximize never changes foreground focus');
    measurements.push({name:'maximized',...value});await screenshot('joint-maximized');
  });
  await check('J5','Restore returns the previous paired normal visible geometry exactly',async()=>{
    const reply=await invoke('restore_pair');assert.equal(reply.pairLayout.state,'restoring');const value=await paired('restored');assert.equal(value.target.maximized,false);
    for(const key of ['frameLeft','frameTop','frameRight','frameBottom']){assert.equal(value.target[key],normal.target[key],`target ${key}`);assert.equal(value.dock[key],normal.dock[key],`dock ${key}`);}
  });
  await check('J6','Target Windows maximize/restore also coordinates the shared workarea',async()=>{
    normal=bounds();action('maximize',target.first);await until('OS maximized group',state=>state.pairLayout?.state==='maximized');const value=bounds();joined(value);assert.equal(value.dock.frameRight,value.target.workRight);
    action('restore',target.first);const restored=await paired('OS restore');for(const key of ['frameLeft','frameTop','frameRight','frameBottom'])assert.equal(restored.target[key],normal.target[key]);
  });
  await check('J6c','Own Windows maximize coordinates workarea without exceeding the preferred pane width',async()=>{
    const before=bounds();joined(before);
    action('maximize',dockHandle);await until('own OS maximized group',state=>state.pairLayout?.state==='maximized');
    const value=bounds();joined(value);assert.equal(value.dock.maximized,false);assert.equal(value.dock.clientWidth/(value.dock.dpi/96),440);assert.equal(value.target.frameLeft,value.target.workLeft);assert.equal(value.dock.frameRight,value.target.workRight);
    await invoke('restore_pair');const restored=await paired('own OS max restore');
    for(const key of ['frameLeft','frameTop','frameRight','frameBottom']){
      assert.equal(restored.target[key],before.target[key],`own OS max restore target ${key}`);
      assert.equal(restored.dock[key],before.dock[key],`own OS max restore dock ${key}`);
    }
    assert.equal(restored.dock.clientWidth/(restored.dock.dpi/96),440);
  });
  await check('J6d','Resizing out of shared maximum changes confirmed mode to paired and keeps a zero seam',async()=>{
    await invoke('maximize_pair');await until('maximum before resize',state=>state.pairLayout?.state==='maximized');
    const before=bounds();action('move',dockHandle,{X:before.dock.left,Y:before.dock.top,Width:before.dock.right-before.dock.left-120,Height:before.dock.bottom-before.dock.top});
    const value=await paired('resize out of maximum');assert.equal(value.dock.clientWidth/(value.dock.dpi/96),440);assert.ok(value.dock.frameRight<value.target.workRight);
  });
  await check('J7','User resized geometry is preserved when detaching and closing target removes the session',async()=>{
    const before=bounds();await invoke('detach');await until('explicit detached',state=>state.target===null);await pause(300);const after=action('info',target.first);
    for(const key of ['left','top','right','bottom'])assert.equal(after[key],before.target[key],`User geometry ${key}`);
    await bind();action('close',target.first);const state=await until('target closed',state=>state.target===null);assert.equal(state.sessionId,null);assert.equal(state.project,null);assert.equal(state.pairLayout.state,'unbound');
  });
} catch(error){try{if(target&&dockHandle)measurements.push({name:'failure-before-cleanup',...bounds()});}catch{/* An owned fixture may already be closed. */}const blocked=error.blocked||error.message.includes('BLOCKED:');checks.push({status:blocked?'BLOCKED':'FAIL',name:error.message});console.error(error);process.exitCode=blocked?2:1;}
finally {
  if(view){try{await invoke('detach');}catch{/* App may have restarted or exited. */}view.close();}
  if(host&&host.exitCode===null)host.kill();
  const status=checks.some(check=>check.status==='FAIL')?'FAIL':checks.some(check=>check.status==='BLOCKED')?'BLOCKED':'PASS';
  mkdirSync(evidence,{recursive:true});writeFileSync(`${evidence}/joint-smoke.json`,JSON.stringify({capturedAt:new Date().toISOString(),status,platform:process.platform,dockPid,scope:'Actual independent Windows group layout; no HarmonyOS rendering assertions',checks,measurements},null,2)+'\n');
  process.exitCode=status==='FAIL'?1:status==='BLOCKED'?2:0;
}
