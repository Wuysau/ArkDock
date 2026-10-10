// Actual development WebView2 assertions. Never supplies snapshots or preview frames.
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { connectOwnedWebview as connectWebview } from './webview-driver.mjs';
import { ownedDesktop } from './owned-desktop.mjs';

const checks = [], dimensions = [], pointerResizes = [];
const evidence = process.env.ARKDOCK_EVIDENCE_DIR ?? 'artifacts/ui-2026-10-09/native';
const pause = (ms) => new Promise((done) => setTimeout(done, ms));
function action(kind, handle, values = {}) {
  const args = ['-NoProfile', '-File', resolve('tests/native/window-actions.ps1'), '-Action', kind, '-Handle', String(handle)];
  for (const [key, value] of Object.entries(values)) args.push(`-${key}`, String(value));
  return JSON.parse(execFileSync('pwsh', args, { encoding: 'utf8', windowsHide: true, timeout: 12_000 }));
}
let view, handle, original, originalTheme, originalMotion;
async function click(selector) {
  const point = await view.evaluate(`(() => { const e=document.querySelector(${JSON.stringify(selector)}); if(!e || e.disabled) return null; const r=e.getBoundingClientRect(); return {x:r.x+r.width/2,y:r.y+r.height/2}; })()`);
  assert.ok(point, `Enabled control required: ${selector}`);
  await view.send('Input.dispatchMouseEvent', {type:'mousePressed', ...point, button:'left', clickCount:1});
  await view.send('Input.dispatchMouseEvent', {type:'mouseReleased', ...point, button:'left', clickCount:1});
  await pause(160);
}
async function clickText(label) {
  assert.equal(await view.evaluate(`(() => { const e=[...document.querySelectorAll('dialog button')].find(item=>item.textContent.trim()===${JSON.stringify(label)}); if(!e) return false; e.dataset.driverControl='current'; return true; })()`), true, label);
  await click('[data-driver-control="current"]');
  await view.evaluate('document.querySelector("[data-driver-control]")?.removeAttribute("data-driver-control")');
}
async function key(keyName, shift=false) {
  const codes = {Escape:27, Tab:9, Enter:13};
  const params = {key:keyName, code:keyName, windowsVirtualKeyCode:codes[keyName], modifiers:shift?8:0};
  await view.send('Input.dispatchKeyEvent', {type:'keyDown', ...params});
  await view.send('Input.dispatchKeyEvent', {type:'keyUp', ...params});
  await pause(130);
}
async function closePanel() {
  if(await view.evaluate('!!document.querySelector("dialog")')) {
    await key('Escape');
    assert.equal(await view.evaluate('!!document.querySelector("dialog")'), false, 'Esc closes the panel');
  }
}
async function settings() {
  await closePanel(); await click('[aria-label="更多操作"]'); await clickText('偏好设置');
  assert.equal(await view.evaluate('document.querySelector("dialog")?.getAttribute("aria-label")'), '偏好设置');
}
async function capture(name) {
  const result = await view.send('Page.captureScreenshot', {format:'png'});
  writeFileSync(`${evidence}/ui-${name}.png`, Buffer.from(result.data,'base64'));
}
const appearance = () => view.evaluate('({theme:document.documentElement.dataset.theme,motion:document.documentElement.dataset.motion,scheme:getComputedStyle(document.documentElement).colorScheme,background:getComputedStyle(document.documentElement).getPropertyValue("--surface").trim(),duration:getComputedStyle(document.querySelector(".drag")).transitionDuration})');
async function resizeLogical(width,height) {
  const native = action('info',handle);
  const current = await view.evaluate('({width:innerWidth,height:innerHeight,ratio:devicePixelRatio})');
  const chromeWidth = native.right-native.left-current.width*current.ratio;
  const chromeHeight = native.bottom-native.top-current.height*current.ratio;
  action('move',handle,{X:180,Y:90,Width:Math.round(width*current.ratio+chromeWidth),Height:Math.round(height*current.ratio+chromeHeight)});
  await pause(250);
  const actual = await view.evaluate('({width:innerWidth,height:innerHeight,ratio:devicePixelRatio,scrollWidth:document.documentElement.scrollWidth,scrollHeight:document.documentElement.scrollHeight})');
  dimensions.push({requested:{width,height},actual});
  assert.ok(Math.abs(actual.width-width)<=1, `Logical width ${width}: ${JSON.stringify(actual)}`);
  assert.ok(Math.abs(actual.height-height)<=1, `Logical height ${height}: ${JSON.stringify(actual)}`);
  assert.ok(actual.scrollWidth<=actual.width, `No horizontal document overflow: ${JSON.stringify(actual)}`);
  assert.ok(actual.scrollHeight<=actual.height, `Shell stays in viewport: ${JSON.stringify(actual)}`);
}
async function check(name,execute) { await execute(); checks.push({status:'PASS',name}); console.log(`PASS ${name}`); }
try {
  mkdirSync(evidence,{recursive:true});
  view = await connectWebview();
  const dock = ownedDesktop();
  handle = action('find',0,{ProcessId:dock.Id}).handle;
  assert.ok(handle,'The actual independent Tauri main window must be found');
  action('restore',handle); await pause(250);
  original = action('info',handle);
  ({theme:originalTheme,motion:originalMotion}=await appearance());
  await resizeLogical(440,820);
  await check('Actual light/dark themes change CSS and persist through reload',async()=>{
    await settings(); await clickText('浅色');
    const light=await appearance(); assert.equal(light.theme,'light'); assert.equal(light.scheme,'light');
    await closePanel(); await capture('light-440x820');
    await settings(); await clickText('深色');
    const dark=await appearance(); assert.equal(dark.theme,'dark'); assert.equal(dark.scheme,'dark'); assert.notEqual(light.background,dark.background);
    await closePanel(); await capture('dark-440x820');
    await view.send('Page.reload'); await pause(800); assert.equal((await appearance()).theme,'dark');
  });
  await check('System theme responds to light/dark media changes',async()=>{
    await settings(); await clickText('跟随系统');
    for(const scheme of ['light','dark']) { await view.send('Emulation.setEmulatedMedia',{features:[{name:'prefers-color-scheme',value:scheme}]}); await pause(100); assert.equal((await appearance()).scheme,scheme); }
    await clickText('深色'); await closePanel();
  });
  await check('360×640,440×820,520×960 logical client sizes fit controls in both themes',async()=>{
    for(const theme of ['深色','浅色']) {
      await settings(); await clickText(theme); await closePanel();
      for(const [width,height] of [[360,640],[440,820],[520,960]]) {
        await resizeLogical(width,height);
        const controls=await view.evaluate('({bridge:!!window.__TAURI__,header:!!document.querySelector(".header"),toolbar:!!document.querySelector(".preview-toolbar"),footer:document.querySelector(".footer").getBoundingClientRect().bottom,disabledPage:document.querySelector("select[aria-label=页面或组件]").disabled,disabledModes:[...document.querySelectorAll("select[aria-label=运行方式] option")].slice(1).every(item=>item.disabled)})');
        assert.equal(controls.bridge,true); assert.equal(controls.header,true); assert.equal(controls.toolbar,true);
        assert.ok(controls.footer<=height); assert.equal(controls.disabledPage,true); assert.equal(controls.disabledModes,true);
        await capture(`${theme==='深色'?'dark':'light'}-${width}x${height}`);
      }
    }
    await resizeLogical(440,820);
  });
  await check('Reduced motion persists and system preference changes real CSS timing',async()=>{
    await settings(); await clickText('减少动画'); assert.equal((await appearance()).motion,'reduce');
    assert.ok((await appearance()).duration.split(',').every(value=>Number.parseFloat(value)<=.001));
    await view.send('Page.reload'); await pause(800); assert.equal((await appearance()).motion,'reduce');
    await settings(); await clickText('跟随系统动画');
    await view.send('Emulation.setEmulatedMedia',{features:[{name:'prefers-reduced-motion',value:'reduce'}]});
    assert.ok((await appearance()).duration.split(',').every(value=>Number.parseFloat(value)<=.001));
    await view.send('Emulation.setEmulatedMedia',{features:[{name:'prefers-reduced-motion',value:'no-preference'}]});
    assert.ok((await appearance()).duration.split(',').some(value=>Number.parseFloat(value)>=.12)); await closePanel();
  });
  await check('Chinese project search, modal Tab confinement, Esc and restored trigger focus',async()=>{
    await click('[aria-label="更多操作"]'); await clickText('选择当前工程'); assert.equal(await view.evaluate('document.activeElement?.getAttribute("aria-label")'),'搜索工程');
    await view.send('Input.insertText',{text:'不存在的工程 中文搜索'});
    assert.equal(await view.evaluate('document.querySelector("input[aria-label=搜索工程]").value'),'不存在的工程 中文搜索');
    assert.equal(await view.evaluate('document.querySelector(".no-results h3").textContent'),'没有匹配的工程');
    await view.evaluate('document.querySelector("dialog button[aria-label=关闭面板]").focus()'); await key('Tab',true);
    assert.equal(await view.evaluate('document.activeElement?.closest("dialog")!==null'),true);
    await key('Escape'); assert.equal(await view.evaluate('!!document.querySelector("dialog")'),false);
    assert.equal(await view.evaluate('document.activeElement?.getAttribute("aria-label")'),'更多操作'); assert.equal(action('info',handle).visible,true);
  });
  await check('Diagnostics, SDK and capability panels show actual unavailable reasons',async()=>{
    await click('[aria-label="打开诊断"]'); assert.equal(await view.evaluate('document.querySelector("dialog")?.getAttribute("aria-label")'),'运行日志'); await capture('diagnostics'); await closePanel();
    await click('[aria-label="更多操作"]'); await clickText('设备与原生工具链'); assert.equal(await view.evaluate('document.querySelector("dialog h3").textContent'),'连接真实原生运行时');
    await closePanel(); await click('[aria-label="更多操作"]'); await clickText('预览能力');
    const unavailable=await view.evaluate('[...document.querySelectorAll(".capability-list > div")].filter(item=>["拖拽 / 滑动","文字输入 / IME","设备方向","鸿蒙应用主题","页面切换","设备切换","模拟器","真机"].includes(item.querySelector("strong").textContent)).map(item=>({status:item.querySelector(".badge").textContent,reason:item.querySelector("p").textContent}))');
    assert.equal(unavailable.length,8); for(const capability of unavailable) { assert.equal(capability.status,'unsupported'); assert.ok(capability.reason.length>5); } await closePanel();
  });
  await check('Real minimize button minimizes only ArkDock; restore and native resize work',async()=>{
    const before=action('info',handle); await click('[aria-label="最小化窗口"]'); await pause(250);
    assert.equal(action('info',handle).minimized,true); action('restore',handle); await pause(200);
    const restored=action('info',handle); assert.equal(restored.visible,true); assert.equal(restored.minimized,false); assert.equal(restored.owner,0); assert.equal(before.owner,0);
    await resizeLogical(520,960); await resizeLogical(440,820);
  });
  await check('Borderless native pointer edge resize retains usable independently resized WebView',async()=>{
    await resizeLogical(440,720);
    const before=action('info',handle);
    const viewport=await view.evaluate('({width:innerWidth,height:innerHeight,ratio:devicePixelRatio})');
    const pointer={X:before.right-2,Y:before.bottom-2,EndX:before.right+98,EndY:before.bottom+78};
    const clientPoint={x:(pointer.X-before.clientX)/viewport.ratio,y:(pointer.Y-before.clientY)/viewport.ratio};
    const hit=await view.evaluate(`(() => { const element=document.elementFromPoint(${clientPoint.x},${clientPoint.y}); return {element:element?{tag:element.tagName,className:element.className}:null,resizeHandlePresent:!!document.querySelector('.resize-handle'),focused:document.hasFocus()}; })()`);
    const foreground=action('foreground',handle); await pause(150);
    const dragged=action('drag',handle,pointer);
    await pause(200);
    const after=action('info',handle);
    const resized=await view.evaluate('({width:innerWidth,height:innerHeight,ratio:devicePixelRatio})');
    pointerResizes.push({handle,before,viewport,pointer,clientPoint,hit,foreground,dragged,after,resized,
      delta:{right:after.right-before.right,bottom:after.bottom-before.bottom,width:resized.width-viewport.width,height:resized.height-viewport.height}});
    assert.ok(after.right-before.right>=50,'Actual Win32 window width changes with pointer edge resize');
    assert.ok(after.bottom-before.bottom>=40,'Actual Win32 window height changes with pointer edge resize');
    assert.ok(resized.width>viewport.width && resized.height>viewport.height,'Actual WebView receives the native resize');
    assert.equal(after.owner,0); await resizeLogical(440,820);
  });
} catch(error) { console.error(error); checks.push({status:'FAIL',name:error.message}); process.exitCode=1; }
finally {
  if(view) {
    try { await view.send('Emulation.setEmulatedMedia',{features:[]}); await settings(); await clickText(originalTheme==='light'?'浅色':originalTheme==='system'?'跟随系统':'深色'); await clickText(originalMotion==='reduce'?'减少动画':'跟随系统动画'); await closePanel(); } catch { /* The app may have exited. */ }
    view.close();
  }
  if(handle && original) action('move',handle,{X:original.left,Y:original.top,Width:original.right-original.left,Height:original.bottom-original.top});
  const report={capturedAt:new Date().toISOString(),scope:'Actual Windows WebView2 UI/theme/motion/keyboard/window assertions; no HarmonyOS frame fixtures',status:checks.some(item=>item.status==='FAIL')?'FAIL':'PASS',dimensions,pointerResizes,checks};
  mkdirSync(evidence,{recursive:true});
  writeFileSync('artifacts/ui-smoke.json',`${JSON.stringify(report,null,2)}\n`);
  writeFileSync(`${evidence}/ui-smoke.json`,`${JSON.stringify(report,null,2)}\n`);
}
