const assert = require('node:assert/strict');
const { test } = require('node:test');
const { readFileSync } = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const ts = require('typescript');

function reader() {
  const source = readFileSync(path.join(__dirname, '../src/components/remote/RemoteSidebar.tsx'), 'utf8');
  const ast = ts.createSourceFile('sidebar.tsx', source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  let declaration;
  function visit(n) { if (ts.isVariableDeclaration(n) && n.name.getText(ast) === 'loadDir') declaration = n; ts.forEachChild(n, visit); }
  visit(ast); assert.ok(declaration);
  const replies = []; const writes = []; let tree = {};
  const scope = { current: { connectionId: 'a', requests: new Map() } };
  const selected = { current: {id:'a'} };
  const load = vm.runInNewContext(ts.transpileModule('const ' + declaration.getText(ast) + '; loadDir;', {compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText, {
    directoryScope:scope,selectedRef:selected,
    sshListDir: () => new Promise((resolve,reject) => replies.push({resolve,reject})),
    setLoadingDir: () => {}, setDirError: e => {if(e) writes.push(e);},
    setTree: update => {tree=update(tree);writes.push(tree);}, normalizeSshError:e=>e,t:k=>k,
  });
  return { load, replies, writes, scope, selected, tree:()=>tree };
}

test('late manual listing cannot overwrite a newer upload completion refresh', async () => {
  const h=reader(); const old=h.load('a','/'); const fresh=h.load('a','/');
  h.replies[1].resolve({entries:['published']}); await fresh;
  h.replies[0].resolve({entries:['old']}); await old;
  assert.deepEqual(Array.from(h.tree()['/']), ['published']);
});
test('connection replacement and unmount fence late listings and errors', async () => {
  const h=reader(); const old=h.load('a','/');
  h.scope.current={connectionId:'b',requests:new Map()}; h.selected.current={id:'b'};
  h.replies[0].resolve({entries:['wrong-server']}); await old;
  assert.equal(h.writes.length,0);
  const pending=h.load('b','/'); h.scope.current.requests.clear();
  h.replies[1].reject(Error('late error')); await pending;
  assert.equal(h.writes.length,0);
});

test('first render after selecting another host hides the previous directory state', () => {
  const source = readFileSync(path.join(__dirname, '../src/components/remote/RemoteSidebar.tsx'), 'utf8');
  const ast = ts.createSourceFile('sidebar.tsx', source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  let tree;
  function visit(n) {
    if (ts.isVariableDeclaration(n) && n.name.getText(ast) === 'tree') tree = n;
    ts.forEachChild(n, visit);
  }
  visit(ast); assert.ok(tree, 'tree view must be scoped to its owning host before paint');
  const expression = ts.transpileModule('const ' + tree.getText(ast) + '; tree;', {compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText;
  const previous = {connectionId:'a',entries:{'/':['old-host']}};
  assert.equal(Object.keys(vm.runInNewContext(expression,{directoryTree:previous,selected:{id:'b'}})).length,0);
  assert.deepEqual(Array.from(vm.runInNewContext(expression,{directoryTree:previous,selected:{id:'a'}})['/']),['old-host']);
});

test('selecting a connected host reloads its root instead of waiting for a transfer notification', () => {
  const source = readFileSync(path.join(__dirname, '../src/components/remote/RemoteSidebar.tsx'), 'utf8');
  const ast = ts.createSourceFile('sidebar.tsx', source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const reloads=[]; const effects=[];
  function visit(n){if(ts.isCallExpression(n)&&n.expression.getText(ast)==='useEffect'&&n.arguments[0]?.getText(ast).includes('createUploadInvalidator'))effects.push(n.arguments[0]);ts.forEachChild(n,visit);}visit(ast);
  assert.equal(effects.length,1);
  const code=ts.transpileModule('const effect='+effects[0].getText(ast)+'; effect;', {compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText;
  const effect=vm.runInNewContext(code,{selected:{id:'b',status:'connected'},loadDir:(...args)=>reloads.push(args),createUploadInvalidator:()=>()=>{},observeRemoteTransfers:()=>()=>{},setDirError:()=>{}});
  effect(); assert.deepEqual(reloads,[['b','/']]);
});
