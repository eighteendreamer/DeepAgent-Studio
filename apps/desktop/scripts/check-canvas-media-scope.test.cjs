const assert = require('node:assert/strict');
const { test } = require('node:test');
const { readFileSync } = require('node:fs');
const path = require('node:path');
const Module = require('node:module');
const ts = require('typescript');
function fixture(invoke) {
  const filename = path.resolve(__dirname, '../src/canvas/workflow/utils/canvasMedia.ts');
  const source = ts.transpileModule(readFileSync(filename, 'utf8'), { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 }, fileName: filename }).outputText;
  const loaded = new Module(filename, module); loaded.filename = filename; loaded.paths = Module._nodeModulePaths(path.dirname(filename));
  const original = loaded.require.bind(loaded);
  let state, previousDeps, scheduled, cleanup;
  const hooks = {
    useState(initial) { if (state === undefined) state = typeof initial === 'function' ? initial() : initial; return [state, value => { state = value; }]; },
    useEffect(effect, deps) { if (!previousDeps || deps.some((value, i) => value !== previousDeps[i])) { scheduled = effect; previousDeps = deps; } },
  };
  loaded.require = name => name === '@tauri-apps/api/core' ? { invoke, convertFileSrc: value => `asset:${value}` } : name === 'react' ? hooks : original(name);
  loaded._compile(source, filename);
  return { ...loaded.exports, effect() { cleanup?.(); cleanup = scheduled?.(); scheduled = undefined; } };
}
const tick = () => new Promise(resolve => setImmediate(resolve));

test('cleared references keep the legacy undefined contract without stale media', async () => {
  const calls = [];
  const f = fixture(async (_command, args) => { calls.push(args); return { path: 'owned.png' }; });
  assert.equal(f.useCanvasMediaSrc('', 'ws-a'), undefined);
  assert.equal(f.useCanvasMediaSrc(undefined, 'ws-a'), undefined);
  assert.equal(calls.length, 0);
  f.useCanvasMediaSrc('artifact://owned', 'ws-a'); f.effect(); await tick();
  assert.equal(f.useCanvasMediaSrc('artifact://owned', 'ws-a'), 'asset:owned.png');
  assert.equal(f.useCanvasMediaSrc('', 'ws-a'), undefined);
  assert.equal(f.useCanvasMediaSrc('https://synthetic.invalid/image.png', 'ws-a'), 'https://synthetic.invalid/image.png');
  assert.equal(calls.length, 1);
});
test('display cache and in-flight reads are partitioned by explicit workspace', async () => {
  const calls = [], releases = [];
  const f = fixture((command, args) => { calls.push({ command, ...args }); return new Promise(resolve => releases.push(() => resolve({ path: `${args.workspaceId}/image.png` }))); });
  const a = f.resolveCanvasMediaSrc('artifact://same', 'ws-a');
  const b = f.resolveCanvasMediaSrc('artifact://same', 'ws-b');
  assert.notEqual(a, b);
  assert.deepEqual(calls.map(call => call.workspaceId), ['ws-a', 'ws-b']);
  releases.forEach(release => release());
  assert.deepEqual(await Promise.all([a, b]), ['asset:ws-a/image.png', 'asset:ws-b/image.png']);
  assert.equal(await f.resolveCanvasMediaSrc('artifact://same', 'ws-a'), 'asset:ws-a/image.png');
  assert.equal(calls.length, 2);
});
test('default scope is explicit and denied media never reuses another scope cache', async () => {
  const calls = [];
  const f = fixture(async (_command, args) => { calls.push(args); if (args.workspaceId === 'ws-b') throw Error('unknown artifact'); return { path: 'default.png' }; });
  assert.equal(await f.resolveCanvasMediaSrc('artifact://same'), 'asset:default.png');
  assert.equal(calls[0].workspaceId, null);
  await assert.rejects(f.resolveCanvasMediaSrc('artifact://same', 'ws-b'), /unknown artifact/);
  assert.equal(calls.length, 2);
});
test('pixel reads carry scope and do not turn a missing artifact into a browser reference', async () => {
  const calls = []; const f = fixture(async (command, args) => { calls.push({ command, ...args }); return null; });
  await assert.rejects(f.loadCanvasMediaPixels('artifact://missing', 'ws-a'), /artifact|制品/);
  assert.equal(calls[0].workspaceId, 'ws-a');
  assert.equal(await f.loadCanvasMediaPixels('https://synthetic.invalid/image.png'), 'https://synthetic.invalid/image.png');
  assert.equal(calls[1].workspaceId, null);
});
test('hook does not render the previous workspace URL before the next effect runs', async () => {
  const f = fixture(async (_command, args) => ({ path: `${args.workspaceId}/image.png` }));
  assert.equal(f.useCanvasMediaSrc('artifact://same', 'ws-a'), undefined); f.effect(); await tick();
  assert.equal(f.useCanvasMediaSrc('artifact://same', 'ws-a'), 'asset:ws-a/image.png');
  assert.equal(f.useCanvasMediaSrc('artifact://same', 'ws-b'), undefined);
  f.effect(); await tick();
  assert.equal(f.useCanvasMediaSrc('artifact://same', 'ws-b'), 'asset:ws-b/image.png');
});
