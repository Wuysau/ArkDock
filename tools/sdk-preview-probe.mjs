// Bounded manual compatibility probe for the user's installed SDK. Not a product adapter.
import net from 'node:net';
import { spawn } from 'node:child_process';
import { mkdirSync, writeFileSync, createWriteStream } from 'node:fs';
import { resolve, join } from 'node:path';
import { randomUUID } from 'node:crypto';

const sdk = process.env.ARKDOCK_SDK;
if (!sdk || process.platform !== 'win32') throw new Error('Explicit Windows SDK required');
const root = resolve('examples/native-counter');
const previewer = join(sdk, 'sdk/default/openharmony/previewer/common/bin/Previewer.exe');
const base = `ArkDockProbe_${randomUUID().replaceAll('-', '')}`;
const trace = `${base}_trace`;
const servers = [];
const connections = [];
const log = createWriteStream('artifacts/sdk-preview-probe.log');
const record = (message) => { console.log(message); log.write(`${message}\n`); };
const listen = async (name, onData) => {
  const server = net.createServer((socket) => { connections.push(socket); record(`CONNECTED ${name}`); socket.on('data', onData); });
  servers.push(server);
  await new Promise((done, reject) => { server.once('error', reject); server.listen(`\\\\.\\pipe\\${name}`, done); });
};
mkdirSync('artifacts', { recursive: true });
let commandSocket;
let jsonBuffer = '';
await listen(`${base}_commandPipe`, (data) => {
  jsonBuffer += data.toString();
  if (jsonBuffer.length > 1024 * 1024) throw new Error('Command limit exceeded');
  const messages = jsonBuffer.split('\0'); jsonBuffer = messages.pop();
  for (const message of messages) record(`COMMAND ${message.trim()}`);
});
servers[0].on('connection', (socket) => { commandSocket = socket; });
await listen(trace, (data) => record(`TRACE ${data.toString().slice(0, 4096)}`));
const portServer = net.createServer();
await new Promise((done) => portServer.listen(0, '127.0.0.1', done));
const port = portServer.address().port;
await new Promise((done) => portServer.close(done));
const args = ['-j', join(root, 'entry/build/default/intermediates/loader_out/default/ets'),
  '-arp', join(root, 'entry/build/default/intermediates/res/default'),
  '-ljPath', join(root, 'entry/build/default/intermediates/loader/default/loader.json'),
  '-s', base, '-ts', trace, '-lws', String(port), '-or', '360', '640', '-cr', '360', '640',
  '-device', 'phone', '-pm', 'Stage', '-av', 'ACE_2_0', '-url', 'pages/Index', '-n', 'entry', '-sd', '160'];
record(`ARGV ${JSON.stringify(args)}`);
const child = spawn(previewer, args, { cwd: join(sdk, 'sdk/default/openharmony/previewer/common/bin'), shell: false, windowsHide: true });
child.stdout.on('data', (data) => log.write(data)); child.stderr.on('data', (data) => log.write(data));
let exitCode;
child.once('exit', (code) => { exitCode = code; record(`EXIT ${code}`); });
let ws;
let frames = 0;
try {
  for (let i = 0; i < 30 && exitCode === undefined; i++) {
    await new Promise((done) => setTimeout(done, 300));
    try {
      await new Promise((done, reject) => {
        const socket = net.createConnection({ host: '127.0.0.1', port });
        socket.once('error', reject); socket.once('connect', () => { socket.destroy(); done(); });
      });
      ws = new WebSocket(`ws://127.0.0.1:${port}`, 'ws'); ws.binaryType = 'arraybuffer';
      ws.addEventListener('message', ({ data }) => {
        const buffer = Buffer.from(data);
        record(`FRAME ${buffer.length} ${buffer.subarray(0, 48).toString('hex')}`);
        if (++frames <= 3) writeFileSync(`artifacts/sdk-frame-${frames}.bin`, buffer);
        writeFileSync('artifacts/sdk-frame-latest.jpg', buffer.subarray(40));
      });
      ws.addEventListener('error', () => record('WS_ERROR'));
      break;
    } catch { /* Actual server is not ready. */ }
  }
  await new Promise((done) => setTimeout(done, 3000));
  if (commandSocket) {
    const send = (command, args = {}, type = 'action') => commandSocket.write(`${JSON.stringify({ version: '1.0.1', command, type, args })}\0`);
    send('MousePress', { x: 180, y: 360 });
    await new Promise((done) => setTimeout(done, 100));
    send('MouseRelease', { x: 180, y: 360 });
    await new Promise((done) => setTimeout(done, 1000));
    send('inspector', {}, 'action');
    await new Promise((done) => setTimeout(done, 200));
    send('inspectorDefault', {}, 'action');
  }
  await new Promise((done) => setTimeout(done, 2000));
  record(`RESULT frames=${frames} exit=${exitCode ?? 'running'}`);
} finally {
  ws?.close();
  if (exitCode === undefined) { child.kill(); await new Promise((done) => child.once('exit', done)); }
  for (const connection of connections) connection.destroy();
  for (const server of servers) server.close();
  log.end();
}
process.exitCode = frames ? 0 : 2;
