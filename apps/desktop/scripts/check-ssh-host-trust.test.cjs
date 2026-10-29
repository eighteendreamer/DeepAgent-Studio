const assert = require('node:assert/strict');
const { test } = require('node:test');
const { readFileSync } = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const ts = require('typescript');
const React = require('react');
const { renderToStaticMarkup } = require('react-dom/server');
const zh = require('../src/locales/zh.json');
const root = path.resolve(__dirname, '../src');
const translate = key => key.split('.').reduce((value, part) => value?.[part], zh) ?? key;

// Execute actual production functions/callbacks instead of duplicating their logic.
function production(file, name, context = {}) {
  const source = readFileSync(path.join(root, file), 'utf8');
  const ast = ts.createSourceFile(file, source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  let found;
  function visit(node) {
    if ((ts.isFunctionDeclaration(node) || ts.isVariableDeclaration(node)) && node.name?.getText(ast) === name) found = node;
    ts.forEachChild(node, visit);
  }
  visit(ast);
  assert.ok(found, `${file}: ${name}`);
  let text = found.getText(ast).replace(/^export\s+/, '');
  if (ts.isVariableDeclaration(found)) text = `const ${text};`;
  const js = ts.transpileModule(`${text}\n${name};`, { compilerOptions: {
    module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.React,
  } }).outputText;
  return vm.runInNewContext(js, context);
}

test('SSH API sends and mock-projects the explicit trust path for create/update', async () => {
  for (const name of ['sshCreateConnection', 'sshUpdateConnection']) {
    const calls = [];
    const args = ['fixture', '127.0.0.1', 2222, 'fixture', 'password', undefined, 'fixture-only', 'C:\\fixture\\verified'];
    if (name.includes('Update')) args.unshift('id');
    const fn = production('api.ts', name, { getInvoke: () => async (command, input) => { calls.push({ command, input }); return input; } });
    await fn(...args);
    assert.equal(calls[0].input.knownHostsPath, args.at(-1));
    const mock = production('api.ts', name, { getInvoke: () => null });
    assert.equal((await mock(...args)).known_hosts_path, args.at(-1));
    args[args.length - 1] = '';
    await fn(...args);
    assert.equal(calls[1].input.knownHostsPath, '', 'explicit reset is not omitted');
  }
});

for (const file of ['components/settings/ConnectionsSettings.tsx', 'components/remote/RemoteSidebar.tsx']) {
  test(`${file}: both save branches forward trust, and edit restores it without a password`, async () => {
    const isRemote = file.includes('RemoteSidebar');
    for (const editing of [false, true]) {
      const calls = [];
      const form = { name: 'fixture', host: '127.0.0.1', port: '2222', username: 'fixture', authType: 'password', keyPath: '', password: 'temporary-only', knownHostsPath: 'C:\\fixture\\verified' };
      const noop = () => {};
      const fn = production(file, 'handleSave', {
        form, editingId: editing ? 'id' : null, managerMode: editing ? 'edit' : 'create', editingConn: { id: 'id' },
        sshCreateConnection: async (...args) => { calls.push(args); return { id: 'created' }; },
        sshUpdateConnection: async (...args) => { calls.push(args); },
        closeModal: noop, load: noop, loadConnections: noop, setForm: noop, setEditingConn: noop, setManagerMode: noop, handleSelect: noop, emptyForm: {},
        normalizeSshError: error => error, message: { error: error => { throw Error(error); } }, t: translate,
      });
      await fn();
      assert.equal(calls.length, 1);
      assert.equal(calls[0].at(-1), form.knownHostsPath);
    }
    let form;
    const edit = production(file, isRemote ? 'startEdit' : 'openEdit', {
      setForm: value => { form = value; }, setEditingId() {}, setIsModalOpen() {}, setTestResult() {}, setEditingConn() {}, setManagerMode() {},
    });
    edit({ id: 'id', name: 'fixture', host: '127.0.0.1', port: 2222, username: 'fixture', known_hosts_path: 'C:\\fixture\\verified' });
    assert.equal(form.knownHostsPath, 'C:\\fixture\\verified');
    assert.equal(form.password, '');
    assert.match(readFileSync(path.join(root, file), 'utf8'), /<SshHostTrustField[\s\S]*?value=\{form\.knownHostsPath\}/);
  });
}

test('host-key diagnostic precedes generic auth/permission normalization', () => {
  const normalize = production('components/settings/ConnectionsSettings.tsx', 'normalizeSshError');
  for (const suffix of ['unknown host', 'server key changed', 'known_hosts is unavailable (PermissionDenied)']) {
    const raw = `ssh host key verification failed: ${suffix}; C:\\fixture\\known_hosts`;
    const result = normalize(raw, translate);
    assert.ok(result.includes(raw));
    assert.ok(result.includes('指纹'));
    assert.notEqual(result, translate('settings.connections.errors.auth'));
  }
  assert.equal(normalize('authentication failed', translate), translate('settings.connections.errors.auth'));
});

test('shared production field renders accessible strict-trust guidance and edits its value', () => {
  const fn = production('components/remote/SshHostTrustField.tsx', 'SshHostTrustField', {
    React, useTranslation: () => ({ t: translate }), Input: 'input', Label: 'label',
  });
  let changed;
  const element = fn({ id: 'fixture-trust', value: 'C:\\fixture\\known_hosts', onChange: value => { changed = value; } });
  const html = renderToStaticMarkup(element);
  assert.match(html, /for="fixture-trust"/);
  assert.match(html, /aria-describedby="fixture-trust-help"/);
  assert.match(html, /不自动接受/);
  assert.match(html, /\[host\]:port/);
  element.props.children[1].props.onChange({ target: { value: 'C:\\fixture\\new' } });
  assert.equal(changed, 'C:\\fixture\\new');
});
