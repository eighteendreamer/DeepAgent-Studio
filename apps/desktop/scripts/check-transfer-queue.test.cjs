const assert = require('node:assert/strict');
const { test } = require('node:test');
const { readFileSync } = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const ts = require('typescript');
const React = require('react');
const { renderToStaticMarkup } = require('react-dom/server');
const root = path.resolve(__dirname, '..');
function production(file, name, context = {}) { const s = readFileSync(path.join(root, file), 'utf8'); const ast = ts.createSourceFile(file, s, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX); const fn = ast.statements.find(n => ts.isFunctionDeclaration(n) && n.name?.text === name); assert.ok(fn, name); return vm.runInNewContext(ts.transpileModule(fn.getText(ast).replace(/^export /, '') + '\n' + name + ';', { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.React } }).outputText, { React, ...context }); }
test('new transfers explicitly default to ask; no credential or overwrite boolean is sent', async () => { const calls = []; const fn = production('src/remoteTransfersApi.ts', 'remoteTransferSubmit', { getInvoke: () => async (cmd, args) => { calls.push({ cmd, args }); return { id: 'fixture' }; } }); await fn('a', { direction: 'upload', localPath: 'C:/fixture.bin', remotePath: '/fixture.bin' }); assert.equal(calls[0].cmd, 'remote_transfer_submit'); assert.equal(calls[0].args.request.connectionId, 'a'); assert.equal(calls[0].args.request.transfer.conflict, 'ask'); assert.equal('overwrite' in calls[0].args.request.transfer, false); });
test('terminal state UI shows exact byte values and conflict actions without saying cancellation is finished early', () => { const file = 'src/components/remote/RemoteTransfersPanel.tsx'; const fn = production(file, 'TransferTaskBody'); const task = { id: 'x', state: 'cancelling', bytes: '18446744073709551614', total: '18446744073709551615', attempt: 2, request: { transfer: { direction: 'download', remotePath: '/file', localPath: 'C:/out' } }, error: null }; const html = renderToStaticMarkup(React.createElement(fn, { task, label: k => k })); assert.match(html, /cancelling/); assert.match(html, /18446744073709551614/); assert.doesNotMatch(html, /18446744073709552000|NaN/); });
test('native drag files are submitted by exact path through the same queue and never forced overwrite', () => { const file = readFileSync(path.join(root, 'src/components/remote/RemoteSidebar.tsx'), 'utf8'); assert.ok(!/await sshPushFile\(conn\.id/.test(file), 'drag must not use old destructive upload'); assert.ok(file.includes('remoteTransferSubmit(conn.id'), 'drag enters queue'); assert.ok(!/paths\.find\(/.test(file), 'same basename cannot select the wrong local file'); });

test('effect refresh shares an in-flight snapshot and fences late writes after cleanup', async () => {
  const file = 'src/components/remote/RemoteTransfersPanel.tsx';
  const source = readFileSync(path.join(root, file), 'utf8');
  const effects = []; const updates = []; const refs = []; let refIndex = 0; let reply; let requests = 0;
  const pending = new Promise(resolve => { reply = resolve; });
  const hooks = { ...React, useEffect: effect => effects.push(effect),
    useRef: value => { const i = refIndex++; return refs[i] ??= {current:value}; },
    useState: value => [value, next => updates.push(next)] };
  const api = {remoteTransfersSnapshot: async () => {requests++; return pending;}};
  const module = {exports:{}};
  vm.runInNewContext(ts.transpileModule(source, {compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.CommonJS,jsx:ts.JsxEmit.ReactJSX}}).outputText, {
    exports:module.exports,module,setTimeout:()=>1,clearTimeout:()=>{},
    require: name => name === 'react' ? hooks : name === 'react/jsx-runtime' ? require('react/jsx-runtime') : name === 'react-i18next' ? {useTranslation:()=>({t:k=>k})} : name.includes('remoteTransfersApi') ? api : new Proxy({}, {get:()=>()=>null}),
  });
  module.exports.RemoteTransfersPanel({connectionId:'fixture'});
  const close = effects[0]();
  close(); refIndex = 0;
  module.exports.RemoteTransfersPanel({connectionId:'fixture'});
  const cleanup = effects[1]();
  assert.equal(requests,1,'refresh must not overlap the previous pending IPC');
  cleanup(); const before = updates.length;
  reply([]); await new Promise(setImmediate);
  assert.equal(updates.length,before,'disposed effects cannot update UI');
});
