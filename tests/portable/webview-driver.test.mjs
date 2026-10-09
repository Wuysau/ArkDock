import assert from 'node:assert/strict';
import test from 'node:test';
import { createServer } from 'node:http';
import { connectWebview } from '../../tools/webview-driver.mjs';

test('真实回环 debugger 握手无响应时有界退出，不强杀 native 驱动', async () => {
  const sockets = new Set();
  const server = createServer((_request, response) => {
    response.setHeader('content-type','application/json');
    response.end(JSON.stringify([{type:'page',url:'http://127.0.0.1:1420/',webSocketDebuggerUrl:`ws://127.0.0.1:${server.address().port}/stalled`} ]));
  });
  server.on('upgrade', (_request, socket) => { sockets.add(socket); socket.on('close',() => sockets.delete(socket)); });
  await new Promise((done) => server.listen(0,'127.0.0.1',done));
  try { await assert.rejects(connectWebview(server.address().port,100),/handshake timed out/); }
  finally { for (const socket of sockets) socket.destroy(); server.closeAllConnections(); await new Promise((done) => server.close(done)); }
});
