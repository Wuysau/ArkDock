// Development-only driver for the actual Windows WebView2, never a preview backend.
export async function connectWebview(port = 9223, timeoutMs = 5000, expectedUrl = 'http://127.0.0.1:1420/') {
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
