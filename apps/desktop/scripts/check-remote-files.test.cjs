const assert = require('node:assert/strict');
const { test } = require('node:test');
const { readFileSync } = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const ts = require('typescript');
const React = require('react');
const { renderToStaticMarkup } = require('react-dom/server');
const root = path.resolve(__dirname, '..');

function production(name, context = {}, file = 'src/api.ts') {
  const source = readFileSync(path.join(root, file), 'utf8');
  const ast = ts.createSourceFile(file, source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const fn = ast.statements.find(n => ts.isFunctionDeclaration(n) && n.name?.text === name);
  assert.ok(fn, `missing production function ${name}`);
  const text = ts.transpileModule(fn.getText(ast).replace(/^export /, '') + '\n' + name + ';', {
    compilerOptions: { target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.React, module: ts.ModuleKind.CommonJS },
  }).outputText;
  return vm.runInNewContext(text, { React, ...context });
}

test('file metadata defaults to lstat, explicit stat preserves scope, and realpath is independent', async () => {
  const calls = []; const result = { connection_id: 'a', path: '/link', kind: 'symlink', size_bytes: '18446744073709551615', followed_symlinks: false };
  const invoke = async (cmd, args) => { calls.push({ cmd, args }); return result; };
  const metadata = production('sshFileMetadata', { getInvoke: () => invoke });
  assert.equal(await metadata('a', '/link'), result);
  assert.equal(calls[0].cmd, 'ssh_file_metadata');
  assert.equal(calls[0].args.followSymlinks, false);
  await metadata('a', '/link', true);
  assert.equal(calls[1].args.followSymlinks, true);
  await production('sshRealpath', { getInvoke: () => invoke })('a', './relative');
  assert.equal(calls[2].cmd, 'ssh_realpath');
  assert.equal(calls[2].args.path, './relative');
  const rust = readFileSync(path.join(root, 'src-tauri/src/lib.rs'), 'utf8');
  for (const cmd of ['ssh_file_metadata', 'ssh_realpath']) {
    assert.ok(new RegExp(`async fn ${cmd}\\(`).test(rust), `${cmd} command definition`);
    assert.ok(new RegExp(`generate_handler!\\[[\\s\\S]*?\\b${cmd},`).test(rust), `${cmd} command registration`);
  }
});

test('missing transport and server failures are not reported as empty successful metadata', async () => {
  for (const name of ['sshFileMetadata', 'sshRealpath']) {
    await assert.rejects(production(name, { getInvoke: () => null })('fixture', '/file'), /desktop app/);
    const error = Error('SFTP permission denied');
    await assert.rejects(production(name, { getInvoke: () => async () => { throw error; } })('fixture', '/file'), e => e === error);
  }
});

test('properties show exact large size, octal permissions, numeric owner and explicit unknown fields', () => {
  const body = production('RemoteFilePropertiesBody', {}, 'src/components/remote/RemoteFileProperties.tsx');
  const label = key => key;
  const html = renderToStaticMarkup(React.createElement(body, {
    metadata: { connection_id: 'fixture', path: '/file', kind: 'file', size_bytes: '18446744073709551615', permissions: 0o100640, uid: 1001, gid: 1002, modified_ms: null, user: null, group: null, followed_symlinks: false },
    canonicalPath: null, label,
  }));
  assert.match(html, /18446744073709551615/);
  assert.match(html, /0640/);
  assert.match(html, /1001/); assert.match(html, /1002/);
  assert.match(html, /unknown/);
  assert.doesNotMatch(html, /Invalid Date|NaN|18446744073709552000/);
  const absent = renderToStaticMarkup(React.createElement(body, { metadata: { connection_id:'fixture', path:'/unknown', kind:'unknown', followed_symlinks:false }, canonicalPath:null, label }));
  assert.match(absent, /kind.unknown/);
  assert.doesNotMatch(absent, />0 B<|>0000</);
});

test('response identity validation rejects metadata for another file or an implicit symlink follow', async () => {
  const file = 'src/components/remote/RemoteFileProperties.tsx';
  for (const value of [
    { connection_id:'other',path:'/file',followed_symlinks:false },
    { connection_id:'fixture',path:'/other',followed_symlinks:false },
    { connection_id:'fixture',path:'/file',followed_symlinks:true },
  ]) {
    await assert.rejects(production('loadRemoteFileProperties', { sshFileMetadata: async () => value }, file)('fixture','/file'), /identity/);
  }
});

test('remote file context menu slots target native buttons, not a tooltip wrapper that discards slot events', () => {
  const file = 'src/components/remote/RemoteSidebar.tsx';
  const source = readFileSync(path.join(root, file), 'utf8');
  const ast = ts.createSourceFile(file, source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  let found = 0;
  function visit(node) {
    if (ts.isJsxElement(node) && node.openingElement.tagName.getText(ast) === 'ContextMenuTrigger') {
      const child = node.children.find(ts.isJsxElement);
      if (child && child.openingElement.tagName.getText(ast) === 'HoverInfo') {
        assert.fail('Radix asChild cannot pass contextmenu/ref through HoverInfo: it does not forward those props');
      }
      if (child?.openingElement.tagName.getText(ast) === 'button') found++;
    }
    ts.forEachChild(node, visit);
  }
  visit(ast);
  assert.ok(found >= 2, 'both file and directory rows must retain the native context-menu trigger');
});

test('closing properties fences an in-flight metadata response and schedules no polling', async () => {
  const file = 'src/components/remote/RemoteFileProperties.tsx';
  const source = readFileSync(path.join(root, file), 'utf8');
  const effects = []; const updates = []; let reply; let requests = 0;
  const pending = new Promise(resolve => { reply = resolve; });
  const hooks = { ...React, useEffect: effect => effects.push(effect), useRef: value => ({ current:value }),
    useState: value => [value, next => updates.push(next)] };
  const api = { sshFileMetadata: async () => { requests++; return pending; } };
  const module = { exports:{} };
  vm.runInNewContext(ts.transpileModule(source, { compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.CommonJS,jsx:ts.JsxEmit.ReactJSX} }).outputText, {
    exports:module.exports,module,
    require: name => name === 'react' ? hooks : name === 'react/jsx-runtime' ? require('react/jsx-runtime') : name === 'react-i18next' ? {useTranslation:()=>({t:k=>k})} : name.includes('api') ? api : new Proxy({}, {get:()=>()=>null}),
  });
  module.exports.RemoteFileProperties({connectionId:'fixture',path:'/old',onClose:()=>{}});
  assert.equal(effects.length,1);
  const cleanup = effects[0]();
  assert.equal(requests,1);
  cleanup(); const before=updates.length;
  reply({connection_id:'fixture',path:'/old',followed_symlinks:false});
  await new Promise(setImmediate);
  assert.equal(updates.length,before,'late promise cannot set state after cleanup');
  assert.equal(requests,1,'properties never poll or auto-follow links');
});
