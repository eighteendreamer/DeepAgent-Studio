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
    React, useTranslation: () => ({ t: translate }), Input: 'input', Label: 'label', Button: 'button',
    useRef: initial => ({ current: initial }), useState: initial => [initial, () => {}], useEffect() {},
  });
  let changed;
  const element = fn({ id: 'fixture-trust', host: 'fixture.invalid', port: 22, value: 'C:\\fixture\\known_hosts', onChange: value => { changed = value; } });
  const html = renderToStaticMarkup(element);
  assert.match(html, /for="fixture-trust"/);
  assert.match(html, /aria-describedby="fixture-trust-help"/);
  assert.match(html, /不自动接受/);
  assert.match(html, /\[host\]:port/);
  element.props.children[1].props.onChange({ target: { value: 'C:\\fixture\\new' } });
  assert.equal(changed, 'C:\\fixture\\new');
});

test('trust inspection API uses only endpoint/source and never fakes a browser success', async () => {
  const calls = [];
  const snapshot = { host: 'fixture.invalid', port: 2222, known_hosts_path: 'C:\\fixture\\verified', keys: [] };
  const inspect = production('api.ts', 'sshInspectHostTrust', {
    getInvoke: () => async (command, input) => { calls.push({ command, input }); return snapshot; },
  });
  assert.equal(await inspect('fixture.invalid', 2222, snapshot.known_hosts_path), snapshot);
  assert.equal(calls[0].command, 'ssh_inspect_host_trust');
  assert.deepEqual(JSON.parse(JSON.stringify(calls[0].input)), {
    host: 'fixture.invalid', port: 2222, knownHostsPath: snapshot.known_hosts_path,
  });
  await assert.rejects(production('api.ts', 'sshInspectHostTrust', { getInvoke: () => null })('fixture.invalid', 22), /desktop app/);
  const rust = readFileSync(path.join(root, '../src-tauri/src/lib.rs'), 'utf8');
  assert.match(rust, /#\[tauri::command\]\s*async fn ssh_inspect_host_trust\(/);
  assert.match(rust, /generate_handler!\[[\s\S]*?\bssh_inspect_host_trust,/);
});

// Minimal hook driver executes the actual component, not a copied request controller.
function trustFieldDriver(inspect) {
  const slots = [];
  let cursor = 0;
  let effects = [];
  const fn = production('components/remote/SshHostTrustField.tsx', 'SshHostTrustField', {
    React, useTranslation: () => ({ t: translate }), Input: 'input', Label: 'label', Button: 'button',
    sshInspectHostTrust: inspect,
    useRef: initial => { const at = cursor++; return slots[at] ??= { current: initial }; },
    useState: initial => {
      const at = cursor++;
      if (!(at in slots)) slots[at] = initial;
      return [slots[at], value => { slots[at] = typeof value === 'function' ? value(slots[at]) : value; }];
    },
    useEffect: (effect, deps) => {
      const at = cursor++;
      const previous = slots[at];
      if (!previous || deps.some((dep, index) => dep !== previous.deps[index])) {
        previous?.cleanup?.();
        effects.push(() => { slots[at] = { deps, cleanup: effect() }; });
      }
    },
  });
  return props => {
    cursor = 0;
    const element = fn(props);
    for (const effect of effects.splice(0)) effect();
    const flat = [];
    function visit(node) {
      if (!node || typeof node !== 'object') return;
      if (Array.isArray(node)) return node.forEach(visit);
      flat.push(node);
      visit(node.props?.children);
    }
    visit(element);
    return { html: renderToStaticMarkup(element), button: flat.find(node => node.type === 'button') };
  };
}

test('inspection UI reports pending/result/errors and prevents form submit or automatic networking', async () => {
  let complete;
  const calls = [];
  const render = trustFieldDriver((...args) => { calls.push(args); return new Promise(resolve => { complete = resolve; }); });
  const props = { id: 'trust', host: 'fixture.invalid', port: 2222, value: '', onChange() {} };
  let view = render(props);
  assert.ok(view.button, 'inspection button is visible');
  assert.equal(view.button.props.type, 'button');
  assert.equal(calls.length, 0, 'no automatic file read or network probe on render');
  const pending = view.button.props.onClick();
  view = render(props);
  assert.equal(view.button.props.disabled, true);
  assert.match(view.html, /检查中/);
  assert.equal(calls.length, 1);
  complete({ host: 'fixture.invalid', port: 2222, known_hosts_path: 'C:\\fixture\\verified', keys: [{ line_number: 3, algorithm: 'ssh-ed25519', fingerprint: 'SHA256:fixture-only' }] });
  await pending;
  view = render(props);
  assert.match(view.html, /SHA256:fixture-only/);
  assert.match(view.html, /ssh-ed25519/);
  assert.match(view.html, /不代表远端服务器已通过校验/);
  assert.match(view.html, /role="status"/);
});

test('inspection UI rejects stale responses after endpoint/source edits and disables invalid ports', async () => {
  let complete;
  const render = trustFieldDriver(() => new Promise(resolve => { complete = resolve; }));
  const props = { id: 'trust', host: 'old.invalid', port: 22, value: '', onChange() {} };
  render(props);
  const pending = render(props).button.props.onClick();
  const next = { ...props, host: 'new.invalid', value: 'C:\\new' };
  render(next);
  complete({ host: props.host, port: 22, known_hosts_path: 'C:\\old', keys: [{ algorithm: 'ssh-ed25519', fingerprint: 'SHA256:stale', line_number: 1 }] });
  await pending;
  assert.doesNotMatch(render(next).html, /SHA256:stale/);
  for (const port of [0, NaN, 22.5, 65536]) assert.equal(render({ ...next, port }).button.props.disabled, true);
  assert.equal(render({ ...next, host: '   ' }).button.props.disabled, true);
});

test('inspection UI exposes failures and keeps the inputs editable while waiting', async () => {
  const render = trustFieldDriver(async () => { throw new Error('fixture trust unreadable'); });
  const props = { id: 'trust', host: 'fixture.invalid', port: 22, value: '', onChange() {} };
  render(props);
  await render(props).button.props.onClick();
  const view = render(props);
  assert.match(view.html, /fixture trust unreadable/);
  assert.match(view.html, /role="alert"/);
  assert.equal(view.button.props.disabled, false);
  for (const file of ['components/settings/ConnectionsSettings.tsx', 'components/remote/RemoteSidebar.tsx']) {
    const source = readFileSync(path.join(root, file), 'utf8');
    const field = source.match(/<SshHostTrustField[\s\S]*?\/>/)?.[0] ?? '';
    assert.match(field, /host=\{form\.host\}/, file);
    assert.match(field, /port=\{Number\(form\.port\)\}/, file);
  }
});

test('completed fingerprints disappear on the first render after editing the endpoint', async () => {
  const render = trustFieldDriver(async () => ({ host: 'old.invalid', port: 22, known_hosts_path: 'C:\\old', keys: [{ algorithm: 'ssh-ed25519', fingerprint: 'SHA256:old-result', line_number: 1 }] }));
  const props = { id: 'trust', host: 'old.invalid', port: 22, value: '', onChange() {} };
  render(props);
  await render(props).button.props.onClick();
  assert.match(render(props).html, /SHA256:old-result/);
  assert.doesNotMatch(render({ ...props, host: 'new.invalid' }).html, /SHA256:old-result/);
});

test('inspection ignores repeated clicks while a read is pending', async () => {
  let complete;
  let calls = 0;
  const render = trustFieldDriver(() => { calls++; return new Promise(resolve => { complete = resolve; }); });
  const props = { id: 'trust', host: 'fixture.invalid', port: 22, value: '', onChange() {} };
  render(props);
  const onClick = render(props).button.props.onClick;
  const pending = onClick();
  await onClick();
  assert.equal(calls, 1);
  complete({ host: props.host, port: 22, known_hosts_path: 'C:\\trusted', keys: [] });
  await pending;
});
