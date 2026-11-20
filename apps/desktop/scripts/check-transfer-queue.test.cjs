const assert = require('node:assert/strict');
const { test } = require('node:test');
const { readFileSync } = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const ts = require('typescript');
const React = require('react');
const { renderToStaticMarkup } = require('react-dom/server');
const root = path.resolve(__dirname, '..');
test('directory-root responses keep their nonce and cannot become file decisions', async () => {
  const calls = [];
  const respond = production('src/remoteTransfersApi.ts', 'remoteTransferResolveDirectoryConflict', {
    getInvoke: () => async (command, args) => { calls.push({command, args}); },
  });
  const response = {version:1,id:'root-nonce',attempt:2,decision:'merge'};
  await respond('task', response);
  assert.equal(calls[0].command, 'remote_transfer_resolve_directory_conflict');
  assert.equal(JSON.stringify(calls[0].args), JSON.stringify({id:'task',response}));
});

test('directory-root controls show the exact destination and keep this attempt', async () => {
  const harness = directoryActionsHarness();
  harness.props.pending = {version:1,id:'root',attempt:2,conflict:{direction:'download',
    destinationPath:'C:/目录/<target>',totalFiles:7,totalBytes:'9007199254740993'}};
  const tree = harness.draw();
  const html = renderToStaticMarkup(tree);
  assert.match(html,/C:\/目录\/&lt;target&gt;/);
  assert.match(html,/waitingDirectoryConflict|directoryConflictSafe/);
  assert.match(html,/9007199254740993/);
  assert.doesNotMatch(html,/applyToRemainingFiles/);
  for (const label of ['skip','merge','keepBothDirectory']) await harness.button(tree,label).props.onClick();
  assert.deepEqual(harness.calls,['skip','merge','keep_both']);
  harness.props.busy = true;
  assert.equal((renderToStaticMarkup(harness.draw()).match(/disabled=""/g)||[]).length,4);
});

test('directory-root planned counts cannot be presented as transferred files', () => {
  const harness = directoryActionsHarness();
  for (const [locale, countLabel, sizeLabel] of [
    ['en', 'Source files', 'Planned bytes'], ['zh', '源文件数', '计划总字节'],
  ]) {
    const labels = JSON.parse(readFileSync(path.join(root, `src/locales/${locale}.json`), 'utf8')).remote.transfers;
    harness.props.pending = {id:'locale',conflict:{direction:'upload',destinationPath:'/tree',totalFiles:7,totalBytes:'9007199254740993'}};
    harness.props.label = key=>labels[key];
    const html = renderToStaticMarkup(harness.draw());
    assert.ok(html.includes(countLabel), `${locale}: ${html}`);
    assert.ok(html.includes(sizeLabel), `${locale}: ${html}`);
    assert.ok(!html.includes(labels.files), `${locale}: planned count is not a transferred count`);
  }
});

function directoryActionsHarness() {
  return conflictActionsHarness('DirectoryConflictActions');
}

test('whole-directory confirmation preserves deletion scope without implementation copy', () => {
  for (const locale of ['en', 'zh']) {
    const labels = JSON.parse(readFileSync(path.join(root, `src/locales/${locale}.json`), 'utf8')).remote.transfers;
    assert.doesNotMatch(labels.replaceWholeDirectoryWarning, /staged|publication|暂存|校验后|发布|SFTP/i);
    assert.match(labels.replaceWholeDirectoryWarning, locale === 'zh' ? /文件.*子目录.*源目录保留/ : /files.*folders.*source.*preserved/i);
  }
});

test('whole-directory replacement needs a separate confirmation bound to the current nonce', async () => {
  const harness = directoryActionsHarness();
  let tree = harness.draw();
  harness.button(tree,'replaceDirectory').props.onClick();
  assert.equal(harness.calls.length,0);
  tree = harness.draw();
  assert.match(renderToStaticMarkup(tree),/replaceWholeDirectoryWarning/);
  assert.ok(harness.find(tree,node=>node.props?.open === true));
  harness.props.pending = {...harness.props.pending,id:'new-root-nonce'};
  tree = harness.draw();
  assert.equal(harness.find(tree,node=>node.props?.open === true),undefined);
  harness.button(tree,'replaceDirectory').props.onClick();
  await harness.button(harness.draw(),'confirmReplaceDirectory').props.onClick();
  assert.deepEqual(harness.calls,['replace']);
});

test('conflict rows omit explanatory copy while retaining actionable context and confirmations', () => {
  for (const component of ['DirectoryConflictActions', 'FileConflictActions']) {
    const harness = conflictActionsHarness(component);
    const tree = harness.draw();
    const html = renderToStaticMarkup(tree);
    assert.doesNotMatch(html, /directoryConflictSafe|fileConflictSafe/);
    assert.match(html, /destinationPath/);
    assert.match(html, /waitingDirectoryConflict|waitingFileConflict/);
    harness.button(tree, component === 'DirectoryConflictActions' ? 'replaceDirectory' : 'replace').props.onClick();
    assert.match(renderToStaticMarkup(harness.draw()), /replaceWholeDirectoryWarning|replaceCurrentFileWarning/);
    assert.equal(harness.calls.length, 0);
  }
});

test('file conflict responses preserve the task, attempt, nonce and bounded scope', async () => {
  const calls = [];
  const respond = production('src/remoteTransfersApi.ts', 'remoteTransferResolveFileConflict', {
    getInvoke: () => async (command, args) => { calls.push({command, args}); },
  });
  const response = {version:1,id:'nonce',attempt:2,decision:{policy:'keep_both',applyToRemainingFiles:true}};
  await respond('task', response);
  assert.equal(calls[0].command, 'remote_transfer_resolve_file_conflict');
  assert.equal(JSON.stringify(calls[0].args), JSON.stringify({id:'task',response}));
});

