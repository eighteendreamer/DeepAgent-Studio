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

// Exercise the production callbacks, including both connection-editor entry points.
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
  let sourceCode = found.getText(ast).replace(/^export\s+/, '');
  if (ts.isVariableDeclaration(found)) sourceCode = `const ${sourceCode};`;
  return vm.runInNewContext(ts.transpileModule(`${sourceCode}\n${name};`, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.React },
  }).outputText, context);
}

for (const file of ['components/settings/ConnectionsSettings.tsx', 'components/remote/RemoteSidebar.tsx']) {
  for (const authType of ['agent', 'openssh_agent', 'pageant']) {
    test(`${file}: ${authType} survives create/edit without submitting stale credentials`, async () => {
      for (const editing of [false, true]) {
        const calls = [];
        const form = { name: 'fixture', host: '127.0.0.1', port: '2222', username: 'fixture', authType, keyPath: 'stale-key', password: 'stale-password', knownHostsPath: '', hostKeyPolicy: 'strict' };
        const noop = () => {};
        await production(file, 'handleSave', {
          form, editingId: editing ? 'id' : null, managerMode: editing ? 'edit' : 'create', editingConn: { id: 'id' },
          sshCreateConnection: async (...args) => { calls.push(args); return { id: 'created' }; },
          sshUpdateConnection: async (...args) => { calls.push(args); },
          closeModal: noop, load: noop, loadConnections: noop, setForm: noop, setEditingConn: noop, setManagerMode: noop, handleSelect: noop, emptyForm: {},
          normalizeSshError: error => error, message: { error: error => { throw Error(error); } }, t: key => key,
        })();
        const offset = editing ? 1 : 0;
        assert.equal(calls[0][4 + offset], authType);
        assert.equal(calls[0][5 + offset], undefined);
        assert.equal(calls[0][6 + offset], undefined);
      }
      let form;
      production(file, file.includes('RemoteSidebar') ? 'startEdit' : 'openEdit', {
        setForm: value => { form = value; }, setEditingId() {}, setIsModalOpen() {}, setTestResult() {}, setEditingConn() {}, setManagerMode() {},
      })({ id: 'id', name: 'fixture', host: '127.0.0.1', port: 22, username: 'fixture', auth_type: authType });
      assert.equal(form.authType, authType);
      assert.equal(form.password, '');
    });
  }
}

test('Agent inspection requires desktop IPC; browser preview must not claim availability', async () => {
  const inspect = production('api.ts', 'sshProbeAgent', { getInvoke: () => null });
  await assert.rejects(inspect('auto'), /desktop|桌面/i);
  const calls = [];
  const viaIpc = production('api.ts', 'sshProbeAgent', { getInvoke: () => async (...args) => { calls.push(args); return []; } });
  await viaIpc('pageant');
  assert.equal(calls[0][0], 'ssh_probe_agent');
  assert.equal(calls[0][1].provider, 'pageant');
});

test('shared Agent fields have readable initial labels, no private credential controls, and truthful states', () => {
  const translate = key => key.split('.').reduce((value, part) => value?.[part], zh) ?? key;
  for (const state of ['available', 'empty', 'unavailable', 'unsupported', 'timed_out', 'error']) {
    let index = 0;
    const states = [0, { provider: 'pageant', refresh: 0, probes: [{ provider: 'pageant', state, identities: [] }] }];
    const passthrough = ({ children }) => React.createElement('div', null, children);
    const Fields = production('components/remote/SshAuthenticationFields.tsx', 'SshAuthenticationFields', {
      React, useId: () => 'fixture', useState: () => [states[index++], () => {}], useEffect() {}, useTranslation: () => ({ t: translate }),
      Select: passthrough, SelectTrigger: passthrough, SelectContent: passthrough, SelectItem: passthrough,
      SelectValue: ({ placeholder }) => React.createElement('span', null, placeholder ?? 'MISSING INITIAL LABEL'),
      Label: passthrough, Button: passthrough, Input: () => { throw Error('Agent must not render password/private key fields'); },
    });
    const html = renderToStaticMarkup(React.createElement(Fields, { value: { authType: 'pageant', keyPath: '', password: '' }, onChange() {}, onPickKeyFile() {} }));
    assert.ok(!html.includes('MISSING INITIAL LABEL'));
    assert.ok(html.includes(translate(`settings.connections.agent.states.${state}`)));
    assert.ok(!html.includes(translate('settings.connections.agent.help')));
    assert.ok(html.includes(translate('settings.connections.agent.refresh')));
  }
});
