const { test } = require('node:test');
const assert = require('node:assert/strict');
const ts = require('typescript');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const file = path.resolve(__dirname, '../src/components/remote/filePanelModel.ts');
const mod = { exports: {} };
vm.runInNewContext(ts.transpileModule(fs.readFileSync(file, 'utf8'), { compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.CommonJS} }).outputText, {module:mod,exports:mod.exports});
const { endpointKey, panelStorageKey, visitDirectory, stepDirectory, joinEndpointPath, orderedEntries } = mod.exports;
test('both panel sides and remote identities have independent persistence scopes', () => {
  assert.notEqual(panelStorageKey('left',{kind:'ssh',connectionId:'a'}),panelStorageKey('right',{kind:'ssh',connectionId:'a'}));
  assert.notEqual(endpointKey({kind:'ssh',connectionId:'a'}),endpointKey({kind:'ssh',connectionId:'b'}));
});
test('history preserves forward navigation and drops future only after a new visit', () => {
  let history={paths:['/'],index:0};
  history=visitDirectory(history,'/a'); history=visitDirectory(history,'/a/b');
  history=stepDirectory(history,-1); assert.equal(history.paths[history.index],'/a');
  assert.equal(stepDirectory(history,1).paths[2],'/a/b');
  history=visitDirectory(history,'/c'); assert.deepEqual(Array.from(history.paths),['/','/a','/c']);
  for(let i=0;i<150;i++) history=visitDirectory(history,`/${i}`);
  assert.equal(history.paths.length,100); assert.equal(history.index,99);
});
test('POSIX paths preserve backslashes while Windows filenames reject drive syntax', () => {
  assert.equal(joinEndpointPath({kind:'ssh',connectionId:'a'},'/home','中文 \\ file'),'/home/中文 \\ file');
  assert.equal(joinEndpointPath({kind:'local'},'D:\\code','file'),'D:\\code\\file');
  for(const name of ['..','a/b','D:bad','a\\b']) assert.throws(()=>joinEndpointPath({kind:'local'},'D:\\',name));
});
test('sorting compares exact u64 decimal sizes without mutating the source', () => {
  const entries=[{name:'a',isDir:false,sizeBytes:'18446744073709551614'}, {name:'b',isDir:false,sizeBytes:'18446744073709551615'}];
  assert.equal(orderedEntries(entries,'','size')[0].name,'b'); assert.equal(entries[0].name,'a');
});
