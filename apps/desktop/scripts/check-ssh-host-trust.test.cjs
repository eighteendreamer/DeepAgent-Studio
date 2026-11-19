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
  return vm.runInNewContext(js, {
    canSaveSshConnection: value => production('components/remote/SshAuthenticationFields.tsx', 'canSaveSshConnection')(value),
    canSaveSshJump: value => production('components/remote/SshJumpFields.tsx', 'canSaveSshJump')(value),
    authenticationFactorRequests: value => production('components/remote/SshAuthenticationFields.tsx', 'authenticationFactorRequests')(value),
    authenticationFactorsFromDto: value => production('components/remote/SshAuthenticationFields.tsx', 'authenticationFactorsFromDto')(value),
    mockAuthenticationFactors: value => production('api.ts', 'mockAuthenticationFactors')(value),
    TextEncoder,
    canSaveSshProxy: value => production('components/remote/SshProxyFields.tsx', 'canSaveSshProxy')(value),
    sshProxyRequest: value => production('components/remote/SshProxyFields.tsx', 'sshProxyRequest')(value),
    sshProxyFromDto: value => production('components/remote/SshProxyFields.tsx', 'sshProxyFromDto')(value),
    sshJumpRequest: value => production('components/remote/SshJumpFields.tsx', 'sshJumpRequest')(value),
    sshJumpFromDto: value => production('components/remote/SshJumpFields.tsx', 'sshJumpFromDto')(value),
    canSaveSshTiming: value => production('components/remote/SshTimingFields.tsx', 'canSaveSshTiming')(value),
    sshTimingRequest: value => production('components/remote/SshTimingFields.tsx', 'sshTimingRequest')(value),
    sshTimingFromDto: value => production('components/remote/SshTimingFields.tsx', 'sshTimingFromDto')(value),
    matchesSshRoute: (...args) => production('components/remote/SshProxyFields.tsx', 'matchesSshRoute')(...args),
    isSshRoute: value => production('components/remote/SshProxyFields.tsx', 'isSshRoute')(value),
    mockProxy: value => production('api.ts', 'mockProxy')(value),
    mockJump: value => production('api.ts', 'mockJump')(value),
    ...context,
  });
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

test('SSH API carries the attached user certificate and explicit removal', async () => {
  for (const name of ['sshCreateConnection', 'sshUpdateConnection']) {
    for (const certificate of ['D:\\fixture\\identity-cert.pub', '']) {
      const args = ['fixture', '127.0.0.1', 2222, 'fixture', 'key_file', 'D:\\fixture\\identity', undefined, '', 'strict', certificate];
      if (name.includes('Update')) args.unshift('id');
      const calls = [];
      const fn = production('api.ts', name, { getInvoke: () => async (_command, input) => { calls.push(input); return input; } });
      await fn(...args);
      assert.equal(calls[0].certificatePath, certificate);
      const mock = production('api.ts', name, { getInvoke: () => null });
      assert.equal((await mock(...args)).certificate_path, certificate);
    }
  }
});

for (const file of ['components/settings/ConnectionsSettings.tsx', 'components/remote/RemoteSidebar.tsx']) {
  test(`${file}: save/edit preserves the certificate attachment and explicit removal`, async () => {
    for (const editing of [false, true]) {
      for (const certificatePath of ['D:\\fixture\\identity-cert.pub', '']) {
        const calls = [];
        const form = { name: 'fixture', host: '127.0.0.1', port: '2222', username: 'fixture', authType: 'file', keyPath: 'identity', certificatePath, password: '', knownHostsPath: '', hostKeyPolicy: 'strict' };
        const noop = () => {};
        await production(file, 'handleSave', {
          form, editingId: editing ? 'id' : null, managerMode: editing ? 'edit' : 'create', editingConn: { id: 'id' },
          sshCreateConnection: async (...args) => { calls.push(args); return { id: 'created' }; }, sshUpdateConnection: async (...args) => { calls.push(args); },
          closeModal: noop, load: noop, loadConnections: noop, setForm: noop, setEditingConn: noop, setManagerMode: noop, handleSelect: noop, emptyForm: {},
          normalizeSshError: error => error, message: { error: error => { throw Error(error); } }, t: translate,
        })();
        assert.equal(calls[0][editing ? 10 : 9], certificatePath);
      }
    }
    let form;
    production(file, file.includes('RemoteSidebar') ? 'startEdit' : 'openEdit', {
      setForm: value => { form = value; }, setEditingId() {}, setIsModalOpen() {}, setTestResult() {}, setEditingConn() {}, setManagerMode() {},
    })({ id: 'id', name: 'fixture', host: '127.0.0.1', port: 22, username: 'fixture', auth_type: 'key_file', key_path: 'identity', certificate_path: 'identity-cert.pub' });
    assert.equal(form.certificatePath, 'identity-cert.pub');
    assert.match(readFileSync(path.join(root, file), 'utf8'), /onPickCertificateFile=/);
  });
}

test('SSH API carries explicit AcceptNew and legacy create defaults to Strict', async () => {
  for (const name of ['sshCreateConnection', 'sshUpdateConnection']) {
    const calls = [];
    const args = ['fixture', '127.0.0.1', 2222, 'fixture', 'password', undefined, 'fixture-only', '', 'accept_new'];
    if (name.includes('Update')) args.unshift('id');
    const fn = production('api.ts', name, { getInvoke: () => async (_command, input) => { calls.push(input); return input; } });
    await fn(...args);
    assert.equal(calls[0].hostKeyPolicy, 'accept_new');
    const mock = production('api.ts', name, { getInvoke: () => null });
    assert.equal((await mock(...args)).host_key_policy, 'accept_new');
    args.pop();
    await fn(...args);
    assert.equal(calls[1].hostKeyPolicy, name.includes('Create') ? 'strict' : undefined);
  }
});

for (const file of ['components/settings/ConnectionsSettings.tsx', 'components/remote/RemoteSidebar.tsx']) {
  test(`${file}: both save branches and edit preserve the selected host-key policy`, async () => {
    for (const editing of [false, true]) {
      const calls = [];
      const form = { name: 'fixture', host: '127.0.0.1', port: '2222', username: 'fixture', authType: 'password', keyPath: '', password: 'temporary-only', knownHostsPath: '', hostKeyPolicy: 'accept_new' };
      const noop = () => {};
      await production(file, 'handleSave', {
        form, editingId: editing ? 'id' : null, managerMode: editing ? 'edit' : 'create', editingConn: { id: 'id' },
        sshCreateConnection: async (...args) => { calls.push(args); return { id: 'created' }; },
        sshUpdateConnection: async (...args) => { calls.push(args); },
        closeModal: noop, load: noop, loadConnections: noop, setForm: noop, setEditingConn: noop, setManagerMode: noop, handleSelect: noop, emptyForm: {},
        normalizeSshError: error => error, message: { error: error => { throw Error(error); } }, t: translate,
      })();
      assert.equal(calls[0][editing ? 9 : 8], 'accept_new');
    }
    let form;
    const edit = production(file, file.includes('RemoteSidebar') ? 'startEdit' : 'openEdit', {
      setForm: value => { form = value; }, setEditingId() {}, setIsModalOpen() {}, setTestResult() {}, setEditingConn() {}, setManagerMode() {},
    });
    edit({ id: 'id', name: 'fixture', host: '127.0.0.1', port: 22, username: 'fixture', host_key_policy: 'accept_new' });
    assert.equal(form.hostKeyPolicy, 'accept_new');
    edit({ id: 'legacy', name: 'fixture', host: '127.0.0.1', port: 22, username: 'fixture' });
    assert.equal(form.hostKeyPolicy, 'strict');
    assert.match(readFileSync(path.join(root, file), 'utf8'), /policy=\{form\.hostKeyPolicy\}/);
  });
}

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
      assert.equal(calls[0][editing ? 8 : 7], form.knownHostsPath);
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

