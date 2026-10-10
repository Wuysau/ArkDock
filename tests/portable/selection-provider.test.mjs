import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, writeFile, rm, symlink } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { createHash } from 'node:crypto';
import { Readable, Writable } from 'node:stream';
import { SelectionProvider, runStdio } from '../../tools/selection-provider.mjs';

const TOOLS = ['arkdock.get_current_selection','arkdock.get_selection','arkdock.get_selection_screenshot','arkdock.get_selection_context'];

async function fixture(t) {
  const temporary = await mkdtemp(join(tmpdir(), 'arkdock-selection-provider-test-'));
  t.after(() => rm(temporary, { recursive: true, force: true }));
  const directory = join(temporary, 'selections'); await mkdir(directory);
  const id = 'sel-unit-fixture'; await mkdir(join(directory,id));
  // The tiny JPEG marker fixture tests transport/integrity; it is not ArkUI evidence.
  const image = Buffer.from([0xff,0xd8,0xff,0xd9]);
  const artifact = (artifactId) => ({artifactId,format:'jpeg',width:1,height:1,byteLength:image.length,
    sha256:createHash('sha256').update(image).digest('hex')});
  const context = {selectionId:id,workspaceId:'workspace',sessionId:'session',workspaceName:'Test fixture',
    buildId:'build',generation:1,frameSequence:1,componentBounds:{x:0,y:0,width:1,height:1},
    selectedNode:null,sourceEvidence:{status:'unavailable',reason:'No verified source'},
    userInstruction:'Change selected area',createdAt:100,expiresAt:600000,
    screenshot:artifact('screenshot.jpg'),crop:artifact('crop.jpg')};
  const contextPath = join(directory,id,'context.json'); await writeFile(contextPath,JSON.stringify(context));
  const runtime = {workspaceId:'workspace',sessionId:'session',buildId:'build',generation:1,available:true,observedAt:100,expiresAt:600000};
  const runtimePath = join(directory,'runtime.json'); await writeFile(runtimePath,JSON.stringify(runtime));
  await writeFile(join(directory,id,'screenshot.jpg'),image); await writeFile(join(directory,id,'crop.jpg'),image);
  await writeFile(join(directory,'current.json'),JSON.stringify({selectionId:id,workspaceId:'workspace',sessionId:'session'}));
  const grant = {version:1,directory,workspaceId:'workspace',sessionId:'session',allowedTools:TOOLS,selectionIds:[id],expiresAt:600000};
  const capabilityFile = join(temporary,'grant.json'); await writeFile(capabilityFile,JSON.stringify(grant));
  let now = 101;
  const options = {directory,workspaceId:'workspace',sessionId:'session',capabilityFile,now:()=>now};
  return {temporary,directory,id,context,contextPath,runtime,runtimePath,grant,capabilityFile,options,setNow:(value)=>{now=value;},provider:await SelectionProvider.open(options)};
}

test('provider returns only explicitly granted selection context and original bytes',async(t)=>{
  const f = await fixture(t);
  const result = await f.provider.call('arkdock.get_current_selection',{});
  assert.equal(result.structuredContent.selectionId,f.id);
  const screenshot = await f.provider.call('arkdock.get_selection_screenshot',{selectionId:f.id});
  assert.deepEqual(Buffer.from(screenshot.content[0].data,'base64'),Buffer.from([0xff,0xd8,0xff,0xd9]));
  const prompt = await f.provider.call('arkdock.get_selection_context',{selectionId:f.id});
  assert.match(prompt.content[0].text,/Change selected area/);
  assert.equal(f.provider.listTools().length,4);
  assert.ok(f.provider.listTools().every((tool)=>tool.annotations.readOnlyHint));
});

test('workspace session expiry traversal and unauthorized selections are rejected',async(t)=>{
  const f = await fixture(t);
  await assert.rejects(SelectionProvider.open({...f.options,workspaceId:'other'}),/GRANT_SCOPE_MISMATCH/);
  await assert.rejects(f.provider.call('arkdock.get_selection',{selectionId:'../context'}),/SELECTION_ID_INVALID/);
  await assert.rejects(f.provider.call('arkdock.get_selection',{selectionId:'sel-ungranted'}),/SELECTION_NOT_AUTHORIZED/);
  f.context.workspaceId='other'; await writeFile(f.contextPath,JSON.stringify(f.context));
  await assert.rejects(f.provider.call('arkdock.get_selection',{selectionId:f.id}),/SELECTION_SCOPE_MISMATCH/);
  f.context.workspaceId='workspace'; f.context.sessionId='other'; await writeFile(f.contextPath,JSON.stringify(f.context));
  await assert.rejects(f.provider.call('arkdock.get_selection',{selectionId:f.id}),/SELECTION_SCOPE_MISMATCH/);
  f.context.sessionId='session'; f.context.expiresAt=102; await writeFile(f.contextPath,JSON.stringify(f.context));
  f.setNow(102);
  await assert.rejects(f.provider.call('arkdock.get_selection',{selectionId:f.id}),/SELECTION_EXPIRED/);
  f.setNow(600000);
  await assert.rejects(f.provider.call('arkdock.get_selection',{selectionId:f.id}),/GRANT_EXPIRED/);
});

