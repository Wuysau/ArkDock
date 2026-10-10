// Explicitly named real user target: geometry and process metadata only, no screen/chat capture.
import assert from 'node:assert/strict';
import {execFileSync} from 'node:child_process';
import {mkdirSync,writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {connectWebview} from './webview-driver.mjs';
const arg=name=>Number(process.argv[process.argv.indexOf(name)+1]);
const targetHandle=arg('--target-handle'),targetPid=arg('--target-pid');
assert.ok(Number.isSafeInteger(targetHandle)&&targetHandle>0);
assert.ok(Number.isSafeInteger(targetPid)&&targetPid>0);
const evidence=process.env.ARKDOCK_EVIDENCE_DIR ?? 'artifacts/joint-layout-2026-10-09';
const action=(kind,handle,values={})=>{const args=['-NoProfile','-File',resolve('tests/native/window-actions.ps1'),'-Action',kind,'-Handle',String(handle)];for(const[key,value]of Object.entries(values))args.push('-'+key,String(value));return JSON.parse(execFileSync('pwsh',args,{encoding:'utf8',windowsHide:true,timeout:12000}));};
const pause=ms=>new Promise(done=>setTimeout(done,ms));
let view,result,ownHandle,actualPid,beforeTarget,state,target,dock;
const measurements=[];
async function invoke(command) {
  const current=await view.evaluate('window.__TAURI__.core.invoke("snapshot")');
  assert.equal(current.target?.process.pid,targetPid,'Named target must still own the association');
  return view.evaluate(`window.__TAURI__.core.invoke(${JSON.stringify(command)},${JSON.stringify({scope:{targetId:current.target.id,sessionId:current.sessionId}})})`);
}
async function waitJoined(mode) {
  const deadline=Date.now()+15_000;
  while(Date.now()<deadline){
    state=await view.evaluate('window.__TAURI__.core.invoke("snapshot")');
    assert.equal(state.target?.process.pid,targetPid,'Association cannot switch while testing');
    target=action('info',targetHandle);dock=action('info',ownHandle);
    assert.equal(target.processId,targetPid,'Named target lifecycle must remain live');
    if(state.pairLayout?.state===mode&&target.frameRight===dock.frameLeft&&target.frameTop===dock.frameTop&&target.frameBottom===dock.frameBottom)return;
    await pause(100);
  }
  throw new Error(`Named target did not confirm ${mode} geometry`);
}
try{
  const ready=Date.now()+45000;
  while(!view&&Date.now()<ready){try{view=await connectWebview();}catch{await pause(250);}}
  assert.ok(view,'Current actual development app must be running');
  actualPid=JSON.parse(execFileSync('pwsh',['-NoProfile','-Command','Get-Process -Name arkdock | Select-Object -First 1 Id | ConvertTo-Json -Compress'],{encoding:'utf8',windowsHide:true})).Id;
  ownHandle=action('find',0,{ProcessId:actualPid}).handle;
  beforeTarget=action('info',targetHandle);
  assert.equal(beforeTarget.processId,targetPid,'Handle belongs to the explicitly named target before any action');
  if(!beforeTarget.maximized && process.argv.includes('--maximize-target')){
    action('maximize',targetHandle);
    const deadline=Date.now()+5000;
    while(!action('info',targetHandle).maximized&&Date.now()<deadline)await pause(100);
    assert.equal(action('info',targetHandle).maximized,true,'Explicit test setup must receive native MAX acknowledgment');
  }
  assert.equal(action('info',targetHandle).maximized,true,'This probe targets the reported already-maximized user window');
  const current=await view.evaluate('window.__TAURI__.core.invoke("snapshot")');
  if(current.target){await view.evaluate(`window.__TAURI__.core.invoke("detach",${JSON.stringify({scope:{targetId:current.target.id,sessionId:current.sessionId}})})`);await pause(300);}
  action('restore',ownHandle);action('move',ownHandle,{X:1800,Y:90,Width:906,Height:1655});
  // Explicit test action: the named target must be visible at the tested edge.
  action('foreground',targetHandle);action('foreground',ownHandle);await pause(150);
  const own=action('info',ownHandle);
  const button=await view.evaluate('(()=>{const r=document.querySelector(".drag").getBoundingClientRect();return{x:r.x+r.width/2,y:r.y+r.height/2,ratio:devicePixelRatio}})()');
  const x=Math.round(own.clientX+button.x*button.ratio),y=Math.round(own.clientY+button.y*button.ratio);
  action('drag',ownHandle,{X:x,Y:y,EndX:beforeTarget.workRight-1,EndY:y,OutlinePid:actualPid});
  const deadline=Date.now()+20000;
  while(Date.now()<deadline){state=await view.evaluate('window.__TAURI__.core.invoke("snapshot")');if(state.target?.process.pid===targetPid&&state.pairLayout?.state==='maximized')break;await pause(100);}
  target=action('info',targetHandle);dock=action('info',ownHandle);
  assert.equal(state.target?.process.pid,targetPid,'Actual target lifecycle matches the explicitly named user window');
  assert.equal(state.pairLayout.state,'maximized','The actual group must be confirmed');
  // Reported custom-frame app has its invisible MAX border included in raw DWM bounds.
  assert.equal(target.clientX+target.clientWidth,dock.frameLeft,'Real target client right touches real Dock visible left');
  assert.equal(target.clientX,target.workLeft);assert.equal(target.clientY,target.workTop);assert.equal(target.clientY+target.clientHeight,target.workBottom);
  assert.equal(dock.frameTop,target.workTop);assert.equal(dock.frameBottom,target.workBottom);assert.equal(dock.frameRight,target.workRight);
  assert.equal(dock.clientWidth/(dock.dpi/96),440);assert.equal(dock.owner,0);assert.equal(target.owner,0);
  assert.equal(state.trusted,false,'Window layout does not authorize project execution');
  measurements.push({name:'actual shared workarea',target,dock,pairLayout:state.pairLayout});
  if(process.argv.includes('--exercise-resize')){
    await invoke('restore_pair');await waitJoined('paired');
    const beforeResize={target,dock};
    action('move',targetHandle,{X:target.left+40,Y:target.top,Width:target.right-target.left-40,Height:target.bottom-target.top});
    await waitJoined('paired');
    assert.ok(target.frameLeft>beforeResize.target.frameLeft,'Actual Agent outer-left resize changes the shared boundary');
    assert.equal(dock.clientWidth/(dock.dpi/96),440);
    measurements.push({name:'actual Agent resize',target,dock,pairLayout:state.pairLayout});
    const priorRight=dock.frameRight;
    action('move',ownHandle,{X:dock.left,Y:dock.top,Width:dock.right-dock.left-80,Height:dock.bottom-dock.top});
    await waitJoined('paired');
    assert.ok(dock.frameRight<priorRight,'Actual Dock resize changes the shared outer boundary');
    assert.equal(dock.clientWidth/(dock.dpi/96),440);
    measurements.push({name:'actual Dock resize',target,dock,pairLayout:state.pairLayout});
    await invoke('maximize_pair');await waitJoined('maximized');
    assert.equal(target.frameLeft,target.workLeft);assert.equal(dock.frameRight,target.workRight);
    assert.equal(dock.frameTop,target.workTop);assert.equal(dock.frameBottom,target.workBottom);
    measurements.push({name:'final shared workarea',target,dock,pairLayout:state.pairLayout});
  }
  result={status:'PASS',capturedAt:new Date().toISOString(),scope:'Actual named user target+ArkDock metadata/geometry only; no chat/screen capture or project execution.',dockPid:actualPid,targetPid,targetHandle,initialTarget:beforeTarget,target,dock,pairLayout:state.pairLayout,measurements,associationLeftIntact:true};
  console.log('PASS actual named user target+ArkDock zero-contact shared workarea');
}catch(error){
  result={status:'FAIL',capturedAt:new Date().toISOString(),scope:'Actual named user target geometry only',reason:error.message,dockPid:actualPid,targetPid,targetHandle,initialTarget:beforeTarget,target,dock,pairLayout:state?.pairLayout,boundProcess:state?.target?.process};console.error(error);process.exitCode=1;
  if(view&&state?.target&&state.target.process.pid!==targetPid){try{await view.evaluate(`window.__TAURI__.core.invoke("detach",${JSON.stringify({scope:{targetId:state.target.id,sessionId:state.sessionId}})})`);}catch{/* No stale retry against a changed user target. */}}
}finally{view?.close();mkdirSync(evidence,{recursive:true});writeFileSync(`${evidence}/actual-user-target-geometry.json`,JSON.stringify(result,null,2)+'\n');}