test('host-key error formatting is idempotent across test results and status rendering', () => {
  const normalize = production('components/settings/ConnectionsSettings.tsx', 'normalizeSshError');
  const raw = 'ssh host key verification failed: presented ssh-ed25519 SHA256:fixture-only';
  for (const t of [translate, key => key === 'settings.connections.hostKeyError' ? 'Verify the host fingerprint.' : key]) {
    const once = normalize(raw, t);
    assert.equal(normalize(once, t), once, 'rendering an already formatted failure must not repeat guidance');
    assert.ok(once.includes('SHA256:fixture-only'), 'public diagnostic details remain visible');
  }
});

test('shared production field keeps its accessible label and editing without explanatory copy', () => {
  const fn = production('components/remote/SshHostTrustField.tsx', 'SshHostTrustField', {
    React, useTranslation: () => ({ t: translate }), Input: 'input', Label: 'label', Button: 'button',
    useRef: initial => ({ current: initial }), useState: initial => [initial, () => {}], useEffect() {},
  });
  let changed;
  const element = fn({ id: 'fixture-trust', host: 'fixture.invalid', port: 22, value: 'C:\\fixture\\known_hosts', onChange: value => { changed = value; } });
  const html = renderToStaticMarkup(element);
  assert.match(html, /for="fixture-trust"/);
  assert.doesNotMatch(html, /aria-describedby/);
  assert.ok(!html.includes(translate('settings.connections.knownHostsHelp')));
  element.props.children[1].props.onChange({ target: { value: 'C:\\fixture\\new' } });
  assert.equal(changed, 'C:\\fixture\\new');
});

