const assert = require('node:assert/strict');
const { test } = require('node:test');
const { readFileSync } = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const ts = require('typescript');
const React = require('react');
const { renderToStaticMarkup } = require('react-dom/server');
const root = path.resolve(__dirname, '..');
test('directory submission and retry carry explicit directory policy through the existing commands', async () => {
  const calls = [];
  const context = { getInvoke: () => async (command, args) => { calls.push({command, args}); } };
  const submit = production('src/remoteTransfersApi.ts', 'remoteTransferSubmit', context);
  await submit('a', {direction:'upload', localPath:'C:/tree', remotePath:'/tree'}, {kind:'directory', directoryConflict:'ask'});
  assert.equal(calls[0].args.request.kind, 'directory');
  assert.equal(calls[0].args.request.directoryConflict, 'ask');
  assert.equal(calls[0].args.request.transfer.conflict, 'ask');
  const retry = production('src/remoteTransfersApi.ts', 'remoteTransferRetry', context);
  await retry('task', 'skip', 'merge');
  assert.equal(calls[1].command, 'remote_transfer_retry');
  assert.equal(calls[1].args.directoryConflict, 'merge');
  assert.equal(calls[1].args.conflict, 'skip');
});
test('directory rows expose skipped files, relative failures, and cleanup warnings', () => {
  const render = production('src/components/remote/RemoteTransfersPanel.tsx', 'TransferTaskBody');
  const task = {state:'failed',bytes:'3',total:'10',attempt:1,totalFiles:3,completedFiles:1,skippedFiles:1,
    request:{kind:'directory',transfer:{direction:'upload',remotePath:'/tree',localPath:'C:/tree'}},
    error:{code:'publish',message:'file failed',relativePath:'子目录/<bad>',cleanupWarnings:[]},
    directoryReceipt:{cleanupWarnings:['inspect owned staging']}};
  const html = renderToStaticMarkup(React.createElement(render,{task,label:k=>k}));
  assert.match(html, /directory/);
  assert.match(html, /skippedFiles 1/);
  assert.match(html, /failedFile.*子目录\/&lt;bad&gt;/);
  assert.match(html, /inspect owned staging/);
  assert.doesNotMatch(html, /<bad>/);
});
function production(file, name, context = {}) { const s = readFileSync(path.join(root, file), 'utf8'); const ast = ts.createSourceFile(file, s, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX); const fn = ast.statements.find(n => ts.isFunctionDeclaration(n) && n.name?.text === name); assert.ok(fn, name); return vm.runInNewContext(ts.transpileModule(fn.getText(ast).replace(/^export /, '') + '\n' + name + ';', { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.React } }).outputText, { React, ...context }); }
test('new transfers explicitly default to ask; no credential or overwrite boolean is sent', async () => { const calls = []; const fn = production('src/remoteTransfersApi.ts', 'remoteTransferSubmit', { getInvoke: () => async (cmd, args) => { calls.push({ cmd, args }); return { id: 'fixture' }; } }); await fn('a', { direction: 'upload', localPath: 'C:/fixture.bin', remotePath: '/fixture.bin' }); assert.equal(calls[0].cmd, 'remote_transfer_submit'); assert.equal(calls[0].args.request.connectionId, 'a'); assert.equal(calls[0].args.request.transfer.conflict, 'ask'); assert.equal('overwrite' in calls[0].args.request.transfer, false); });
test('terminal state UI shows exact byte values and conflict actions without saying cancellation is finished early', () => { const file = 'src/components/remote/RemoteTransfersPanel.tsx'; const fn = production(file, 'TransferTaskBody'); const task = { id: 'x', state: 'cancelling', bytes: '18446744073709551614', total: '18446744073709551615', attempt: 2, request: { transfer: { direction: 'download', remotePath: '/file', localPath: 'C:/out' } }, error: null }; const html = renderToStaticMarkup(React.createElement(fn, { task, label: k => k })); assert.match(html, /cancelling/); assert.match(html, /18446744073709551614/); assert.doesNotMatch(html, /18446744073709552000|NaN/); });
test('progress UI labels exact backend average speed, current file and file counts', () => {
  const fn = production('src/components/remote/RemoteTransfersPanel.tsx', 'TransferTaskBody');
  const task = { state: 'running', bytes: '18446744073709551614', total: '18446744073709551615',
    speedBps: '9007199254740993', currentFile: '/目录/<file>.bin', totalFiles: 7, completedFiles: 2,
    attempt: 1, request: { transfer: { direction: 'upload', remotePath: '/batch', localPath: 'C:/out' } } };
  const html = renderToStaticMarkup(React.createElement(fn, { task, label: k => k }));
  assert.match(html, /averageSpeed 9007199254740993 B\/s/);
  assert.match(html, /currentFile.*目录\/&lt;file&gt;\.bin/);
  assert.match(html, /files 2 \/ 7/);
  assert.match(html, /18446744073709551614/);
  assert.doesNotMatch(html, /9007199254740992|NaN|<file>/);
});
test('legacy unknown totals stay unknown and terminal rows hide stale live speed', () => {
  const fn = production('src/components/remote/RemoteTransfersPanel.tsx', 'TransferTaskBody');
  const task = { state: 'completed', bytes: '0', total: null, attempt: 1, speedBps: '123456789',
    request: { transfer: { direction: 'download', remotePath: '/file', localPath: 'C:/out' } } };
  const html = renderToStaticMarkup(React.createElement(fn, { task, label: k => k }));
  assert.match(html, /files \? \/ \?/);
  assert.match(html, /averageSpeed — B\/s/);
  assert.doesNotMatch(html, /123456789|undefined|NaN|100%/);
});
test('native drag files are submitted by exact path through the same queue and never forced overwrite', () => { const file = readFileSync(path.join(root, 'src/components/remote/RemoteSidebar.tsx'), 'utf8'); assert.ok(!/await sshPushFile\(conn\.id/.test(file), 'drag must not use old destructive upload'); assert.ok(file.includes('remoteTransferSubmit(conn.id'), 'drag enters queue'); assert.ok(!/paths\.find\(/.test(file), 'same basename cannot select the wrong local file'); });

test('panel consumes the shared observer and unmount only unsubscribes', async () => {
  const file = 'src/components/remote/RemoteTransfersPanel.tsx';
  const source = readFileSync(path.join(root, file), 'utf8');
  const effects = []; const updates = []; const listeners = new Set(); let subscriptions = 0;
  const hooks = { ...React, useEffect: effect => effects.push(effect),
    useState: value => [value, next => updates.push(next)] };
  const observer = {observeRemoteTransfers: listener => {
    subscriptions++; listeners.add(listener); return () => listeners.delete(listener);
  }};
  const module = {exports:{}};
  vm.runInNewContext(ts.transpileModule(source, {compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.CommonJS,jsx:ts.JsxEmit.ReactJSX}}).outputText, {
    exports:module.exports,module,setTimeout:()=>1,clearTimeout:()=>{},
    require: name => name === 'react' ? hooks : name === 'react/jsx-runtime' ? require('react/jsx-runtime') : name === 'react-i18next' ? {useTranslation:()=>({t:k=>k})} : name.includes('remoteTransferObserver') ? observer : new Proxy({}, {get:()=>()=>null}),
  });
  module.exports.RemoteTransfersPanel({connectionId:'fixture'});
  const close = effects[0]();
  close();
  module.exports.RemoteTransfersPanel({connectionId:'fixture'});
  const cleanup = effects[1]();
  assert.equal(subscriptions,2); assert.equal(listeners.size,1,'old component unsubscribes');
  for (const listener of listeners) listener.next({revision:'1',tasks:[]});
  assert.ok(updates.length>0);
  cleanup(); const before = updates.length;
  for (const listener of listeners) listener.next({revision:'2',tasks:[]});
  await new Promise(setImmediate);
  assert.equal(updates.length,before,'disposed effects cannot update UI');
});
