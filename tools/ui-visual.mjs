// Isolated browser acceptance for explicit UI fixtures. This cannot pass native gates.
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve, sep } from 'node:path';

const jointLayout = process.argv.includes('--joint-layout');
const evidence = resolve(process.env.ARKDOCK_EVIDENCE_DIR ?? (jointLayout ? 'artifacts/joint-layout-2026-10-09' : 'artifacts/ui-2026-10-09'));
const sizes = [[360, 640], [440, 820], [520, 960]];
const scenarios = ['welcome', 'hint', 'discovering', 'reliable', 'multi', 'weak', 'none', 'trust', 'sdkMissing', 'building', 'waiting', 'running', 'stale', 'recovery', 'diagnostics'];
if (jointLayout) scenarios.push('pairUnconfirmed', 'pairMaximizing', 'pairMaximized', 'pairRestoring', 'pairBlocked');
const checks = []; const screenshots = []; const browserErrors = []; const motion = []; const performance = []; const contrast = [];
const pause = (duration) => new Promise((done) => setTimeout(done, duration));
const suppliedUrl = process.argv.find((arg) => arg.startsWith('--url='))?.slice(6);
const quick = process.argv.includes('--quick');
const url = suppliedUrl ?? 'http://127.0.0.1:1421/ui-demo.html';
const parsedUrl = new URL(url);
if (parsedUrl.hostname !== '127.0.0.1' || parsedUrl.protocol !== 'http:' || parsedUrl.pathname !== '/ui-demo.html') throw new Error('UI fixture acceptance only accepts the explicit loopback ui-demo.html entry.');
let server; let browser; let driver; let profile; let serverTail = ''; let demoBuild = null;
function files(path) { return readdirSync(path, { withFileTypes: true }).flatMap((item) => item.isDirectory() ? files(join(path, item.name)) : [join(path, item.name)]); }
async function waitUntil(predicate, message, timeout = 12_000) {
  const start = Date.now();
  while (Date.now() - start < timeout) { if (await predicate()) return; await pause(40); }
  throw new Error(message);
}
async function connectDebugger(port) {
  const pages = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
  const page = pages.find((item) => item.type === 'page');
  if (!page) throw new Error('Owned headless browser did not create a page');
  const endpoint = new URL(page.webSocketDebuggerUrl);
  if (endpoint.protocol !== 'ws:' || endpoint.hostname !== '127.0.0.1' || Number(endpoint.port) !== port) throw new Error('Debugger endpoint escaped owned loopback browser');
  const socket = new WebSocket(endpoint); const pending = new Map(); let id = 0;
  await new Promise((done, reject) => { socket.onopen = done; socket.onerror = () => reject(new Error('Owned browser debugger handshake failed')); });
  socket.onmessage = (event) => {
    const response = JSON.parse(event.data);
    if (response.method === 'Runtime.exceptionThrown') browserErrors.push(response.params.exceptionDetails.exception?.description ?? response.params.exceptionDetails.text);
    const request = pending.get(response.id);
    if (!request) return;
    clearTimeout(request.timer); pending.delete(response.id);
    if (response.error) request.reject(new Error(response.error.message)); else request.resolve(response.result);
  };
  const send = (method, params = {}) => new Promise((done, reject) => {
    const requestId = ++id;
    const timer = setTimeout(() => { pending.delete(requestId); reject(new Error(`Debugger timeout: ${method}`)); }, method === 'Page.captureScreenshot' ? 45_000 : 12_000);
    pending.set(requestId, { resolve: done, reject, timer }); socket.send(JSON.stringify({ id: requestId, method, params }));
  });
  const evaluate = async (expression) => {
    const result = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
    if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text);
    return result.result.value;
  };
  await send('Page.enable'); await send('Runtime.enable');
  return { send, evaluate, close: () => socket.close() };
}
async function check(name, operation) {
  if (quick && name.startsWith('V1:')) { checks.push({status:'SKIPPED',name,reason:'Explicit --quick diagnostic run; full visual run required before final acceptance'}); return; }
  try { await operation(); checks.push({ status: 'PASS', name }); console.log(`PASS ${name}`); }
  catch (error) {
    let context;
    try { context = await driver.evaluate('({documentFocused:document.hasFocus(),focusTrace:window.__ARKDOCK_FOCUS_TRACE__,active:{tag:document.activeElement?.tagName,text:document.activeElement?.textContent?.slice(0,150),aria:document.activeElement?.getAttribute("aria-label")},motion:document.documentElement.dataset.motion,fixture:window.__ARKDOCK_DEMO__?.current(),calls:window.__ARKDOCK_DEMO__?.calls,body:document.body.textContent.slice(-2500),animations:document.getAnimations().filter(e=>e.playState==="running").map(e=>({name:e.animationName,duration:e.effect.getComputedTiming().duration}))})'); } catch { /* Report the original assertion. */ }
    checks.push({ status: 'FAIL', name, reason: error.message, context }); console.error(`FAIL ${name}: ${error.message}`); process.exitCode = 1;
  }
}
async function load(scenario = 'welcome', theme = 'dark', width = 440, height = 820, scale = 1) {
  // The banner is outside the product surface, so ArkDock retains the requested DIP height.
  const scheme = theme === 'light' || theme === 'minimal-light' ? 'light' : 'dark';
  await driver.send('Emulation.setDeviceMetricsOverride', { width, height: height + 44, deviceScaleFactor: scale, mobile: false });
  await driver.send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-reduced-motion', value: 'no-preference' }, { name: 'prefers-color-scheme', value: scheme }] });
  await driver.send('Page.navigate', { url: `${url}?scenario=${scenario}&theme=${theme}` });
  await waitUntil(() => driver.evaluate('!!window.__ARKDOCK_DEMO__ && !!document.querySelector(".app") && !!document.querySelector("footer").dataset.preview'), 'Demo application state did not initialize');
  if (['running', 'recovery', 'stale', 'diagnostics'].includes(scenario)) await waitUntil(()=>driver.evaluate('document.querySelector(".native-frame")?.dataset.renderStatus==="displayed"'), 'Fixture calibration bitmap was not actually presented');
  if (['stale', 'diagnostics'].includes(scenario)) await waitUntil(()=>driver.evaluate('window.__ARKDOCK_DEMO__.current().phase==="stale"'), 'Build failure must follow actual bitmap presentation');
  await pause(320);
}
async function scenario(value) { await driver.evaluate(`window.__ARKDOCK_DEMO__.setScenario(${JSON.stringify(value)})`); await pause(35); if(['running','recovery'].includes(value))await waitUntil(()=>driver.evaluate('document.querySelector(".native-frame")?.dataset.sessionId===window.__ARKDOCK_DEMO__.current().sessionId&&document.querySelector(".native-frame")?.dataset.renderStatus==="displayed"'),'Current scenario frame did not present'); }
async function click(selector, text) {
  const hiddenToolbar = await driver.evaluate(`(() => {const e=[...document.querySelectorAll(${JSON.stringify(selector)})].find(e=>${text ? `e.textContent.trim() === ${JSON.stringify(text)}` : 'true'});return e?.closest('.chrome-top')?.dataset.open==='false' && !e.classList.contains('preview-chrome-trigger');})()`);
  if(hiddenToolbar){
    await driver.evaluate('document.querySelector(".toolbar-trigger").focus({preventScroll:true})');
    await key('Enter');
    await waitUntil(()=>driver.evaluate('document.querySelector(".chrome-top")?.dataset.open==="true"'),'Preview toolbar did not reveal');await pause(220);
  }
  let point = await driver.evaluate(`(() => { const e = [...document.querySelectorAll(${JSON.stringify(selector)})].find(e => ${text ? `e.textContent.trim() === ${JSON.stringify(text)}` : 'true'}); if (!e || e.disabled) return null; e.scrollIntoView({block:'nearest',inline:'nearest'}); const r=e.getBoundingClientRect(); return {x:r.x+r.width/2,y:r.y+r.height/2}; })()`);
  assert.ok(point, `Clickable element ${selector} ${text ?? ''}`);
  await driver.send('Input.dispatchMouseEvent', { type: 'mouseMoved', ...point });
  await waitUntil(async()=>{
    const settled=await driver.evaluate(`(() => {const e=[...document.querySelectorAll(${JSON.stringify(selector)})].find(e=>${text ? `e.textContent.trim() === ${JSON.stringify(text)}` : 'true'});if(!e||e.disabled||e.closest('[inert]'))return null;const r=e.getBoundingClientRect(),x=r.x+r.width/2,y=r.y+r.height/2,hit=document.elementFromPoint(x,y);return r.width>0&&r.height>0&&(hit===e||e.contains(hit))?{x,y}:null;})()`);
    if(settled)point=settled;return Boolean(settled);
  },`Element did not become hit-testable: ${selector} ${text??''}`);
  await driver.send('Input.dispatchMouseEvent', { type: 'mousePressed', ...point, button: 'left', clickCount: 1 });
  await driver.send('Input.dispatchMouseEvent', { type: 'mouseReleased', ...point, button: 'left', clickCount: 1 });
}
async function menuItem(text) {
  if(await driver.evaluate('!!document.querySelector("dialog")')) {await key('Escape');await waitUntil(()=>driver.evaluate('!document.querySelector("dialog")'),'Previous sheet did not close');}
  await click('[aria-label="更多操作"]');
  await waitUntil(()=>driver.evaluate('document.querySelector("dialog")?.getAttribute("aria-label")==="更多操作"'),'Menu did not open');
  await click('dialog button',text);
}
async function cancelDiscoveryAssociation() {
  await click('[aria-label="展开工程信息"]');
  await waitUntil(()=>driver.evaluate('document.querySelector(".workspace-card")?.dataset.expanded==="true"'),'Discovery actions did not expand');
  await pause(250);
  await click('.workspace-detail-actions button','取消关联');
}
async function key(value, options = {}) {
  const virtualCode = { Tab: 9, Escape: 27, Enter: 13, ArrowDown: 40 }[value];
  const code = virtualCode ? { windowsVirtualKeyCode: virtualCode, nativeVirtualKeyCode: virtualCode } : {};
  await driver.send('Input.dispatchKeyEvent', { type: 'keyDown', key: value, code: value, ...code, ...(value === 'Enter' ? { text: '\r', unmodifiedText: '\r' } : {}), ...options });
  await driver.send('Input.dispatchKeyEvent', { type: 'keyUp', key: value, code: value, ...code, ...options });
}
async function state() { return driver.evaluate('({binding:document.querySelector(".app").dataset.binding,workspace:document.querySelector(".app").dataset.workspace,pairLayout:document.querySelector(".app").dataset.pairLayout,preview:document.querySelector("footer").dataset.preview,fixture:window.__ARKDOCK_DEMO__.current()})'); }
async function screenshot(name) {
  const image = await driver.send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false });
  const path = join(evidence, `${name}.png`); writeFileSync(path, Buffer.from(image.data, 'base64')); screenshots.push(path);
}
function luminance(color) {
  const hex = color.startsWith('#') ? color.slice(1) : null;
  const expanded = hex?.length === 3 ? [...hex].map(item=>item+item).join('') : hex;
  const components = expanded ? expanded.match(/.{2}/g).map(item=>parseInt(item,16)) : color.match(/[\d.]+/g).slice(0,3).map(Number);
  return components.map(value=>{const c=value/255;return c<=.04045?c/12.92:((c+.055)/1.055)**2.4;}).reduce((sum,value,index)=>sum+value*[.2126,.7152,.0722][index],0);
}
function contrastRatio(foreground,background) { const values=[luminance(foreground),luminance(background)].sort((a,b)=>b-a);return (values[0]+.05)/(values[1]+.05); }
try {
  mkdirSync(evidence, { recursive: true });
  if (!suppliedUrl) {
    // Test a fixed explicit demo build. Live development HMR must not reset in-flight race fixtures.
    const built = spawnSync(process.execPath, [resolve('node_modules/vite/bin/vite.js'), 'build', '--config', 'vite.demo.config.ts'], { cwd: process.cwd(), windowsHide: true, encoding: 'utf8', timeout: 30_000 });
    serverTail = `${built.stdout ?? ''}${built.stderr ?? ''}`.slice(-6000);
    if (built.status !== 0) throw new Error(`Isolated demo build failed: ${serverTail}`);
    demoBuild = { capturedAt: new Date().toISOString(), directory: resolve('artifacts/ui-demo-dist'), assets: files('artifacts/ui-demo-dist').map(path=>({path,sha256:createHash('sha256').update(readFileSync(path)).digest('hex')})) };
    server = spawn(process.execPath, [resolve('node_modules/vite/bin/vite.js'), 'preview', '--config', 'vite.demo.config.ts', '--host', '127.0.0.1', '--port', '1421', '--strictPort'], { cwd: process.cwd(), windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] });
    const remember = (chunk) => { serverTail = (serverTail + chunk).slice(-6000); };
    server.stdout.on('data', remember); server.stderr.on('data', remember);
    await waitUntil(async () => { try { return (await fetch(url, { signal: AbortSignal.timeout(1000) })).ok; } catch { return false; } }, 'Isolated Vite demo server did not start');
  }
  const choices = [process.env.ARKDOCK_UI_BROWSER, 'C:/Program Files/Google/Chrome/Application/chrome.exe', 'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe'].filter(Boolean);
  const executable = choices.find(existsSync);
  if (!executable) throw new Error('No browser executable found; set ARKDOCK_UI_BROWSER to an installed Chromium browser.');
  profile = mkdtempSync(join(tmpdir(), 'arkdock-ui-owned-'));
  browser = spawn(executable, ['--headless=new', '--remote-debugging-address=127.0.0.1', '--remote-debugging-port=0', `--user-data-dir=${profile}`, '--no-first-run', '--no-default-browser-check', '--disable-extensions', 'about:blank'], { windowsHide: true, stdio: 'ignore' });
  let port;
  await waitUntil(() => { try { port = Number(readFileSync(join(profile, 'DevToolsActivePort'), 'utf8').split('\n')[0]); return Number.isInteger(port) && port > 0; } catch { return false; } }, 'Owned headless browser did not open debugger');
  driver = await connectDebugger(port);
  if (jointLayout) {
    await check('Joint UI: unbound/unconfirmed/blocked disable pair controls and disclose reasons without confirmed badges', async () => {
      await load('welcome');assert.equal(await driver.evaluate('document.querySelector(".header [aria-label=共同最大化]").disabled'),true);
      assert.equal(await driver.evaluate('!!document.querySelector(".pair-badge")'),false);
      await load('pairUnconfirmed');assert.equal((await state()).pairLayout,'unbound');
      assert.equal(await driver.evaluate('document.querySelector(".header [aria-label=共同最大化]").disabled'),true);
      assert.equal(await driver.evaluate('document.querySelector(".pair-badge").textContent'),'等待布局');
      await load('pairBlocked');assert.equal((await state()).pairLayout,'blocked');
      const blocked=await driver.evaluate('(()=>{const e=document.querySelector(".header [aria-label=共同最大化]");return {disabled:e.disabled,reason:e.title,badge:document.querySelector(".pair-badge").textContent};})()');
      assert.equal(blocked.disabled,true);assert.ok(blocked.reason.includes('权限高于'));assert.equal(blocked.badge,'布局受限');
    });
    await check('Joint UI: confirmed maximize/restore remain available independently of SDK and project trust', async () => {
      for(const initial of ['sdkMissing','trust']) {
        await load(initial);if(initial==='trust'){await key('Escape');await waitUntil(()=>driver.evaluate('!document.querySelector("dialog")'),'Trust panel did not close');}
        const before=(await state()).fixture;assert.equal(before.backend.available,false);
        assert.equal(await driver.evaluate('document.querySelector(".header [aria-label=共同最大化]").disabled'),false);
        await click('.header [aria-label="共同最大化"]');await waitUntil(async()=>(await state()).pairLayout==='maximized','Maximize confirmation not received');
        assert.equal(await driver.evaluate('document.querySelector(".pair-badge").textContent'),'共同最大化');
        await click('.header [aria-label="恢复共同布局"]');await waitUntil(async()=>(await state()).pairLayout==='paired','Restore confirmation not received');
        assert.equal(await driver.evaluate('document.querySelector(".pair-badge").textContent'),'全贴合');
        const after=(await state()).fixture;assert.equal(after.trusted,before.trusted);assert.equal(after.phase,before.phase);assert.equal(after.backend.available,false);
        const calls=await driver.evaluate('window.__ARKDOCK_DEMO__.calls');assert.equal(calls.filter(e=>e.command==='maximize_pair').length,1);assert.equal(calls.filter(e=>e.command==='restore_pair').length,1);
        assert.equal(calls.some(e=>e.command==='set_trust'||e.command==='refresh_preview'),false);
        assert.deepEqual(calls.find(e=>e.command==='maximize_pair').scope,{targetId:before.target.id,sessionId:before.sessionId});
      }
    });
    await check('Joint UI: explicit pending events never claim completion and rapid repeated clicks issue one request', async () => {
      await load('sdkMissing');
      for(const [command,initial,transient,confirmed,label] of [['maximize_pair','paired','maximizing','maximized','共同最大化'],['restore_pair','maximized','restoring','paired','恢复共同布局']]) {
        assert.equal((await state()).pairLayout,initial);await driver.evaluate(`window.__ARKDOCK_DEMO__.deferNext(${JSON.stringify(command)},"command")`);
        const point=await driver.evaluate(`(()=>{const r=document.querySelector('.header [aria-label=${JSON.stringify(label)}]').getBoundingClientRect();return{x:r.x+r.width/2,y:r.y+r.height/2};})()`);
        await click(`.header [aria-label="${label}"]`);await waitUntil(()=>driver.evaluate('window.__ARKDOCK_DEMO__.hasDeferred()'),'Pair request did not defer');
        assert.equal((await state()).pairLayout,initial,'Request start predicted a confirmed pair state');
        await driver.evaluate(`window.__ARKDOCK_DEMO__.emitPairLayout(${JSON.stringify(transient)})`);
        await waitUntil(async()=>(await state()).pairLayout===transient,'Transient pair event not rendered');
        assert.equal(await driver.evaluate('document.querySelector(".header .maximize").disabled'),true);
        assert.equal(await driver.evaluate('document.querySelector(".pair-badge").textContent'),'布局调整中');
        for(let repeat=0;repeat<3;repeat++){await driver.send('Input.dispatchMouseEvent',{type:'mousePressed',...point,button:'left',clickCount:1});await driver.send('Input.dispatchMouseEvent',{type:'mouseReleased',...point,button:'left',clickCount:1});}
        assert.equal(await driver.evaluate(`window.__ARKDOCK_DEMO__.calls.filter(e=>e.command===${JSON.stringify(command)}).length`),1);
        await driver.evaluate('window.__ARKDOCK_DEMO__.releaseDeferred()');await waitUntil(async()=>(await state()).pairLayout===confirmed,'Pair completion not rendered');
        await driver.evaluate('window.__ARKDOCK_DEMO__.emitOldState()');await pause(40);assert.equal((await state()).pairLayout,confirmed,'Stale pair event overwrote completion');
      }
    });
    await check('Joint UI: stale maximize response cannot overwrite a newly connected target layout', async () => {
      await load('sdkMissing');await driver.evaluate('window.__ARKDOCK_DEMO__.deferNext("maximize_pair","response")');
      await click('.header [aria-label="共同最大化"]');await waitUntil(()=>driver.evaluate('window.__ARKDOCK_DEMO__.hasDeferred()'),'Maximize response did not defer');
      await driver.evaluate('window.__ARKDOCK_DEMO__.switchTarget();window.__ARKDOCK_DEMO__.emitPairLayout("paired")');await pause(40);
      const current=(await state()).fixture;await driver.evaluate('window.__ARKDOCK_DEMO__.releaseDeferred()');await pause(80);
      assert.equal((await state()).fixture.target.id,current.target.id);assert.equal((await state()).pairLayout,'paired');
      assert.equal(await driver.evaluate('document.querySelector(".pair-badge").textContent'),'全贴合');
      assert.equal(await driver.evaluate('document.querySelector(".header [aria-label=共同最大化]").disabled'),false);
    });
    await check('Joint UI: overflow maximize and restore are keyboard operable in the 360 DIP window', async () => {
      await load('sdkMissing','dark',360,640);
      for(const label of ['共同最大化','恢复共同布局']) {
        await click('[aria-label="更多操作"]');await waitUntil(()=>driver.evaluate('!!document.querySelector("dialog")'),'Overflow did not open');
        for(let count=0;count<20;count++){if(await driver.evaluate(`document.activeElement?.textContent.trim()===${JSON.stringify(label)}`))break;await key('Tab');}
        assert.equal(await driver.evaluate('document.activeElement?.textContent.trim()'),label);await key('Enter');
        await waitUntil(()=>driver.evaluate('!document.querySelector("dialog")'),'Confirmed overflow action did not close its originating sheet');
      }
      assert.equal((await state()).pairLayout,'paired');
    });
  }
  await check('V1: all explicit scenarios at 360×640 / 440×820 / 520×960 in dark and light; no outer scroll', async () => {
    for (const [width, height] of sizes) for (const theme of ['dark', 'light']) for (const name of scenarios) {
      await load(name, theme, width, height);
      if (name === 'diagnostics') { await menuItem('运行日志'); await pause(280); }
      const bounds = await driver.evaluate('({width:innerWidth,height:innerHeight,scrollWidth:document.documentElement.scrollWidth,scrollHeight:document.documentElement.scrollHeight,banner:document.querySelector(".ui-demo-banner").textContent,app:document.querySelector(".app").getBoundingClientRect().toJSON(),footer:document.querySelector("footer").getBoundingClientRect().toJSON()})');
      assert.ok(bounds.banner.includes('UI 演示 · 非原生渲染'));
      assert.ok(bounds.scrollWidth <= width, `${name}/${theme}/${width}: horizontal overflow`);
      assert.ok(bounds.scrollHeight <= height + 44, `${name}/${theme}/${height}: outer vertical overflow`);
      assert.ok(Math.abs(bounds.app.height - height) <= 1, `${name}/${theme}/${height}: product surface has incorrect DIP height`);
      assert.ok(bounds.footer.bottom <= height + 45, `${name}/${theme}/${height}: footer cropped`);
      await screenshot(`${name}-${theme}-${width}x${height}`);
    }
  });
  await check('V1: 125% / 150% / 200% browser scale retain logical fit and proportional frame', async () => {
    for (const scale of [1.25, 1.5, 2]) {
      await load('running', 'dark', 440, 820, scale);
      const frame = await driver.evaluate('(() => {const e=document.querySelector(".native-frame"),r=e.getBoundingClientRect();return {width:r.width,height:r.height,scroll:document.documentElement.scrollWidth};})()');
      assert.ok(frame.width > 0 && frame.height > 0); assert.ok(Math.abs(frame.width / frame.height - 360 / 640) < .01); assert.ok(frame.scroll <= 440);
      await screenshot(`running-dark-440x820-dpr-${scale}`);
    }
  });
  await check('V4: core text, secondary text and primary action exceed 4.5:1 contrast in all shell themes', async () => {
    for (const theme of ['dark', 'light', 'minimal-light', 'titanium-dark']) {
      await load('trust', theme);
      const colors = await driver.evaluate('(()=>{const root=getComputedStyle(document.documentElement),button=getComputedStyle(document.querySelector(".primary-button"));return {text:root.getPropertyValue("--text").trim(),muted:root.getPropertyValue("--muted").trim(),background:root.getPropertyValue("--bg").trim(),panel:root.getPropertyValue("--panel").trim(),buttonText:button.color,buttonBackground:button.backgroundColor};})()');
      const ratios={textOnBackground:contrastRatio(colors.text,colors.background),mutedOnPanel:contrastRatio(colors.muted,colors.panel),primaryAction:contrastRatio(colors.buttonText,colors.buttonBackground)};
      contrast.push({theme,colors,ratios});assert.ok(Object.values(ratios).every(value=>value>=4.5),`${theme}: ${JSON.stringify(ratios)}`);
    }
  });
  await check('V1: Minimal Light and Titanium Dark themes fit 360×640 / 440×820 / 520×960, persist, fallback safely, and leave native frame unfiltered', async () => {
    for (const theme of ['minimal-light', 'titanium-dark']) {
      for (const [width, height] of sizes) {
        for (const name of ['welcome', 'running', 'multi', 'trust']) {
          await load(name, theme, width, height);
          const bounds = await driver.evaluate('({width:innerWidth,height:innerHeight,scrollWidth:document.documentElement.scrollWidth,scrollHeight:document.documentElement.scrollHeight,theme:document.documentElement.dataset.theme,app:document.querySelector(".app").getBoundingClientRect().toJSON(),footer:document.querySelector("footer").getBoundingClientRect().toJSON()})');
          assert.equal(bounds.theme, theme);
          assert.ok(bounds.scrollWidth <= width, `${name}/${theme}/${width}: horizontal overflow`);
          assert.ok(bounds.scrollHeight <= height + 44, `${name}/${theme}/${height}: outer vertical overflow`);
          assert.ok(bounds.footer.bottom <= height + 45, `${name}/${theme}/${height}: footer cropped`);
          if (width === 440 && height === 820) await screenshot(`${name}-${theme}-${width}x${height}`);
        }
      }
    }
    await load('running', 'dark', 440, 820);
    const beforeSession = (await state()).fixture.sessionId;
    await click('[aria-label="更多操作"]'); await click('button', '偏好设置'); await pause(40);
    for (const [label, id, scheme] of [['极简浅色', 'minimal-light', 'light'], ['深色钛金', 'titanium-dark', 'dark'], ['浅色', 'light', 'light'], ['深色', 'dark', 'dark'], ['跟随系统', 'system', 'dark']]) {
      await click('button', label); await pause(30);
      const current = await driver.evaluate('({theme:document.documentElement.dataset.theme,stored:localStorage.getItem("arkdock:theme"),scheme:getComputedStyle(document.documentElement).colorScheme,pressed:document.querySelector(`[data-theme-id="${document.documentElement.dataset.theme}"] .theme-card-button`)?.getAttribute("aria-pressed"),frameFilter:getComputedStyle(document.querySelector(".native-frame")).filter,surfaceFilter:getComputedStyle(document.querySelector(".native-surface")).filter,displayed:document.querySelector(".native-frame")?.dataset.renderStatus,dialogScrollWidth:document.querySelector("dialog")?.scrollWidth,dialogClientWidth:document.querySelector("dialog")?.clientWidth})');
      assert.equal(current.theme, id);
      assert.equal(current.stored, id);
      assert.equal(current.scheme, scheme);
      assert.equal(current.pressed, 'true');
      assert.equal(current.frameFilter, 'none');
      assert.equal(current.surfaceFilter, 'none');
      assert.equal(current.displayed, 'displayed');
      assert.ok(current.dialogScrollWidth <= current.dialogClientWidth + 1, `${id}: settings dialog horizontal overflow`);
      if (id === 'minimal-light' || id === 'titanium-dark') await screenshot(`settings-${id}-440x820`);
    }
    assert.equal((await state()).fixture.sessionId, beforeSession);
    assert.equal(await driver.evaluate('window.__ARKDOCK_DEMO__.calls.some(e=>["refresh_preview","rediscover","detach"].includes(e.command))'), false);
    await driver.evaluate('localStorage.setItem("arkdock:theme", "unsupported-legacy-theme")');
    await driver.send('Page.reload'); await pause(200);
    assert.equal(await driver.evaluate('document.documentElement.dataset.theme'), 'dark');
  });
  await check('V1: 悬浮球透明度拖动即时预览、松开写入一次、键盘同样合并并持久化', async () => {
    await load('running', 'dark', 440, 820);
    await click('[aria-label="更多操作"]'); await click('button', '偏好设置'); await pause(40);
    const slider = 'input[aria-label="悬浮球透明度"]';
    await driver.evaluate(`document.querySelector(${JSON.stringify(slider)}).scrollIntoView({block:'center'})`); await pause(60);
    // 连接后的启动同步会先写入一次偏好；以它为基线衡量拖动与键盘各自只追加一次。
    await waitUntil(async () => (await driver.evaluate('window.__ARKDOCK_DEMO__.calls.filter(c=>c.command==="set_orb_opacity").length')) >= 1, '启动偏好透明度未写入');
    const baseline = await driver.evaluate('window.__ARKDOCK_DEMO__.calls.filter(c=>c.command==="set_orb_opacity").length');
    const initial = await driver.evaluate(`(()=>{const e=document.querySelector(${JSON.stringify(slider)});return {value:e.value,min:e.min,max:e.max,step:e.step,disabled:e.disabled,specimen:document.querySelector(".orb-specimen").style.opacity};})()`);
    assert.deepEqual({ value: initial.value, min: initial.min, max: initial.max, step: initial.step, disabled: initial.disabled }, { value: '0', min: '0', max: '80', step: '5', disabled: false });
    assert.equal(Number(initial.specimen), 1, '默认不透明时示例球不应用额外透明度');
    const track = await driver.evaluate(`(()=>{const r=document.querySelector(${JSON.stringify(slider)}).getBoundingClientRect();return {left:r.left,top:r.top+r.height/2,width:r.width};})()`);
    const at = (fraction) => ({ x: track.left + track.width * fraction, y: track.top });
    await driver.send('Input.dispatchMouseEvent', { type: 'mousePressed', ...at(.75), button: 'left', clickCount: 1 });
    await pause(60);
    const preview = await driver.evaluate(`(()=>{const e=document.querySelector(${JSON.stringify(slider)});return {value:e.value,valuetext:e.getAttribute("aria-valuetext"),specimen:document.querySelector(".orb-specimen").style.opacity,output:document.querySelector(".setting-slider-value").textContent,calls:window.__ARKDOCK_DEMO__.calls.filter(c=>c.command==="set_orb_opacity").length};})()`);
    assert.equal(preview.calls, baseline, '拖动过程中不得发起原生写入');
    assert.ok(Number(preview.value) > 0, '按压滑轨后透明度应即时变化');
    assert.equal(preview.output, `${preview.value}%`);
    assert.equal(preview.valuetext, `${preview.value}% 透明`);
    assert.equal(Number(preview.specimen), (100 - Number(preview.value)) / 100, '示例球透明度跟随拖动值');
    await driver.send('Input.dispatchMouseEvent', { type: 'mouseReleased', ...at(.75), button: 'left', clickCount: 1 });
    await waitUntil(async () => (await driver.evaluate('window.__ARKDOCK_DEMO__.calls.filter(c=>c.command==="set_orb_opacity").length')) === baseline + 1, '松开后透明度未写入原生一次');
    const committed = await driver.evaluate('window.__ARKDOCK_DEMO__.calls.filter(c=>c.command==="set_orb_opacity").at(-1)');
    assert.equal(committed.args.opacity, 100 - Number(preview.value));
    const current = (await state()).fixture;
    assert.deepEqual(committed.scope, { targetId: current.target.id, sessionId: current.sessionId });
    assert.equal(current.display.orbOpacity, committed.args.opacity, '原生确认值回到快照');
    assert.equal(await driver.evaluate('localStorage.getItem("arkdock:orb-opacity")'), String(committed.args.opacity));
    await driver.evaluate(`document.querySelector(${JSON.stringify(slider)}).focus()`);
    await key('ArrowDown');
    await waitUntil(async () => (await driver.evaluate('window.__ARKDOCK_DEMO__.calls.filter(c=>c.command==="set_orb_opacity").length')) === baseline + 2, '键盘调整后透明度未合并为一次写入');
    // ArrowDown 降低透明度 = 提高不透明度，同样只在 keyup 写入一次。
    const stepped = await driver.evaluate('window.__ARKDOCK_DEMO__.calls.filter(c=>c.command==="set_orb_opacity").at(-1)');
    assert.equal(stepped.args.opacity, committed.args.opacity + 5);
    await screenshot('settings-orb-opacity-440x820');
  });
  await check('V2: reliable candidate bypasses selection; trust gates execution', async () => {
    await load('reliable'); assert.equal((await state()).workspace, 'untrusted');
    assert.equal(await driver.evaluate('!!document.querySelector(".candidate-list")'), false);
    assert.equal(await driver.evaluate('window.__ARKDOCK_DEMO__.calls.some(e=>e.command==="refresh_preview"||e.command==="set_trust")'), false);
  });
  await check('V2: weak candidate needs explicit confirmation; all six multi candidates searchable and keyboard-selectable', async () => {
    await load('weak'); assert.equal((await state()).workspace, 'selecting'); assert.equal(await driver.evaluate('document.querySelectorAll(".candidate").length'), 1);
    assert.equal((await state()).fixture.project, null);
    await load('multi'); assert.equal(await driver.evaluate('document.querySelectorAll(".candidate").length'), 6);
    await click('input[aria-label="筛选已列出的工程候选"]'); await driver.send('Input.insertText', { text: 'quick-action' }); await pause(40);
    assert.equal(await driver.evaluate('document.querySelectorAll(".candidate").length'), 1);
    await key('Tab'); await key('Tab');
    // Focus the matching native button through the actual tab sequence before Enter.
    for (let index = 0; index < 8; index++) {
      if (await driver.evaluate('document.activeElement?.closest(".candidate") !== null')) break;
      await key('Tab');
    }
    assert.equal(await driver.evaluate('!!document.activeElement?.closest(".candidate")'), true);
    await key('Enter'); await pause(80); assert.equal((await state()).fixture.project.name, 'quick-action'); assert.equal((await state()).fixture.trusted, false);
  });
  await check('V2: zero candidates has directory fallback; unsupported directory action reports actual reason', async () => {
    await load('none'); await click('button', '手动选择工程目录'); await pause(50);
    assert.ok((await driver.evaluate('document.body.textContent')).includes('UI 演示不打开系统目录对话框'));
  });
  await check('V2: obsolete discovery snapshot cannot replace newer target and cancellation stays cancelled', async () => {
    await load('multi'); await driver.evaluate('window.__ARKDOCK_DEMO__.switchTarget()'); await pause(40);
    const active = (await state()).fixture.target.id;
    await driver.evaluate('window.__ARKDOCK_DEMO__.emitOldState()'); await pause(40);
    assert.equal((await state()).fixture.target.id, active); assert.equal((await state()).binding, 'bound'); assert.equal((await state()).workspace, 'discovering');
    await menuItem('解除当前绑定'); await pause(80);
    assert.equal(await driver.evaluate('window.__ARKDOCK_DEMO__.calls.some(e=>e.command==="detach")'), true);
    await driver.evaluate('window.__ARKDOCK_DEMO__.emitOldState()'); await pause(40);
    assert.notEqual((await state()).workspace, 'discovering');
  });
  await check('V2: cancellation survives newly identified session and never detaches a newer target', async () => {
    await load('discovering'); await driver.evaluate('window.__ARKDOCK_DEMO__.deferNext("detach","command")');
    await cancelDiscoveryAssociation(); await waitUntil(()=>driver.evaluate('window.__ARKDOCK_DEMO__.hasDeferred()'),'Deferred cancellation was not requested');
    await scenario('reliable'); await driver.evaluate('window.__ARKDOCK_DEMO__.releaseDeferred()');
    await waitUntil(async()=>!(await state()).fixture.target,'Cancel did not invalidate the newly discovered session on the same target');
    assert.equal(await driver.evaluate('window.__ARKDOCK_DEMO__.calls.filter(e=>e.command==="detach").length'),2);
    await load('discovering'); await driver.evaluate('window.__ARKDOCK_DEMO__.deferNext("detach","command")');
    await cancelDiscoveryAssociation(); await waitUntil(()=>driver.evaluate('window.__ARKDOCK_DEMO__.hasDeferred()'),'Second deferred cancellation was not requested');
    await driver.evaluate('window.__ARKDOCK_DEMO__.switchTarget()'); await pause(40); const targetId=(await state()).fixture.target.id;
    await driver.evaluate('window.__ARKDOCK_DEMO__.releaseDeferred()'); await pause(80);
    assert.equal((await state()).fixture.target.id,targetId);assert.equal((await state()).workspace,'discovering');
    assert.equal(await driver.evaluate('window.__ARKDOCK_DEMO__.calls.filter(e=>e.command==="detach").length'),1);
  });
  await check('V3: delayed trust response cannot close or overwrite a newer session trust panel', async () => {
    await load('trust'); await driver.evaluate('window.__ARKDOCK_DEMO__.deferNext("set_trust","response")');
    await click('button','信任此工程'); await waitUntil(()=>driver.evaluate('window.__ARKDOCK_DEMO__.hasDeferred()'),'Trust response did not defer');
    await scenario('trust'); const sessionId=(await state()).fixture.sessionId;
    await driver.evaluate('window.__ARKDOCK_DEMO__.releaseDeferred()'); await pause(80);
    assert.equal((await state()).fixture.sessionId,sessionId);assert.equal((await state()).workspace,'untrusted');
    assert.equal(await driver.evaluate('document.querySelector("dialog")?.getAttribute("aria-label")'),'工程信任');
  });
  await check('V3: build completion without current frame waits; old session frame rejected', async () => {
    await load('waiting'); assert.equal((await state()).preview, 'waitingFrame'); assert.equal(await driver.evaluate('!!document.querySelector(".native-frame[data-render-status=displayed]")'), false);
    await scenario('running'); assert.equal((await state()).preview, 'running');
    await driver.evaluate('window.__ARKDOCK_DEMO__.switchTarget();window.__ARKDOCK_DEMO__.emitOldFrame()'); await pause(50);
    assert.equal(await driver.evaluate('!!document.querySelector(".native-frame[data-render-status=displayed]")'), false);
  });
  await check('V3: failed build retains clearly stale image, disables input, and recovery accepts current frame', async () => {
    await load('stale'); assert.equal((await state()).preview, 'stale'); assert.equal(await driver.evaluate('!!document.querySelector(".native-frame[data-render-status=displayed]")'), true);
    assert.equal(await driver.evaluate('document.querySelector(".native-surface").disabled'), true);
    assert.ok((await driver.evaluate('document.body.textContent')).includes('旧画面'));
    await scenario('recovery'); assert.equal((await state()).preview, 'running'); assert.equal(await driver.evaluate('document.querySelector(".native-surface").disabled'), false);
  });
  await check('V3: image edge maps click coordinates; refresh/stop are explicit fixture requests', async () => {
    await load('running'); await click('.native-surface'); await pause(40);
    const call = await driver.evaluate('window.__ARKDOCK_DEMO__.calls.find(e=>e.command==="click_preview")');
    assert.ok(call); assert.ok(Math.abs(call.args.x - 180) <= 1 && Math.abs(call.args.y - 320) <= 1);
    await click('[aria-label="手动刷新"]'); await pause(40); assert.equal((await state()).preview, 'building');
    await click('[aria-label="停止原生预览"]'); await pause(40); assert.equal((await state()).preview, 'stopped');
  });
  await check('V3: missing independent orientation and theme control states remain unavailable with reasons', async () => {
    await load('running'); await click('[aria-label="预览能力"]'); await pause(60);
    const controls = await driver.evaluate('[...document.querySelectorAll(".capability-list>div")].map(e=>({text:e.textContent,status:e.querySelector(".badge")?.textContent,reason:e.querySelector("p")?.textContent}))');
    assert.ok(controls.some(e => /方向|旋转/.test(e.text) && e.status === '未接入' && e.reason));
    assert.ok(controls.some(e => /应用.*主题/.test(e.text) && e.status === '未接入' && e.reason));
  });
  await check('V4: panel receives focus, traps Tab, Esc closes, and restores trigger focus', async () => {
    await load('running'); await click('[aria-label="更多操作"]'); await click('button', '偏好设置'); await pause(40);
    assert.equal(await driver.evaluate('!!document.activeElement?.closest("dialog,[role=dialog]")'), true);
    for (let i = 0; i < 22; i++) await key('Tab');
    assert.equal(await driver.evaluate('!!document.activeElement?.closest("dialog,[role=dialog]")'), true);
    await key('Escape'); await pause(40);
    assert.equal(await driver.evaluate('!!document.querySelector("dialog,[role=dialog]")'), false);
    assert.ok(await driver.evaluate('document.activeElement?.getAttribute("aria-label")==="更多操作" || document.activeElement?.textContent.includes("偏好设置")'));
  });
  await check('V4: delayed action completion preserves diagnostics opened after the request', async () => {
    await load('running');
    await driver.evaluate('(()=>{const trace=[];window.__ARKDOCK_FOCUS_TRACE__=trace;const note=(event,target)=>{trace.push({event,time:performance.now(),focused:document.hasFocus(),active:document.activeElement?.tagName,activeLabel:document.activeElement?.getAttribute("aria-label"),target:target?.tagName,targetLabel:target?.getAttribute("aria-label"),targetText:target?.textContent?.slice(0,50)});if(trace.length>60)trace.shift()};const original=HTMLElement.prototype.focus;HTMLElement.prototype.focus=function(...args){note("focus called",this);original.apply(this,args);note("focus returned",this)};for(const event of ["focusin","focusout"])document.addEventListener(event,e=>note(event,e.target));new MutationObserver(records=>{note("disabled/child mutation "+records.length);requestAnimationFrame(()=>note("RAF after mutation"))}).observe(document.querySelector(".app"),{subtree:true,attributes:true,attributeFilter:["disabled"],childList:true});})()');
    await click('[aria-label="预览能力"]');await driver.evaluate('window.__ARKDOCK_DEMO__.deferNext("refresh_preview","response")');
    await click('button','手动刷新');await waitUntil(()=>driver.evaluate('window.__ARKDOCK_DEMO__.hasDeferred()'),'Refresh response did not defer');
    await waitUntil(()=>driver.evaluate('[...document.querySelectorAll("dialog button")].some(e=>e.textContent.trim()==="手动刷新" && e.disabled)'),'Pending action button did not enter confirmed disabled state');
    await waitUntil(()=>driver.evaluate('!!document.activeElement?.closest("dialog")'),'Disabling the focused action lost modal focus');
    await key('Escape');await waitUntil(()=>driver.evaluate('!document.querySelector("dialog")'),'Controls sheet did not close before opening diagnostics');
    await menuItem('运行日志');await waitUntil(()=>driver.evaluate('document.querySelector("dialog")?.getAttribute("aria-label")==="运行日志"'),'Diagnostics did not open before releasing the delayed response');
    assert.equal(await driver.evaluate('document.querySelector("dialog")?.getAttribute("aria-label")'),'运行日志');
    await driver.evaluate('window.__ARKDOCK_DEMO__.releaseDeferred()');await pause(80);
    assert.equal(await driver.evaluate('document.querySelector("dialog")?.getAttribute("aria-label")'),'运行日志');
  });
  await check('V4: error toast cannot steal modal focus and becomes dismissible after the sheet closes', async () => {
    await load('none');await click('button','手动选择工程目录');await pause(50);
    assert.equal(await driver.evaluate('document.querySelector("[aria-label=关闭提示]")?.disabled'),true);
    for(let index=0;index<14;index++)await key('Tab');
    assert.equal(await driver.evaluate('!!document.activeElement?.closest("dialog")'),true);
    await key('Escape');await pause(40);await click('[aria-label="关闭提示"]');await pause(30);
    assert.equal(await driver.evaluate('!!document.querySelector(".toast")'),false);
  });
  await check('V4: real CSS motion starts and rapid events replace prior transitions; reduced motion eliminates loops/displacement', async () => {
    await load('running'); await menuItem('运行日志');
    const opening = await driver.evaluate('[...document.getAnimations()].map(e=>({name:e.animationName,playState:e.playState,timing:e.effect.getComputedTiming(),transform:getComputedStyle(e.effect.target).transform}))');
    motion.push({ event: 'open diagnostics', elapsed: 0, animations: opening });
    assert.ok(opening.some(e => e.playState === 'running' && Number(e.timing.duration) > 1), 'No running panel animation observed');
    await pause(90); motion.push({ event: 'open diagnostics', elapsed: 90, animations: await driver.evaluate('[...document.getAnimations()].map(e=>({name:e.animationName,playState:e.playState,timing:e.effect.getComputedTiming(),transform:getComputedStyle(e.effect.target).transform}))') });
    await key('Escape'); await scenario('discovering'); await scenario('running'); await scenario('discovering'); await scenario('running'); await pause(350);
    assert.equal((await state()).preview, 'running');
    assert.equal(await driver.evaluate('document.getAnimations().filter(e=>e.playState==="running" && e.effect.getComputedTiming().iterations!==Infinity).length'), 0);
    await driver.send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-reduced-motion', value: 'reduce' }] });
    await menuItem('运行日志'); await pause(40);
    const reduced = await driver.evaluate('[...document.getAnimations()].filter(e=>e.playState==="running").map(e=>({duration:e.effect.getComputedTiming().duration,iterations:e.effect.getComputedTiming().iterations,transform:getComputedStyle(e.effect.target).transform}))');
    motion.push({ event: 'system reduced motion', animations: reduced });
    assert.ok(reduced.every(e => Number(e.duration) <= 1 && e.iterations !== Infinity));
  });
  await check('V4: Chinese composition is confined to search input and does not trigger window dragging', async () => {
    await load('multi'); await click('input[aria-label="筛选已列出的工程候选"]');
    await driver.send('Input.imeSetComposition', { text: '中文', selectionStart: 2, selectionEnd: 2 });
    await driver.send('Input.insertText', { text: '中文' }); await pause(50);
    assert.ok((await driver.evaluate('document.querySelector("input[aria-label=筛选已列出的工程候选]").value')).includes('中文'));
    assert.equal(await driver.evaluate('window.__ARKDOCK_DEMO__.calls.some(e=>e.command==="begin_drag")'), false);
  });
  await check('V4: explicit reduced-motion preference cancels panel movement independently of system media', async () => {
    await load('running'); await click('[aria-label="更多操作"]'); await click('button', '偏好设置');
    await click('button', '减少动画'); await key('Escape'); await pause(40);
    await menuItem('运行日志'); await pause(40);
    const active = await driver.evaluate('[...document.getAnimations()].filter(e=>e.playState==="running").map(e=>({duration:e.effect.getComputedTiming().duration,iterations:e.effect.getComputedTiming().iterations}))');
    assert.ok(active.every(e=>Number(e.duration)<=1 && e.iterations!==Infinity));
    motion.push({event:'explicit reduced-motion setting with system no-preference',animations:active});
  });
  await check('Performance: 200 frame events leave header stable; capture actual layout/style/heap counters', async () => {
    await load('running'); await driver.send('Performance.enable');
    const before = await driver.send('Performance.getMetrics');
    const mutations = await driver.evaluate('new Promise(resolve=>{let header=0;const observer=new MutationObserver(records=>{header+=records.length});observer.observe(document.querySelector(".header"),{attributes:true,childList:true,subtree:true,characterData:true});for(let index=0;index<200;index++)window.__ARKDOCK_DEMO__.emitCurrentFrame();requestAnimationFrame(()=>requestAnimationFrame(()=>{observer.disconnect();resolve(header)}));})');
    const after = await driver.send('Performance.getMetrics');
    const wanted = new Set(['LayoutCount','RecalcStyleCount','JSHeapUsedSize','TaskDuration','ScriptDuration']);
    performance.push({workload:'200 explicitly injected labelled test frames in one event burst; no FPS assertion',motionPreference:await driver.evaluate('document.documentElement.dataset.motion'),headerMutations:mutations,before:before.metrics.filter(e=>wanted.has(e.name)),after:after.metrics.filter(e=>wanted.has(e.name))});
    assert.equal(mutations,0); assert.equal((await state()).preview,'running');
  });
  await check('V3: production bundle excludes fixtures and demo route import', async () => {
    assert.ok(existsSync('dist/index.html'), 'Build production dist before acceptance');
    const bundle = files('dist').filter(path=>/\.(js|html)$/.test(path)).map(path=>readFileSync(path,'utf8')).join('\n');
    assert.ok(!/__ARKDOCK_DEMO__|UI 测试画面|demo-candidate-|demo-source-session|ui-demo\.html/.test(bundle), 'Production build contains fixture payload');
    assert.equal(existsSync('dist/ui-demo.html'), false);
  });
  await check('Browser execution raised no uncaught runtime exceptions', async () => { assert.deepEqual(browserErrors, []); });
} catch (error) {
  checks.push({ status: 'FAIL', name: 'browser acceptance setup', reason: error.message }); console.error(error); process.exitCode = 1;
} finally {
  if (driver) { try { await driver.send('Browser.close'); } catch { /* Browser may already have closed. */ } driver.close(); }
  if (browser && browser.exitCode === null) browser.kill();
  if (server && server.exitCode === null) server.kill();
  // The only removable tree is the exact temporary profile this process created.
  if (profile && resolve(profile).startsWith(`${resolve(tmpdir())}${sep}arkdock-ui-owned-`)) { await pause(250); try { rmSync(profile, { recursive: true, force: true }); } catch { /* Browser release may be delayed; retain this owned profile. */ } }
  mkdirSync(evidence, { recursive: true });
  writeFileSync(join(evidence, 'checks.json'), `${JSON.stringify({ capturedAt: new Date().toISOString(), scope: 'V1–V4 browser UI fixtures only; no native window or HarmonyOS claim', dockLogicalSizes: sizes, extraDemoBannerDip: 44, demoBuild, url, node: process.version, platform: process.platform, checks, screenshots, browserErrors, serverTail }, null, 2)}\n`);
  writeFileSync(join(evidence, 'motion-samples.json'), `${JSON.stringify({ scope: 'CSS animation samples from running owned Chromium; not native snap animation or FPS measurement', samples: motion }, null, 2)}\n`);
  writeFileSync(join(evidence, 'performance.json'), `${JSON.stringify({scope:'Browser counters under explicit UI fixture frame burst; no native transport, Windows compositor, or 60 FPS claim',samples:performance}, null, 2)}\n`);
  writeFileSync(join(evidence, 'contrast.json'), `${JSON.stringify({scope:'Measured core text tokens and actual primary action colors; not a complete assistive-technology audit',samples:contrast}, null, 2)}\n`);
  console.log(`Evidence: ${evidence}`);
}