function conflictActionsHarness(component = 'FileConflictActions') {
  const values = []; let index = 0;
  const calls = [];
  const control = tag => props => React.createElement(tag, props, props.children);
  const render = production('src/components/remote/RemoteTransfersPanel.tsx', component, {
    useState: initial => { const slot = index++; if (!(slot in values)) values[slot] = initial;
      return [values[slot], value => { values[slot] = value; }]; },
    Button:control('button'), Checkbox:control('span'), Label:control('label'),
    Dialog:control('section'),DialogContent:control('div'),DialogHeader:control('header'),
    DialogTitle:control('h2'),DialogDescription:control('p'),DialogFooter:control('footer'),
  });
  const props = {pending:{version:1,id:'nonce',attempt:1,conflict:{relativePath:'子目录/<file>',
    destinationPath:'/tree/子目录/<file>',direction:'upload',sourceSize:'9007199254740993'}},
    label:key=>key,busy:false,onResolve:async decision=>{calls.push(decision);}};
  const draw = () => { index=0; return render(props); };
  const find = (tree, predicate) => { if (!tree || typeof tree !== 'object') return;
    if (predicate(tree)) return tree;
    for (const child of React.Children.toArray(tree.props?.children)) { const found=find(child,predicate); if(found)return found; } };
  const button = (tree,label) => { const found=find(tree,node=>node.props?.children===label && !!node.props?.onClick); assert.ok(found,label);return found; };
  return {draw,find,button,calls,props};
}

test('pending conflict controls expose exact context and default to this file', async () => {
  const harness = conflictActionsHarness();
  let tree = harness.draw();
  const html = renderToStaticMarkup(tree);
  assert.match(html, /子目录\/&lt;file&gt;/);
  assert.match(html, /9007199254740993/);
  assert.match(html, /applyToRemainingFiles/);
  await harness.button(tree,'skip').props.onClick();
  assert.equal(JSON.stringify(harness.calls[0]),JSON.stringify({policy:'skip',applyToRemainingFiles:false}));
  harness.find(tree,node=>!!node.props?.onCheckedChange).props.onCheckedChange(true);
  tree=harness.draw();
  await harness.button(tree,'keepBoth').props.onClick();
  assert.equal(JSON.stringify(harness.calls[1]),JSON.stringify({policy:'keep_both',applyToRemainingFiles:true}));
});

test('replacement requires confirmation and names current versus remaining scope', async () => {
  const harness = conflictActionsHarness();
  let tree=harness.draw();
  harness.button(tree,'replace').props.onClick();
  assert.equal(harness.calls.length,0);
  tree=harness.draw();
  assert.match(renderToStaticMarkup(tree),/replaceCurrentFileWarning/);
  await harness.button(tree,'confirmReplace').props.onClick();
  assert.equal(harness.calls[0].policy,'replace');
  assert.equal(harness.calls[0].applyToRemainingFiles,false);
  harness.find(tree,node=>!!node.props?.onCheckedChange).props.onCheckedChange(true);
  assert.match(renderToStaticMarkup(harness.draw()),/replaceRemainingFilesWarning/);
});

test('latest file decision translates keep-both and pending rows hide live speed', () => {
  const render=production('src/components/remote/RemoteTransfersPanel.tsx','TransferTaskBody');
  const task={state:'running',bytes:'5',total:'10',attempt:1,speedBps:'99999',pendingFileConflict:{id:'next'},
    request:{transfer:{direction:'upload',remotePath:'/tree',localPath:'C:/tree'}},
    lastFileConflict:{pending:{conflict:{relativePath:'b'}},decision:{policy:'keep_both',applyToRemainingFiles:false}}};
  const html=renderToStaticMarkup(React.createElement(render,{task,label:key=>key}));
  assert.match(html,/keepBoth/);
  assert.doesNotMatch(html,/keep_both|99999/);
});

test('keep-both receipts display actual targets and directory rename counts', () => {
  const render = production('src/components/remote/RemoteTransfersPanel.tsx', 'TransferTaskBody');
  const task = {state:'completed',bytes:'3',total:'3',attempt:1,completedFiles:1,
    request:{kind:'directory',transfer:{direction:'upload',remotePath:'/tree',localPath:'C:/tree'}},
    directoryReceipt:{destinationPath:'/tree (1)/<copy>',renamedFiles:2,cleanupWarnings:[]}};
  const html = renderToStaticMarkup(React.createElement(render,{task,label:k=>k}));
  assert.match(html, /destinationPath.*\/tree \(1\)\/&lt;copy&gt;/);
  assert.match(html, /renamedFiles 2/);
  const failed = {...task,state:'failed',directoryReceipt:null,
    error:{code:'publish',message:'partial',cleanupWarnings:[],destinationPath:'/tree (2)'}};
  const failureHtml = renderToStaticMarkup(React.createElement(render,{task:failed,label:k=>k}));
  assert.match(failureHtml, /destinationPath.*\/tree \(2\)/);
});
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
function production(file, name, context = {}) { const s = readFileSync(path.join(root, file), 'utf8'); const ast = ts.createSourceFile(file, s, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX); const fn = ast.statements.find(n => ts.isFunctionDeclaration(n) && n.name?.text === name); assert.ok(fn, name); return vm.runInNewContext(ts.transpileModule(fn.getText(ast).replace(/^export /, '') + '\n' + name + ';', { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.React } }).outputText, { React, isTransferRequest: request => "transfer" in request, ...context }); }
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
