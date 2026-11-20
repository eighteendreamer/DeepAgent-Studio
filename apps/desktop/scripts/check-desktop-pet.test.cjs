const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
const test = require('node:test');
const ts = require('typescript');
const root = path.join(__dirname, '../src-tauri/resources/pets/dsh-pet');
const catalog = require(path.join(root, 'catalog.json'));
const character = catalog.characters[0];

test('the original character, videos, and previews ship with the catalog', () => {
  const files = new Set(catalog.files.map(item => item.path));
  assert.ok(files.has(character.thumbnail));
  assert.ok(files.has(character.preview));
  assert.equal(Object.keys(character.actions).length, 114);
  for (const action of Object.values(character.actions)) {
    assert.ok(files.has(action.file));
    assert.ok(!action.preview || files.has(action.preview));
    assert.equal(action.width, character.canvas.width);
    assert.equal(action.height, character.canvas.height);
  }
  for (const file of catalog.files) {
    const data = fs.readFileSync(path.join(root, file.path));
    assert.equal(data.length, file.bytes, file.path);
    assert.equal(crypto.createHash('sha256').update(data).digest('hex'), file.sha256, file.path);
  }
  for (const { actions } of [...character.groups, ...Object.values(character.states)]) {
    for (const action of actions) assert.ok(character.actions[action], action);
  }
});

const filename = path.join(__dirname, '../src/pet/api.ts');
const output = ts.transpileModule(fs.readFileSync(filename, 'utf8'), {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020 },
}).outputText;
const mockModule = { exports: {} };
new Function('require', 'exports', output)(() => ({ isTauri: () => false }), mockModule.exports);
const { resolveAction, workState } = mockModule.exports;

test('state mapping preserves approvals and multiple active runs', () => {
  assert.equal(workState({ activeRuns: 2, pendingApprovals: 1, runningActions: 1 }), 'waiting');
  assert.equal(workState({ activeRuns: 2, pendingApprovals: 0, runningActions: 1 }), 'working');
  assert.equal(workState({ activeRuns: 1, pendingApprovals: 0, runningActions: 0 }), 'thinking');
  assert.equal(workState({ activeRuns: 0, pendingApprovals: 0, runningActions: 0 }), 'idle');
});

test('missing or unplayable animations fall back to an available loop', () => {
  assert.equal(resolveAction(character, 'success').loop, false);
  assert.equal(resolveAction(character, 'thinking').loop, true);
  assert.notEqual(resolveAction(character, 'click', 0).name, resolveAction(character, 'click', 1).name);
  const result = structuredClone(character);
  delete result.actions[result.states.result.actions[0]];
  assert.equal(resolveAction(result, 'result').name, result.states.working.actions[0]);
  const failed = result.states.working.actions[0];
  assert.equal(resolveAction(result, 'result', 0, failed).name, result.states.idle.actions[0]);
  assert.equal(resolveAction(result, 'idle', 0, result.states.idle.actions[0]), null);
});
