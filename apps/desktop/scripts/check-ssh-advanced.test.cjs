const { test } = require('node:test');
const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const ts = require('typescript');

function production(file, name, context = {}) {
  const source = readFileSync(path.resolve(__dirname, '../src', file), 'utf8');
  const ast = ts.createSourceFile(file, source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  let found;
  const visit = node => { if (ts.isFunctionDeclaration(node) && node.name?.text === name) found = node; ts.forEachChild(node, visit); };
  visit(ast); assert.ok(found, name);
  return vm.runInNewContext(ts.transpileModule(`${found.getText(ast).replace(/^export\s+/, '')}\n${name};`, {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
  }).outputText, context);
}

test('forwarding defaults keep listeners local and require explicit exposure authorization', () => {
  const serialize = production('components/remote/SshForwardPanel.tsx', 'forwardingRequest');
  const form = { kind:'local', bindHost:'127.0.0.1', bindPort:'0', targetHost:'service.remote', targetPort:'22', allowNonLoopback:false };
  assert.equal(serialize(form).bind_port, 0);
  assert.equal(serialize({...form, bindHost:'0.0.0.0'}), null);
  assert.equal(serialize({...form, bindHost:'0.0.0.0', allowNonLoopback:true}).allow_non_loopback, true);
  for (const invalid of ['-1','65536','1.5','1e3','']) assert.equal(serialize({...form, bindPort:invalid}), null);
  assert.equal(serialize({...form, targetPort:'0'}), null);
  assert.equal(serialize({...form, targetHost:'bad\nhost'}), null);
  const dynamic = serialize({...form, kind:'dynamic', targetHost:'old-target', targetPort:'old-port'});
  assert.equal(dynamic.target_host, null); assert.equal(dynamic.target_port, null);
});

test('start and stop bind to the reviewed connection generation', async () => {
  const calls = [];
  const getInvoke = () => async (command, arguments) => { calls.push({command, arguments}); return {}; };
  const start = production('api.ts', 'sshStartForward', {getInvoke});
  const stop = production('api.ts', 'sshStopForward', {getInvoke});
  await start('connection', 42, {kind:'dynamic'}); await stop('connection',42,'task');
  assert.equal(calls[0].arguments.generation,42); assert.equal(calls[0].arguments.id,'connection');
  assert.equal(calls[1].arguments.taskId,'task'); assert.equal(calls[1].arguments.generation,42);
});

test('Agent mutation keeps secrets only in the explicit request and requires desktop IPC', async () => {
  let seen;
  const add = production('api.ts','sshAddAgentIdentity', {getInvoke:()=>async (command,args)=> {seen={command,args}; return {already_present:false};}});
  const request={provider:'open_ssh', key_path:'key', key_content:null,passphrase:'temporary',expected_fingerprint:'SHA256:reviewed',lifetime_secs:60,authorized:true};
  await add(request); assert.equal(seen.command,'ssh_add_agent_identity'); assert.equal(seen.args.request,request);
  await assert.rejects(production('api.ts','sshAddAgentIdentity',{getInvoke:()=>null})(request),/desktop/);
  await assert.rejects(production('api.ts','sshStartForward',{getInvoke:()=>null})('id',42,{}),/desktop/);
});

test('advanced options survive both create and update IPC', async () => {
  for (const name of ['sshCreateConnection','sshUpdateConnection']) {
    let submitted;
    const invoke=production('api.ts',name,{getInvoke:()=>async (_,args)=>{submitted=args;return {};}});
    const advanced={forward_agent:true,agent_provider:'pageant',allow_legacy_algorithms:true,x11:null};
    const args=['name','host',22,'user','agent',undefined,undefined,undefined,undefined,undefined,undefined,undefined,undefined,undefined,undefined,undefined,advanced];
    if(name==='sshUpdateConnection')args.unshift('id');
    await invoke(...args);assert.equal(submitted.advanced,advanced);
  }
});