test('tampered screenshots and extra arguments never expose foreign files',async(t)=>{
  const f = await fixture(t);
  await writeFile(join(f.directory,f.id,'screenshot.jpg'),'tampered');
  await assert.rejects(f.provider.call('arkdock.get_selection_screenshot',{selectionId:f.id}),/IMAGE_INTEGRITY_MISMATCH/);
  await assert.rejects(f.provider.call('arkdock.get_selection_screenshot',{selectionId:f.id,path:'C:/secret'}),/ARGUMENTS_INVALID/);
  f.context.screenshot.artifactId='../foreign.jpg'; await writeFile(f.contextPath,JSON.stringify(f.context));
  await assert.rejects(f.provider.call('arkdock.get_selection_screenshot',{selectionId:f.id}),/IMAGE_METADATA_INVALID/);
});

test('rebuild stop and expired runtime lease block old selection even with valid grant',async(t)=>{
  const f=await fixture(t);
  f.runtime.buildId='new-build'; await writeFile(f.runtimePath,JSON.stringify(f.runtime));
  await assert.rejects(f.provider.call('arkdock.get_selection',{selectionId:f.id}),/SELECTION_RUNTIME_STALE/);
  f.runtime.buildId='build'; f.runtime.available=false; await writeFile(f.runtimePath,JSON.stringify(f.runtime));
  await assert.rejects(f.provider.call('arkdock.get_current_selection',{}),/SELECTION_RUNTIME_STALE/);
  f.runtime.available=true; f.runtime.expiresAt=101; await writeFile(f.runtimePath,JSON.stringify(f.runtime));
  await assert.rejects(f.provider.call('arkdock.get_selection',{selectionId:f.id}),/RUNTIME_LEASE_EXPIRED/);
});

test('canonical directory boundaries reject symlinked selection directory',async(t)=>{
  const f = await fixture(t); const foreign = join(f.temporary,'foreign'); await mkdir(foreign);
  await writeFile(join(foreign,'context.json'),JSON.stringify({...f.context,selectionId:'sel-linked'}));
  try { await symlink(foreign,join(f.directory,'sel-linked'),'junction'); }
  catch(error) { if(error.code==='EPERM') {t.skip('Host cannot create test directory symlink');return;} throw error; }
  f.provider.grant.selectionIds.push('sel-linked');
  await assert.rejects(f.provider.call('arkdock.get_selection',{selectionId:'sel-linked'}),/PATH_OUTSIDE_SELECTIONS/);
});

test('stdio MCP requires initialization and returns honest read-only errors without OS paths',async(t)=>{
  const f = await fixture(t); const outputs=[];
  const output = new Writable({write(chunk,encoding,callback){outputs.push(chunk.toString());callback();}});
  const lines = [
    {jsonrpc:'2.0',id:1,method:'tools/list'},
    {jsonrpc:'2.0',id:2,method:'initialize',params:{protocolVersion:'2025-03-26'}},
    {jsonrpc:'2.0',method:'notifications/initialized'},
    {jsonrpc:'2.0',id:3,method:'tools/list'},
    {jsonrpc:'2.0',id:4,method:'tools/call',params:{name:'arkdock.send',arguments:{}}},
    {jsonrpc:'2.0',id:5,method:'tools/call',params:{name:'arkdock.get_selection',arguments:{selectionId:f.id}}},
  ];
  await runStdio(f.provider,Readable.from(lines.map((line)=>`${JSON.stringify(line)}\n`)),output);
  const results=outputs.map((line)=>JSON.parse(line));
  assert.equal(results[0].error.message,'NOT_INITIALIZED');
  assert.equal(results[1].result.serverInfo.name,'arkdock-selection');
  assert.equal(results[2].result.tools.length,4);
  assert.equal(results[3].result.isError,true); assert.equal(results[3].result.content[0].text,'TOOL_NOT_AUTHORIZED');
  assert.equal(results[4].result.structuredContent.selectionId,f.id);
  assert.ok(!JSON.stringify(results).includes(f.temporary));
});

test('provider rejects missing explicit config and bounded stdio payload overflow',async(t)=>{
  const f=await fixture(t);
  await assert.rejects(SelectionProvider.open({}),/ABSOLUTE_PATH_REQUIRED/);
  await assert.rejects(runStdio(f.provider,Readable.from(['x'.repeat(65537)]),new Writable({write(_c,_e,cb){cb();}})),/REQUEST_LIMIT/);
  f.grant.allowedTools=['arkdock.get_selection']; await writeFile(f.capabilityFile,JSON.stringify(f.grant));
  const restricted=await SelectionProvider.open(f.options);
  assert.equal(restricted.listTools().length,1);
  await assert.rejects(restricted.call('arkdock.get_current_selection',{}),/TOOL_NOT_AUTHORIZED/);
});

test('MCP negotiates a supported version for newer clients and waits for initialized notification',async(t)=>{
  const f=await fixture(t),outputs=[];
  const output=new Writable({write(chunk,_encoding,callback){outputs.push(JSON.parse(chunk.toString()));callback();}});
  await runStdio(f.provider,Readable.from([
    {jsonrpc:'2.0',id:1,method:'initialize',params:{protocolVersion:'2025-11-25'}},
    {jsonrpc:'2.0',id:2,method:'tools/list'},
    {jsonrpc:'2.0',method:'notifications/initialized'},
    {jsonrpc:'2.0',id:3,method:'tools/list'},
  ].map((request)=>`${JSON.stringify(request)}\n`)),output);
  assert.equal(outputs[0].result.protocolVersion,'2025-06-18');
  assert.equal(outputs[1].error.message,'NOT_INITIALIZED');
  assert.equal(outputs[2].result.tools.length,4);
});