test('shared policy selector omits explanatory copy, refuses invalid values, and switches back to Strict', () => {
  const Select = ({ children }) => React.createElement('div', null, children);
  const fn = production('components/remote/SshHostTrustField.tsx', 'SshHostTrustField', {
    React, useTranslation: () => ({ t: translate }), Input: 'input', Label: 'label', Button: 'button',
    Select, SelectTrigger: 'button', SelectValue: ({ placeholder }) => React.createElement('span', null, placeholder), SelectContent: 'div', SelectItem: 'div',
    useRef: initial => ({ current: initial }), useState: initial => [initial, () => {}], useEffect() {},
  });
  const changed = [];
  const element = fn({ id: 'policy-fixture', host: 'fixture.invalid', port: 22, value: '', onChange() {}, policy: 'accept_new', onPolicyChange: value => changed.push(value) });
  const html = renderToStaticMarkup(element);
  assert.ok(!html.includes(translate('settings.connections.acceptNewHelp')));
  assert.ok(!html.includes(translate('settings.connections.acceptNewFileHelp')));
  assert.doesNotMatch(html, /aria-describedby/);
  const find = node => node && typeof node === 'object' && (node.type === Select ? node : React.Children.toArray(node.props?.children).map(find).find(Boolean));
  const select = find(element);
  assert.ok(select, 'use the existing shadcn Select');
  assert.equal(select.props.value, 'accept_new');
  select.props.onValueChange('insecure');
  assert.deepEqual(changed, []);
  select.props.onValueChange('strict');
  assert.deepEqual(changed, ['strict']);
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
  assert.doesNotMatch(view.html, /不代表远端服务器已通过校验/);
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

test('application host trust API preserves the reviewed snapshot and never fakes browser writes', async () => {
  const expected = { host: 'fixture.invalid', port: 22, known_hosts_path: 'C:\\fixture', keys: [] };
  for (const [name, command, args, payload] of [
    ['sshListHostKeys', 'ssh_list_host_keys', ['id'], { id: 'id' }],
    ['sshImportHostTrust', 'ssh_import_host_trust', ['id', expected], { id: 'id', expected }],
    ['sshRevokeHostKey', 'ssh_revoke_host_key', ['id', 'SHA256:fixture'], { id: 'id', fingerprint: 'SHA256:fixture' }],
  ]) {
    const calls = [];
    await production('api.ts', name, { getInvoke: () => async (command, args) => calls.push({ command, args }) })(...args);
    assert.equal(calls[0].command, command);
    assert.deepEqual(JSON.parse(JSON.stringify(calls[0].args)), payload);
    await assert.rejects(production('api.ts', name, { getInvoke: () => null })(...args), /desktop app/);
    const rust = readFileSync(path.join(root, '../src-tauri/src/lib.rs'), 'utf8');
    assert.match(rust, new RegExp(`#\\[tauri::command\\]\\s*async fn ${command}\\(`));
    assert.match(rust, new RegExp(`generate_handler!\\[[\\s\\S]*?\\b${command},`));
  }
});

function trustManagerDriver(api = {}, file = 'components/remote/SshHostTrustManager.tsx', name = 'HostTrustPanel', extra = {}) {
  const slots = [];
  let cursor = 0;
  let effects = [];
  const names = ['Dialog', 'DialogContent', 'DialogHeader', 'DialogTitle', 'DialogDescription', 'DialogFooter'];
  const context = Object.fromEntries(names.map(name => [name, 'div']));
  const fn = production(file, name, {
    ...context, React, Button: 'button', Checkbox: 'fixture-checkbox', Label: 'label',
    SshRouteLabel: production('components/remote/SshProxyFields.tsx', 'SshRouteLabel', { React, useTranslation: () => ({ t: translate }) }),
    useTranslation: () => ({ t: translate }),
    sshListHostKeys: async () => [], sshInspectHostTrust: async () => { throw Error('unexpected inspection'); },
    sshImportHostTrust: async () => { throw Error('unexpected import'); },
    sshRevokeHostKey: async () => { throw Error('unexpected revoke'); }, ...api,
    useRef: initial => { const at = cursor++; return slots[at] ??= { current: initial }; },
    useState: initial => {
      const at = cursor++;
      if (!(at in slots)) slots[at] = typeof initial === 'function' ? initial() : initial;
      return [slots[at], value => { slots[at] = typeof value === 'function' ? value(slots[at]) : value; }];
    }, ...extra,
    useEffect: (effect, deps) => {
      const at = cursor++;
      const previous = slots[at];
      if (!previous || deps.some((dep, index) => dep !== previous.deps[index])) {
        previous?.cleanup?.();
        effects.push(() => { slots[at] = { deps, cleanup: effect(), effect }; });
      }
    },
  });
  function flatten(node, result = []) {
    if (!node || typeof node !== 'object') return result;
    if (Array.isArray(node)) { node.forEach(child => flatten(child, result)); return result; }
    result.push(node); flatten(node.props?.children, result); return result;
  }
  function label(node) {
    if (typeof node === 'string') return node;
    if (Array.isArray(node)) return node.map(label).join('');
    return node && typeof node === 'object' ? label(node.props?.children) : '';
  }
  return {
    render(props) {
      cursor = 0; const element = fn(props);
      effects.splice(0).forEach(effect => effect());
      const nodes = flatten(element);
      return { nodes, button: text => nodes.find(node => node.type === 'button' && label(node) === text), text: label(element) };
    },
    unmount() { for (const slot of slots) slot?.cleanup?.(); },
    replayEffects() { for (const slot of slots) { if (slot?.effect) { slot.cleanup?.(); slot.cleanup = slot.effect(); } } },
  };
}

const managerText = key => translate(`settings.connections.trustManager.${key}`);

test('trust import preview rejects a route that differs from the open manager', async () => {
  const proxy = { mode: 'socks5', host: 'proxy.invalid', port: 1080, has_password: false };
  const props = { ...managerProps(), connection: { ...managerProps().connection, proxy } };
  for (const wholeFile of [false, true]) {
    const foreign = { ...fileImportPreview(), route: 'direct', keys: [] };
    const driver = trustManagerDriver({
      KnownHostsImportReport: 'fixture-file-report',
      sshInspectHostTrust: async () => foreign,
      sshPreviewKnownHostsImport: async () => foreign,
    });
    driver.render(props); await flushManager();
    await driver.render(props).button(managerText(wholeFile ? 'previewFile' : 'preview')).props.onClick();
    assert.ok(driver.render(props).text.includes(auditText('unexpected')));
  }
});
const managerProps = () => ({ connection: { id: 'id', host: 'fixture.invalid', port: 2222, known_hosts_path: 'C:\\fixture' }, onClose() {}, onChanged() {} });
const flushManager = () => new Promise(resolve => setImmediate(resolve));

const auditText = key => translate(`settings.connections.trustAudit.${key}`);
const auditView = (count=25) => ({connection_id:'id',host:'fixture.invalid',port:2222,route:'direct',revision:count,
  records:Array.from({length:count},(_,i)=>({revision:i+1,at_ms:i+1,action:'trust_verified',trusted_fingerprints:[`SHA256:audit-${i+1}`],revoked_fingerprints:[]}))});
const auditDriver = api => trustManagerDriver(api,'components/remote/SshHostTrustAuditDialog.tsx','SshHostTrustAuditDialog');

test('proxy audit accepts the saved route and refuses another protocol or endpoint', async () => {
  const proxy = { mode: 'socks5', host: 'proxy.invalid', port: 1080, has_password: true };
  const props = { connection: { ...managerProps().connection, proxy }, onBack() {} };
  for (const endpoint of [proxy, { ...proxy, port: 1081 }, { ...proxy, host: 'other.invalid' }, { ...proxy, mode: 'http_connect' }]) {
    const driver = auditDriver({ sshHostTrustAudit: async () => ({ ...auditView(1), route: 'socks5', proxy: endpoint }) });
    driver.render(props); await flushManager();
    const text = driver.render(props).text;
    assert.equal(text.includes('SHA256:audit-1'), endpoint === proxy);
    assert.equal(text.includes(auditText('unexpected')), endpoint !== proxy);
  }
});

test('trust management and audit omit explanatory paragraphs while retaining records and actions', async () => {
  const props = managerProps();
  const manager = trustManagerDriver();
  manager.render(props); await flushManager();
  const managerView = manager.render(props);
  assert.ok(!managerView.text.includes(managerText('help')));
  assert.ok(managerView.button(managerText('close')));
  const audit = auditDriver({sshHostTrustAudit:async()=>auditView(1)});
  audit.render({...props,onBack(){}}); await flushManager();
  const auditViewResult = audit.render({...props,onBack(){}});
  assert.ok(!auditViewResult.text.includes(auditText('help')));
  assert.ok(!auditViewResult.text.includes(auditText('usageHelp')));
  assert.ok(auditViewResult.text.includes('SHA256:audit-1'));
  assert.ok(auditViewResult.button(auditText('refresh')));
});

test('audit UI distinguishes successful authentication time, unknown history and metadata errors',async()=>{
  const props={connection:managerProps().connection,onBack(){}};
  for(const value of [undefined,null,1000]) {
    const driver=auditDriver({sshHostTrustAudit:async()=>({...auditView(0),last_used_at_ms:value,activity_error:null})});
    driver.render(props);await flushManager();const text=driver.render(props).text;
    assert.ok(text.includes(auditText('lastUsed')));
    assert.ok(text.includes(value == null ? auditText('usageUnknown') : new Date(value).toLocaleString()));
    assert.ok(!text.includes('Invalid Date'));
  }
  const driver=auditDriver({sshHostTrustAudit:async()=>({...auditView(0),last_used_at_ms:null,activity_error:'fixture metadata invalid'})});
  driver.render(props);await flushManager();assert.match(driver.render(props).text,/fixture metadata invalid/);
});

test('shared trust records show per-key usage without substituting the enrollment timestamp',async()=>{
  const record={algorithm:'ssh-ed25519',fingerprint:'SHA256:fixture',source:'fixture',trusted_at_ms:1,revoked:false,last_used_at_ms:1000};
  const driver=trustManagerDriver({sshListHostKeys:async()=>[record]});
  const props=managerProps();driver.render(props);await flushManager();let view=driver.render(props);
  assert.ok(view.text.includes(managerText('lastUsed')));
  assert.ok(view.text.includes(new Date(1000).toLocaleString()));
});

test('read-only trust audit API uses the saved connection id and refuses browser mock success',async()=>{
  const calls=[];const expected=auditView();
  const fn=production('api.ts','sshHostTrustAudit',{getInvoke:()=>async(c,p)=>{calls.push([c,p]);return expected;}});
  assert.equal(await fn('id'),expected);
  assert.equal(calls[0][0],'ssh_host_trust_audit');
  assert.deepEqual(JSON.parse(JSON.stringify(calls[0][1])),{id:'id'});
  await assert.rejects(production('api.ts','sshHostTrustAudit',{getInvoke:()=>null})('id'),/desktop app/);
  const rust=readFileSync(path.join(root,'../src-tauri/src/lib.rs'),'utf8');
  assert.match(rust,/#\[tauri::command\]\s*async fn ssh_host_trust_audit\(/);
  assert.match(rust,/generate_handler!\[[\s\S]*?\bssh_host_trust_audit,/);
});

test('audit UI reads only when opened and pages bounded records newest first',async()=>{
  let calls=0;
  const driver=auditDriver({sshHostTrustAudit:async()=>{calls++;return auditView();}});
  const props={connection:managerProps().connection,onBack(){}};
  let view=driver.render(props);await flushManager();view=driver.render(props);
  assert.equal(calls,1);
  let rows=view.nodes.filter(n=>n.props?.['data-audit-revision']);
  assert.equal(rows.length,10);assert.equal(rows[0].props['data-audit-revision'],25);assert.equal(rows.at(-1).props['data-audit-revision'],16);
  view.button(auditText('next')).props.onClick();view=driver.render(props);
  rows=view.nodes.filter(n=>n.props?.['data-audit-revision']);assert.equal(rows[0].props['data-audit-revision'],15);
  view.button(auditText('next')).props.onClick();view=driver.render(props);
  assert.equal(view.nodes.filter(n=>n.props?.['data-audit-revision']).length,5);
  assert.equal(view.button(auditText('next')).props.disabled,true);assert.equal(calls,1,'pagination is local, not another IPC read');
});

test('audit UI exposes loading/errors/empty state and single-flights refresh',async()=>{
  let calls=0,resolve;
  const driver=auditDriver({sshHostTrustAudit:()=>{calls++;return calls===1?Promise.reject(Error('fixture unavailable')):new Promise(r=>{resolve=r;});}});
  const props={connection:managerProps().connection,onBack(){}};
  driver.render(props);await flushManager();let view=driver.render(props);
  assert.match(view.text,/fixture unavailable/);
  const pending=view.button(auditText('refresh')).props.onClick();
  await view.button(auditText('refresh')).props.onClick();
  view=driver.render(props);assert.equal(calls,2);assert.ok(view.text.includes(auditText('busy')));
  resolve(auditView(0));await pending;view=driver.render(props);
  assert.ok(view.text.includes(auditText('empty')));
});

test('audit UI refuses foreign scope and late StrictMode responses cannot replace current history',async()=>{
  const props={connection:managerProps().connection,onBack(){}};
  for(const mismatch of [{host:'foreign.invalid'},{connection_id:'foreign'},{port:22},{route:'jump'},{records:auditView(257).records}]) {
    const foreign=auditDriver({sshHostTrustAudit:async()=>({...auditView(),...mismatch})});
    foreign.render(props);await flushManager();assert.ok(foreign.render(props).text.includes(auditText('unexpected')));
  }
  const waiting=[];const driver=auditDriver({sshHostTrustAudit:()=>new Promise(resolve=>waiting.push(resolve))});
  driver.render(props);driver.replayEffects();assert.equal(waiting.length,2);
  waiting[1](auditView(2));await flushManager();
  waiting[0](auditView(25));await flushManager();
  assert.equal(driver.render(props).nodes.filter(n=>n.props?.['data-audit-revision']).length,2);
  driver.unmount();
});

test('shared manager exposes the same read-only audit surface to both existing entries',async()=>{
  const driver=trustManagerDriver({SshHostTrustAuditDialog:'fixture-audit'});
  const props=managerProps();driver.render(props);await flushManager();
  driver.render(props).button(managerText('audit')).props.onClick();
  const view=driver.render(props);const surface=view.nodes.find(n=>n.type==='fixture-audit');
  assert.equal(surface.props.connection,props.connection);
  surface.props.onBack();await flushManager();
  assert.ok(driver.render(props).button(managerText('audit')));
});

test('rotation APIs use separate commands and do not accept client-supplied public keys', async () => {
  for (const [name, command, args, expected] of [
    ['sshBeginHostKeyRotation', 'ssh_begin_host_key_rotation', ['id'], { id:'id' }],
    ['sshConfirmHostKeyRotation', 'ssh_confirm_host_key_rotation', ['id','attempt','challenge'], { id:'id', attemptId:'attempt', challengeId:'challenge' }],
  ]) {
    const calls=[];
    const fn=production('api.ts',name,{getInvoke:()=>async(c,input)=>{calls.push([c,input]);return 'receipt';}});
    assert.equal(await fn(...args),'receipt');
    assert.equal(calls[0][0],command);
    assert.deepEqual(JSON.parse(JSON.stringify(calls[0][1])),expected);
    await assert.rejects(production('api.ts',name,{getInvoke:()=>null})(...args),/desktop app/);
    const rust=readFileSync(path.join(root,'../src-tauri/src/lib.rs'),'utf8');
    assert.match(rust,new RegExp(`#\\[tauri::command\\]\\s*async fn ${command}\\(`));
    assert.match(rust,new RegExp(`generate_handler!\\[[\\s\\S]*?\\b${command},`));
  }
});

const rotationText=key=>translate(`settings.connections.hostRotation.${key}`);
const rotationPreview=()=>({challenge:{connection_id:'id',attempt_id:'attempt',challenge_id:'challenge',host:'fixture.invalid',port:2222,username:'fixture',role:'target',route:'direct',kind:'key_rotation',algorithm:'ssh-ed25519',fingerprint:'SHA256:new-fixture',expires_at_ms:Date.now()+120000},previous_keys:[{line_number:1,algorithm:'ssh-ed25519',fingerprint:'SHA256:old-fixture'}]});
function rotationDriver(api={}) {
  return trustManagerDriver(api,'components/remote/SshHostKeyRotationDialog.tsx','SshHostKeyRotationDialog',{window:{setTimeout:()=>1,clearTimeout(){}}});
}

test('proxy rotation accepts only the configured protocol and endpoint', async () => {
  const proxy = { mode: 'http_connect', host: 'proxy.invalid', port: 8080, has_password: false };
  const props = { connection: { ...managerProps().connection, proxy }, onBack() {}, onCommitted() {} };
  for (const endpoint of [proxy, { ...proxy, port: 8081 }, { ...proxy, mode: 'socks5' }]) {
    let cancelled = 0;
    const driver = rotationDriver({
      sshBeginHostKeyRotation: async () => { const value = rotationPreview(); return { ...value, challenge: { ...value.challenge, route: 'http_connect', proxy: endpoint } }; },
      sshCancelHostKey: async () => { cancelled++; },
    });
    await driver.render(props).button(rotationText('probe')).props.onClick();
    assert.equal(driver.render(props).text.includes('SHA256:new-fixture'), endpoint === proxy);
    assert.equal(cancelled, endpoint === proxy ? 0 : 1);
  }
});

test('rotation UI requires explicit probing and independent verification, single-flights and retains the receipt',async()=>{
  let probes=0,writes=0,complete;
  const driver=rotationDriver({sshBeginHostKeyRotation:async()=>{probes++;return rotationPreview();},sshConfirmHostKeyRotation:async()=>{writes++;return new Promise(resolve=>{complete=resolve;});}});
  const props={connection:managerProps().connection,onBack(){},onCommitted(){}};
  let view=driver.render(props);
  assert.equal(probes,0,'mount never contacts the server');
  await view.button(rotationText('probe')).props.onClick();
  view=driver.render(props);
  assert.match(view.text,/SHA256:old-fixture/);assert.match(view.text,/SHA256:new-fixture/);
  assert.equal(view.button(rotationText('confirm')).props.disabled,true);
  view.nodes.find(n=>n.type==='fixture-checkbox').props.onCheckedChange(true);
  view=driver.render(props);
  const first=view.button(rotationText('confirm')).props.onClick();
  await view.button(rotationText('confirm')).props.onClick();
  assert.equal(writes,1);
  complete({audit:{revision:2,at_ms:1,algorithm:'ssh-ed25519',fingerprint:'SHA256:new-fixture',retired_fingerprints:['SHA256:old-fixture']},disconnect_failed_ids:['alias']});
  await first; view=driver.render(props);
  assert.ok(view.text.includes(rotationText('committed')));
  assert.match(view.text,/alias/);
  assert.equal(view.button(rotationText('confirm')),undefined,'no repeat write after committed receipt');
});

test('rotation UI requires a new probe after consumed/stale response and blocks expired confirmation',async()=>{
  const driver=rotationDriver({sshBeginHostKeyRotation:async()=>rotationPreview(),sshConfirmHostKeyRotation:async()=>{throw Error('stale fixture');}});
  const props={connection:managerProps().connection,onBack(){},onCommitted(){}};
  await driver.render(props).button(rotationText('probe')).props.onClick();
  driver.render(props).nodes.find(n=>n.type==='fixture-checkbox').props.onCheckedChange(true);
  await driver.render(props).button(rotationText('confirm')).props.onClick();
  let view=driver.render(props);
  assert.match(view.text,/stale fixture/);assert.equal(view.button(rotationText('confirm')),undefined);
  const expired=rotationDriver({sshBeginHostKeyRotation:async()=>{const p=rotationPreview();p.challenge.expires_at_ms=0;return p;}});
  await expired.render(props).button(rotationText('probe')).props.onClick();
  view=expired.render(props);
  assert.equal(view.button(rotationText('confirm')).props.disabled,true);
  assert.ok(view.text.includes(rotationText('expired')));
});

test('rotation UI cancels late probes after unmount and cancelled prompts never submit',async()=>{
  let complete,cancels=0,backs=0;
  const driver=rotationDriver({sshBeginHostKeyRotation:()=>new Promise(resolve=>{complete=resolve;}),sshCancelHostKey:async()=>{cancels++;}});
  const props={connection:managerProps().connection,onBack(){backs++;},onCommitted(){}};
  const probe=driver.render(props).button(rotationText('probe')).props.onClick();
  driver.unmount();complete(rotationPreview());await probe;
  assert.equal(cancels,1);
  const next=rotationDriver({sshBeginHostKeyRotation:async()=>rotationPreview(),sshCancelHostKey:async()=>{cancels++;}});
  await next.render(props).button(rotationText('probe')).props.onClick();
  await next.render(props).button(rotationText('back')).props.onClick();
  assert.equal(cancels,2);assert.equal(backs,1);
});

test('rotation UI rejects a response for another endpoint or a normal enrollment challenge',async()=>{
  for(const mismatch of [{connection_id:'foreign'},{host:'other.invalid'},{port:22},{kind:'unknown_host'},{route:'jump'},{role:'jump'}]) {
    let cancelled=0;
    const driver=rotationDriver({sshBeginHostKeyRotation:async()=>{const p=rotationPreview();Object.assign(p.challenge,mismatch);return p;},sshCancelHostKey:async()=>{cancelled++;}});
    const props={connection:managerProps().connection,onBack(){},onCommitted(){}};
    await driver.render(props).button(rotationText('probe')).props.onClick();
    const view=driver.render(props);
    assert.equal(cancelled,1);assert.equal(view.button(rotationText('confirm')),undefined);
  }
});

test('shared trust manager opens the separate rotation surface and reloads when returning',async()=>{
  const driver=trustManagerDriver({SshHostKeyRotationDialog:'fixture-rotation'});
  const props=managerProps();driver.render(props);await flushManager();
  let view=driver.render(props);
  view.button(managerText('rotation')).props.onClick();view=driver.render(props);
  const surface=view.nodes.find(n=>n.type==='fixture-rotation');
  assert.equal(surface.props.connection,props.connection);
  surface.props.onBack();await flushManager();
  assert.ok(driver.render(props).button(managerText('rotation')));
});

const fileImportPreview = () => ({ connection_id:'id', host:'fixture.invalid', port:2222, route:'direct', known_hosts_path:'C:\\fixture', source_sha256:'fixture-digest', importable:1, skipped:1, rows:[
  {line_number:1,host:'other.invalid',port:22,algorithm:'ssh-ed25519',fingerprint:'SHA256:public-fixture',status:'ready'},
  {line_number:2,host:null,port:null,algorithm:null,fingerprint:null,status:'hashed_host'},
] });

test('whole-file API requires desktop and forwards the reviewed manifest unchanged', async () => {
  const preview = fileImportPreview();
  for (const [name, command, args] of [
    ['sshPreviewKnownHostsImport','ssh_preview_known_hosts_import',['id']],
    ['sshImportKnownHosts','ssh_import_known_hosts',['id',preview]],
  ]) {
    const calls=[];
    const fn=production('api.ts',name,{getInvoke:()=>async (method,input)=>{calls.push([method,input]); return preview;}});
    await fn(...args); assert.equal(calls[0][0],command); assert.equal(calls[0][1].id,'id');
    if(args.length===2)assert.equal(calls[0][1].expected,preview);
    await assert.rejects(production('api.ts',name,{getInvoke:()=>null})(...args),/desktop/i);
  }
});

test('file import shares pending gate, requires fresh acknowledgement, and displays committed outcome', async () => {
  let writes=0;
  const expected=fileImportPreview();
  const driver=trustManagerDriver({ KnownHostsImportReport:'fixture-file-report',
    sshPreviewKnownHostsImport:async()=>expected,
    sshImportKnownHosts:async(_id,received)=>{writes++; assert.equal(received,expected);return {preview:received,imported:1,skipped:1,disconnect_failed_ids:[]};},
  });
  const props=managerProps();driver.render(props);await flushManager();
  await driver.render(props).button(managerText('previewFile')).props.onClick();
  assert.equal(writes,0);
  let view=driver.render(props);assert.equal(view.button(managerText('confirmFileImport')).props.disabled,true);
  view.nodes.find(node=>node.type==='fixture-checkbox').props.onCheckedChange(true);
  await driver.render(props).button(managerText('confirmFileImport')).props.onClick();
  assert.equal(writes,1);
  view=driver.render(props);assert.ok(view.nodes.some(node=>node.type==='fixture-file-report'&&node.props.imported===1));
  assert.equal(view.button(managerText('confirmFileImport')),undefined);
});

test('file report paginates large manifests rather than mounting every row', () => {
  let page=0;
  const fn=production('components/remote/KnownHostsImportReport.tsx','KnownHostsImportReport',{
    React,Button:'button',useTranslation:()=>({t:translate}),useState:()=>[page,value=>{page=typeof value==='function'?value(page):value;}],
  });
  const preview=fileImportPreview();preview.rows=Array.from({length:250},(_,i)=>({...preview.rows[0],line_number:i+1,host:`host-${i}.invalid`}));
  let element=fn({preview});let html=renderToStaticMarkup(element);
  assert.equal((html.match(/data-import-row=/g)||[]).length,100);
  assert.ok(!html.includes('host-100.invalid'));
  const find=node=>node&&typeof node==='object'&&(node.props?.['data-import-next']?node:React.Children.toArray(node.props?.children).map(find).find(Boolean));
  find(element).props.onClick();
  html=renderToStaticMarkup(fn({preview}));assert.ok(html.includes('host-100.invalid'));assert.ok(!html.includes('host-0.invalid'));
});

test('committed whole-file receipt survives a follow-up list failure without enabling a repeat write', async () => {
  let reads=0;let writes=0;
  const preview=fileImportPreview();
  const driver=trustManagerDriver({KnownHostsImportReport:'fixture-file-report',
    sshListHostKeys:async()=>{if(++reads>1)throw Error('fixture list failure');return [];},
    sshPreviewKnownHostsImport:async()=>preview,
    sshImportKnownHosts:async()=>{writes++;return {preview,imported:1,skipped:1,disconnect_failed_ids:[]};},
  });
  const props=managerProps();driver.render(props);await flushManager();
  await driver.render(props).button(managerText('previewFile')).props.onClick();
  driver.render(props).nodes.find(node=>node.type==='fixture-checkbox').props.onCheckedChange(true);
  await driver.render(props).button(managerText('confirmFileImport')).props.onClick();
  const view=driver.render(props);assert.equal(writes,1);
  assert.ok(view.nodes.some(node=>node.type==='fixture-file-report'&&node.props.imported===1));
  assert.ok(view.text.includes('fixture list failure'));
  assert.equal(view.button(managerText('confirmFileImport')),undefined);
});

test('failed file commit requires a new preview and fresh acknowledgement', async () => {
  const preview=fileImportPreview();
  const driver=trustManagerDriver({KnownHostsImportReport:'fixture-file-report',sshPreviewKnownHostsImport:async()=>preview,
    sshImportKnownHosts:async()=>{throw Error('stale fixture preview');},
  });const props=managerProps();driver.render(props);await flushManager();
  await driver.render(props).button(managerText('previewFile')).props.onClick();
  driver.render(props).nodes.find(node=>node.type==='fixture-checkbox').props.onCheckedChange(true);
  await driver.render(props).button(managerText('confirmFileImport')).props.onClick();
  assert.equal(driver.render(props).button(managerText('confirmFileImport')),undefined);
  await driver.render(props).button(managerText('previewFile')).props.onClick();
  assert.equal(driver.render(props).button(managerText('confirmFileImport')).props.disabled,true);
});

test('host trust manager previews without writing, and cancellation never grants trust', async () => {
  let inspections = 0; let writes = 0;
  const expected = { host: 'fixture.invalid', port: 2222, route: 'direct', known_hosts_path: 'C:\\fixture', keys: [{algorithm:'ssh-ed25519',fingerprint:'SHA256:fixture',line_number:1}] };
  const driver = trustManagerDriver({ sshInspectHostTrust: async () => { inspections++; return expected; }, sshImportHostTrust: async () => { writes++; return []; } });
  const props = managerProps(); driver.render(props); await flushManager();
  assert.equal(inspections, 0); assert.equal(writes, 0);
  await driver.render(props).button(managerText('preview')).props.onClick();
  const preview = driver.render(props);
  assert.ok(preview.text.includes('SHA256:fixture'));
  assert.equal(preview.button(managerText('confirmImport')).props.disabled, true);
  preview.button(managerText('cancel')).props.onClick();
  assert.equal(driver.render(props).button(managerText('confirmImport')), undefined);
  assert.equal(writes, 0);
});

test('host trust manager requires acknowledgement and sends one immutable preview', async () => {
  const expected = { host: 'fixture.invalid', port: 2222, route: 'direct', known_hosts_path: 'C:\\fixture', keys: [{algorithm:'ssh-ed25519',fingerprint:'SHA256:fixture',line_number:1}] };
  let complete; const calls = [];
  const driver = trustManagerDriver({ sshInspectHostTrust: async () => expected, sshImportHostTrust: async (...args) => { calls.push(args); return new Promise(resolve => { complete = resolve; }); } });
  const props = managerProps(); driver.render(props); await flushManager();
  await driver.render(props).button(managerText('preview')).props.onClick();
  driver.render(props).nodes.find(node => node.type === 'fixture-checkbox').props.onCheckedChange(true);
  const confirm = driver.render(props).button(managerText('confirmImport')).props.onClick;
  const first = confirm(); await confirm();
  assert.equal(calls.length, 1);
  assert.deepEqual(calls[0], ['id', expected]);
  assert.equal(driver.render(props).button(managerText('confirmImport')).props.disabled, true);
  driver.unmount(); complete([]); await first;
});

test('revocation needs a separate explicit confirmation and preserves errors', async () => {
  const record = { algorithm:'ssh-ed25519', fingerprint:'SHA256:fixture', source:'fixture', trusted_at_ms:1, revoked:false };
  const calls = [];
  const driver = trustManagerDriver({ sshListHostKeys: async () => [record], sshRevokeHostKey: async (...args) => { calls.push(args); throw Error('storage unavailable'); } });
  const props = managerProps(); driver.render(props); await flushManager();
  driver.render(props).button(managerText('revoke')).props.onClick();
  assert.equal(calls.length, 0);
  await driver.render(props).button(managerText('confirmRevoke')).props.onClick();
  assert.deepEqual(calls[0], ['id', 'SHA256:fixture']);
  assert.ok(driver.render(props).text.includes('storage unavailable'));
});

test('host trust manager survives StrictMode effect cleanup/replay while initial read is pending', async () => {
  const waiting = [];
  const driver = trustManagerDriver({ sshListHostKeys: async () => new Promise(resolve => waiting.push(resolve)) });
  const props = managerProps(); driver.render(props);
  driver.replayEffects();
  assert.equal(waiting.length, 2, 'the replay must not remain stuck behind an invalidated request');
  waiting[0]([{algorithm:'ssh-ed25519',fingerprint:'SHA256:stale',source:'fixture',trusted_at_ms:1,revoked:false}]);
  waiting[1]([{algorithm:'ssh-ed25519',fingerprint:'SHA256:current',source:'fixture',trusted_at_ms:1,revoked:false}]);
  await flushManager();
  const view = driver.render(props);
  assert.ok(view.text.includes('SHA256:current'));
  assert.ok(!view.text.includes('SHA256:stale'));
  assert.equal(view.button(managerText('preview')).props.disabled, false);
});

test('both real SSH surfaces use the same application trust manager', () => {
  for (const file of ['components/settings/ConnectionsSettings.tsx', 'components/remote/RemoteSidebar.tsx']) {
    assert.match(readFileSync(path.join(root, file), 'utf8'), /<SshHostTrustManager\b/);
  }
});

test('connection confirmation APIs bind both server-generated IDs and never fake a browser connection', async () => {
  for (const [name, command, args, expected] of [
    ['sshBeginConnection', 'ssh_begin_connection', ['id'], {id:'id'}],
    ['sshRespondHostKey', 'ssh_respond_host_key', ['id','attempt','challenge','once'], {id:'id',attemptId:'attempt',challengeId:'challenge',decision:'once'}],
    ['sshCancelHostKey', 'ssh_cancel_host_key', ['id','attempt','challenge'], {id:'id',attemptId:'attempt',challengeId:'challenge'}],
    ['sshRespondKeyboardInteractive', 'ssh_respond_keyboard_interactive', ['id','attempt','challenge',['code','device']], {id:'id',attemptId:'attempt',challengeId:'challenge',responses:['code','device']}],
    ['sshCancelConnectionAttempt', 'ssh_cancel_connection_attempt', ['id','attempt','challenge'], {id:'id',attemptId:'attempt',challengeId:'challenge'}],
  ]) {
    const calls=[];
    await production('api.ts',name,{getInvoke:()=>async(command,args)=>calls.push({command,args})})(...args);
    assert.equal(calls[0].command,command);
    assert.deepEqual(JSON.parse(JSON.stringify(calls[0].args)),expected);
    await assert.rejects(production('api.ts',name,{getInvoke:()=>null})(...args),/desktop app/);
  }
});

function confirmationFlow(api, now = () => 1) {
  const states=[];
  const create = production('components/remote/SshConnectionFlow.ts','createSshConnectionFlow',{console});
  const flow = create(state=>states.push(state),api,now);
  flow.activate();
  return {flow,states};
}
function unknownChallenge(id='id') { return {connection_id:id,attempt_id:`attempt-${id}`,challenge_id:`challenge-${id}`,host:'fixture.invalid',port:22,username:'fixture',role:'target',route:'direct',kind:'unknown_host',algorithm:'ssh-ed25519',fingerprint:'SHA256:fixture',expires_at_ms:1000,can_save:true}; }

test('shared connection flow accepts proxy challenges and rejects inconsistent route metadata', async () => {
  for (const mode of ['http_connect', 'socks5']) {
    const challenge = { ...unknownChallenge(), route: mode, proxy: { mode, host: 'proxy.invalid', port: 1080 } };
    const { flow, states } = confirmationFlow({ begin: async () => ({ status: 'confirmation_required', challenge }), respond: async () => ({ status: 'connected' }), cancel: async () => {} });
    const connected = flow.connect('id').catch(() => false);
    await flushManager();
    assert.equal(states.at(-1).challenge?.proxy?.host, 'proxy.invalid');
    await flow.respond('once');
    assert.equal(await connected, true);
  }
  for (const metadata of [{ route: 'jump' }, { route: 'socks5' }, { route: 'direct', proxy: { mode: 'socks5', host: 'proxy.invalid', port: 1080 } }]) {
    let cancelled = 0;
    const { flow } = confirmationFlow({ begin: async () => ({ status: 'confirmation_required', challenge: { ...unknownChallenge(), ...metadata } }), cancel: async () => { cancelled++; } });
    await assert.rejects(flow.connect('id'), /scope/);
    assert.equal(cancelled, 1);
  }
});
function authenticationChallenge(id, attempt, challenge, prompts) { return {connection_id:id,attempt_id:attempt,challenge_id:challenge,host:'fixture.invalid',port:22,username:'fixture',name:'Fixture verification',instructions:'Complete both verification steps',prompts,expires_at_ms:1000}; }

test('unknown connection waits for an explicit decision and duplicate clicks never authenticate twice', async () => {
  const challenge=unknownChallenge(); const calls=[]; let complete;
  const {flow,states}=confirmationFlow({begin:async()=>({status:'confirmation_required',challenge}),respond:async(...args)=>{calls.push(args);await new Promise(resolve=>complete=resolve);return {status:'connected'};},cancel:async()=>{}});
  const connecting=flow.connect('id'); await flushManager();
  assert.equal(calls.length,0);
  assert.equal(states.at(-1).challenge.fingerprint,challenge.fingerprint);
  const pending=flow.respond('once'); await flow.respond('save');
  assert.equal(calls.length,1);
  assert.deepEqual(calls[0],['id',challenge.attempt_id,challenge.challenge_id,'once']);
  assert.equal(states.at(-1).busy,true);
  complete();await pending;assert.equal(await connecting,true);
});

test('cancelled, superseded and unmounted prompts never submit a trust response', async () => {
  let resolveOld; const cancelled=[]; const responded=[];
  const {flow}=confirmationFlow({begin:id=>id==='old'?new Promise(resolve=>resolveOld=resolve):Promise.resolve({status:'confirmation_required',challenge:unknownChallenge(id)}),respond:async(...args)=>responded.push(args),cancel:async(...args)=>cancelled.push(args)});
  const old=flow.connect('old'); const current=flow.connect('new'); await flushManager();
  resolveOld({status:'confirmation_required',challenge:unknownChallenge('old')});await flushManager();
  assert.equal(await old,false);
  await flow.cancel();assert.equal(await current,false);
  const disposed=flow.connect('disposed');await flushManager();flow.dispose();await flushManager();
  assert.equal(await disposed,false);
  assert.equal(responded.length,0);
  assert.equal(cancelled.length,3);
});

test('expired or non-saveable challenges cannot be accepted by a stale UI callback', async () => {
  const challenge={...unknownChallenge(),can_save:false};let now=1;const responded=[];
  const {flow}=confirmationFlow({begin:async()=>({status:'confirmation_required',challenge}),respond:async(...args)=>responded.push(args),cancel:async()=>{}},()=>now);
  const first=flow.connect('id');await flushManager();
  await flow.respond('save');assert.equal(responded.length,0);
  now=1001;await flow.respond('once');assert.equal(responded.length,0);
  await flow.cancel();assert.equal(await first,false);
});

test('backend confirmation failure rejects the connection and never reports success', async () => {
  const {flow,states}=confirmationFlow({begin:async()=>({status:'confirmation_required',challenge:unknownChallenge()}),respond:async()=>{throw Error('changed before authentication');},cancel:async()=>{}});
  const connecting=flow.connect('id');const failed=assert.rejects(connecting,/changed before authentication/);
  await flushManager();await flow.respond('once');await failed;
  assert.equal(states.at(-1).challenge,null);
});

test('one connection attempt advances from host verification through multiple MFA rounds', async () => {
  const host=unknownChallenge();
  const first=authenticationChallenge('id',host.attempt_id,'mfa-one',[{prompt:'Verification code',echo:false},{prompt:'Device label',echo:true}]);
  const second=authenticationChallenge('id',host.attempt_id,'mfa-two',[{prompt:'Second factor',echo:false}]);
  const calls=[];
  const {flow,states}=confirmationFlow({
    begin:async()=>({status:'confirmation_required',challenge:host}),
    respond:async()=>({status:'authentication_required',challenge:first}),
    respondAuthentication:async(...args)=>{
      calls.push(args);
      return calls.length===1?{status:'authentication_required',challenge:second}:{status:'connected'};
    },
    cancel:async()=>{},cancelAuthentication:async()=>{},
  });
  const connecting=flow.connect('id');
  await flushManager();
  await flow.respond('once');
  assert.equal(states.at(-1).authenticationChallenge.challenge_id,'mfa-one');
  await flow.respondAuthentication(['482901','terminal']);
  assert.equal(states.at(-1).authenticationChallenge.challenge_id,'mfa-two');
  await flow.respondAuthentication(['731604']);
  assert.equal(await connecting,true);
  assert.deepEqual(calls.map(args=>args.slice(0,3)),[
    ['id',host.attempt_id,'mfa-one'],['id',host.attempt_id,'mfa-two'],
  ]);
  assert.deepEqual(calls.map(args=>args[3]),[['482901','terminal'],['731604']]);
});

test('cancelling an MFA prompt uses the authentication challenge cancellation endpoint', async () => {
  const challenge=authenticationChallenge('id','attempt-id','mfa-id',[{prompt:'Code',echo:false}]);
  const cancelled=[];
  const {flow}=confirmationFlow({
    begin:async()=>({status:'authentication_required',challenge}),
    respond:async()=>({status:'connected'}),cancel:async()=>{},
    respondAuthentication:async()=>({status:'connected'}),
    cancelAuthentication:async(...args)=>cancelled.push(args),
  });
  const connecting=flow.connect('id');
  await flushManager();
  await flow.cancel();
  assert.equal(await connecting,false);
  assert.deepEqual(cancelled,[["id","attempt-id","mfa-id"]]);
});

test('private-key unlock prompt clears submitted, cancelled and expired secrets', async () => {
  let expire;
  const driver = trustManagerDriver({}, 'components/remote/SshHostKeyPrompt.tsx', 'SshAuthenticationPrompt', {
    Input: 'input', window: { setTimeout: callback => { expire = callback; return 1; }, clearTimeout() {} },
  });
  const submitted = [];
  let cancelled = 0;
  const props = {
    challenge: { ...authenticationChallenge('id', 'attempt', 'unlock', [{ prompt: 'Local unlock', echo: false }]), kind: 'private_key', expires_at_ms: Date.now() + 60000 },
    busy: false, onRespond: async responses => submitted.push([...responses]), onCancel: async () => cancelled++,
  };
  const input = view => view.nodes.find(node => node.type === 'input');
  const form = view => view.nodes.find(node => node.type === 'form');
  let view = driver.render(props);
  assert.ok(view.text.includes(translate('settings.connections.unlockPrivateKey')));
  assert.equal(input(view).props.type, 'password');
  assert.equal(input(view).props.autoComplete, 'off');
  input(view).props.onChange({ target: { value: 'fixture-unlock-only' } });
  await form(driver.render(props)).props.onSubmit({ preventDefault() {} });
  assert.deepEqual(submitted, [['fixture-unlock-only']]);
  assert.equal(input(driver.render(props)).props.value, '');

  input(driver.render(props)).props.onChange({ target: { value: 'fixture-cancel-only' } });
  driver.render(props).button(translate('settings.connections.authenticationChallenge.cancel')).props.onClick();
  assert.equal(cancelled, 1);
  assert.equal(input(driver.render(props)).props.value, '');

  input(driver.render(props)).props.onChange({ target: { value: 'fixture-expiry-only' } });
  expire();
  view = driver.render(props);
  assert.equal(input(view).props.value, '');
  assert.equal(input(view).props.disabled, true);
  await form(view).props.onSubmit({ preventDefault() {} });
  assert.equal(submitted.length, 1);
  driver.unmount();
});

test('private-key unlock prompt exposes its localized purpose as the accessible dialog name', () => {
  const driver = trustManagerDriver({}, 'components/remote/SshHostKeyPrompt.tsx', 'SshAuthenticationPrompt', {
    Input: 'input', window: { setTimeout: () => 1, clearTimeout() {} },
  });
  const view = driver.render({
    challenge: { ...authenticationChallenge('id', 'attempt', 'unlock', [{ prompt: 'Passphrase', echo: false }]), kind: 'private_key' },
    busy: false, onRespond: async () => {}, onCancel: async () => {},
  });
  assert.ok(view.nodes.some(node => node.props?.['aria-label'] === translate('settings.connections.unlockPrivateKey')));
  assert.equal(view.nodes.find(node => node.type === 'label').props.children, '私钥口令');
  driver.unmount();
});

test('connection flow advances from local unlock into server MFA without retaining its answer', async () => {
  const unlock = { ...authenticationChallenge('id', 'attempt', 'unlock', [{ prompt: 'Passphrase', echo: false }]), kind: 'private_key' };
  const mfa = { ...authenticationChallenge('id', 'attempt', 'mfa', [{ prompt: 'Code', echo: false }]), kind: 'keyboard_interactive' };
  const calls = [];
  const { flow, states } = confirmationFlow({
    begin: async () => ({ status: 'authentication_required', challenge: unlock }),
    respondAuthentication: async (...args) => { calls.push(args); return calls.length === 1 ? { status: 'authentication_required', challenge: mfa } : { status: 'connected' }; },
    cancelAuthentication: async () => {},
  });
  const connecting = flow.connect('id');
  await flushManager();
  await flow.respondAuthentication(['fixture-unlock-only']);
  assert.equal(states.at(-1).authenticationChallenge.kind, 'keyboard_interactive');
  assert.ok(!JSON.stringify(states).includes('fixture-unlock-only'));
  await flow.respondAuthentication(['482901']);
  assert.equal(await connecting, true);
  assert.deepEqual(calls, [['id', 'attempt', 'unlock', ['fixture-unlock-only']], ['id', 'attempt', 'mfa', ['482901']]]);
});

test('both actual SSH surfaces use the shared confirmation hook rather than a bare connect', () => {
  const settings=readFileSync(path.join(root,'components/settings/ConnectionsSettings.tsx'),'utf8');
  const remote=readFileSync(path.join(root,'components/remote/RemoteSidebar.tsx'),'utf8');
  for (const source of [settings,remote]) {
    assert.match(source,/useSshConnectionConfirmation\(\)/);
    assert.match(source,/confirmation\.connect\(/);
    assert.match(source,/\{confirmation\.prompt\}/);
  }
  assert.doesNotMatch(remote,/await sshConnect\(/);
  assert.match(settings,/await sshExec\(id, "echo ok"\)/,'connection test exercises the same accepted live client');
});

test('actual prompt requires verification, honors expiry and locks submitted decisions', async () => {
  const text=key=>translate(`settings.connections.hostChallenge.${key}`);
  const create=()=>trustManagerDriver({},'components/remote/SshHostKeyPrompt.tsx','SshHostKeyPrompt', { window:{setTimeout:()=>1,clearTimeout() {}} });
  const driver=create();const decisions=[];let cancellations=0;
  const props={challenge:{...unknownChallenge(),expires_at_ms:Date.now()+60000},busy:false,onRespond:async decision=>decisions.push(decision),onCancel:async()=>cancellations++};
  let view=driver.render(props);
  assert.ok(view.text.includes(props.challenge.fingerprint));
  assert.equal(view.button(text('once')).props.disabled,true);
  assert.equal(view.button(text('save')).props.disabled,true);
  view.nodes.find(node=>node.type==='fixture-checkbox').props.onCheckedChange(true);
  view=driver.render(props);
  assert.equal(view.button(text('once')).props.disabled,false);
  await view.button(text('once')).props.onClick();assert.deepEqual(decisions,['once']);
  view=driver.render({...props,busy:true});
  assert.equal(view.button(text('once')).props.disabled,true);
  assert.equal(view.button(text('cancel')).props.disabled,true);
  const expired=create().render({...props,challenge:{...props.challenge,expires_at_ms:1,can_save:false}});
  assert.ok(expired.text.includes(text('expired')));
  assert.equal(expired.button(text('once')).props.disabled,true);
  assert.equal(expired.button(text('save')),undefined);
  await expired.button(text('cancel')).props.onClick();assert.equal(cancellations,1);
});

test('settings tests only the same accepted live connection and never execute after cancellation', async () => {
  for (const accepted of [true,false]) {
    const calls=[];const noop=()=>{};
    const handler=production('components/settings/ConnectionsSettings.tsx','handleTest',{
      setTestingId:noop,setConnections:noop,setTestResult:noop,
      confirmation:{connect:async id=>{calls.push(['connect',id]);return accepted;}},
      sshExec:async(id,command)=>{calls.push(['exec',id,command]);return {exit_code:0,stdout:'ok\n',stderr:'',duration_ms:3};},
      normalizeSshError:error=>error??'',t:translate,message:{success:noop,error:error=>{throw Error(error);}},
    });
    await handler('id');
    assert.deepEqual(calls,accepted?[['connect','id'],['exec','id','echo ok']]:[['connect','id']]);
  }
});

test('remote directory loading reflects actual work, never an undefined tree after rejecting trust', () => {
  for (const [connectingId, loadingDir, expected] of [[null,null,false],['id',null,true],[null,'/',true],['other',null,false]]) {
    const pending=production('components/remote/RemoteSidebar.tsx','showDirectoryLoading',{selected:{id:'id'},connectingId,loadingDir});
    assert.equal(pending,expected);
  }
  const source=readFileSync(path.join(root,'components/remote/RemoteSidebar.tsx'),'utf8');
  assert.match(source,/showDirectoryLoading \? "settings.connections.checking" : "remote.notConnected"/);
});
