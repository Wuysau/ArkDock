import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from '../../src/App.tsx';
import '../../src/styles.css';
import { createFixtureDriver, scenarios } from './fixtures.ts';
import type { FixtureDriver, Scenario } from './fixtures.ts';

declare global { interface Window { __ARKDOCK_DEMO__: FixtureDriver } }
const query = new URLSearchParams(location.search);
const requested = query.get('scenario');
const initial: Scenario = requested && requested in scenarios ? requested as Scenario : 'welcome';
if (query.get('theme') === 'dark' || query.get('theme') === 'light') localStorage.setItem('arkdock:theme', query.get('theme')!);
const driver = createFixtureDriver(initial);
window.__ARKDOCK_DEMO__ = driver;
function Demo() {
  const [scenario, setScenario] = useState<Scenario>(initial);
  return <div className="ui-demo-shell">
    <aside className="ui-demo-banner" aria-label="开发演示标识"><strong>UI 演示 · 非原生渲染</strong>
      <select aria-label="演示场景" value={scenario} onChange={(event) => { const next = event.target.value as Scenario; setScenario(next); driver.setScenario(next); }}>
        {Object.entries(scenarios).map(([key, label]) => <option key={key} value={key}>{label}</option>)}
      </select>
    </aside>
    <div className="ui-demo-application"><App bridge={driver.bridge} /></div>
  </div>;
}
const style = document.createElement('style');
style.textContent = `html,body,#demo-root{width:100%;height:100%;margin:0;overflow:hidden}.ui-demo-shell{height:100%;display:flex;flex-direction:column}.ui-demo-banner{flex:0 0 44px;display:flex;align-items:center;justify-content:space-between;gap:8px;padding:0 12px;background:#392d16;color:#ffdfa5;border-bottom:1px solid #987d40;font:11px "Segoe UI","Microsoft YaHei UI",sans-serif}.ui-demo-banner strong{white-space:nowrap}.ui-demo-banner select{min-width:0;max-width:148px;padding:5px;background:#261e10;color:#ffdfa5;border:1px solid #987d40;border-radius:6px;font:inherit}.ui-demo-application{flex:1;min-height:0;overflow:hidden}.ui-demo-application>.app{height:100%;min-height:0}`;
document.head.append(style);
createRoot(document.getElementById('demo-root')!).render(<Demo />);
