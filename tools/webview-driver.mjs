// Development-only driver for the actual Windows WebView2, never a preview backend.
import { execFileSync } from 'node:child_process';
import { ownedDesktop } from './owned-desktop.mjs';

export async function connectOwnedWebview(port = Number(process.env.ARKDOCK_DEBUG_PORT ?? 9223), timeoutMs = 5000, expectedUrl = process.env.ARKDOCK_UI_URL ?? 'http://127.0.0.1:1420/') {
  if (!Number.isInteger(port) || port < 1 || port > 65535) throw new Error('Invalid development debugger port');
  const desktop = ownedDesktop();
  // The debugger is owned by a WebView2 child, not necessarily the main ArkDock PID.
  // Verify its real process ancestry before connecting or sending any IPC command.
  execFileSync('pwsh', ['-NoProfile', '-Command', `
    $ErrorActionPreference='Stop'
    $taskListeners=@(Get-NetTCPConnection -State Listen -LocalPort ${port} | Select-Object -ExpandProperty OwningProcess -Unique)
    if($taskListeners.Count -ne 1){throw 'Expected one owned debugger listener'}
    $taskAncestorId=[uint32]$taskListeners[0]
    $taskOwned=$false
    for($taskStep=0;$taskStep -lt 32;$taskStep++){
      if($taskAncestorId -eq ${desktop.Id}){$taskOwned=$true;break}
      $taskProcess=Get-CimInstance Win32_Process -Filter ('ProcessId='+$taskAncestorId)
      if(-not $taskProcess -or $taskProcess.ParentProcessId -eq $taskAncestorId){break}
      $taskAncestorId=[uint32]$taskProcess.ParentProcessId
    }
    if(-not $taskOwned){throw 'Debugger belongs to another desktop instance; no IPC will be sent'}
  `], { encoding: 'utf8', windowsHide: true, timeout: 12_000 });
  return connectWebview(port, timeoutMs, expectedUrl);
}

export async function connectWebview(port = Number(process.env.ARKDOCK_DEBUG_PORT ?? 9223), timeoutMs = 5000, expectedUrl = process.env.ARKDOCK_UI_URL ?? 'http://127.0.0.1:1420/') {
  if (!Number.isInteger(port) || port < 1 || port > 65535) throw new Error('Invalid development debugger port');
  const requested = new URL(expectedUrl);
  if (requested.protocol !== 'http:' || requested.hostname !== '127.0.0.1' || requested.pathname !== '/') throw new Error('Only an explicitly configured local ArkDock main URL may be inspected');
  const pages = await (await fetch(`http://127.0.0.1:${port}/json/list`, {signal:AbortSignal.timeout(timeoutMs)})).json();
  const page = pages.find((item) => item.type === 'page' && item.url === expectedUrl);
  if (!page) throw new Error('Requested ArkDock WebView2 was not found');
  const endpoint = new URL(page.webSocketDebuggerUrl);
  if (endpoint.protocol !== 'ws:' || endpoint.hostname !== '127.0.0.1' || Number(endpoint.port) !== port) throw new Error('Debugger endpoint must use the same loopback port');
  const socket = new WebSocket(endpoint);
  await new Promise((resolve, reject) => {
    const timer = setTimeout(() => { reject(new Error('WebView2 debugger handshake timed out')); socket.close(); }, timeoutMs);
    socket.onopen = () => { clearTimeout(timer); resolve(); };
    socket.onerror = () => { clearTimeout(timer); reject(new Error('WebView2 debugger connection failed')); };
    socket.onclose = () => { clearTimeout(timer); reject(new Error('WebView2 debugger connection closed')); };
  });
  let serial = 0;
  const pending = new Map();
  const listeners = new Set();
  socket.onmessage = (event) => {
    const response = JSON.parse(event.data);
    if (response.method) {
      for (const listener of listeners) listener(response.method, response.params);
      return;
    }
    const request = pending.get(response.id);
    if (!request) return;
    pending.delete(response.id); clearTimeout(request.timer);
    if (response.error) request.reject(new Error(response.error.message));
    else request.resolve(response.result);
  };
  function send(method, params = {}) {
    const id = ++serial;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { pending.delete(id); reject(new Error(`Timed out: ${method}`)); }, 10_000);
      pending.set(id, { resolve, reject, timer });
      socket.send(JSON.stringify({ id, method, params }));
    });
  }
  async function evaluate(expression) {
    const result = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
    if (result.exceptionDetails) throw new Error(result.exceptionDetails.text + ': ' + (result.exceptionDetails.exception?.value ?? result.exceptionDetails.exception?.description ?? result.result.description ?? ''));
    return result.result.value;
  }
  return { send, evaluate, onEvent: (listener) => { listeners.add(listener); return () => listeners.delete(listener); },
    close: () => { listeners.clear(); socket.close(); } };
}
