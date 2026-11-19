const { test } = require('node:test');
const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const ts = require('typescript');
const React = require('react');
const { renderToStaticMarkup } = require('react-dom/server');
const timingFile = 'components/remote/SshTimingFields.tsx';

function production(file, name, context = {}) {
  const source = readFileSync(path.resolve(__dirname, '../src', file), 'utf8');
  const ast = ts.createSourceFile(file, source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  let found;
  const visit = node => {
    if ((ts.isFunctionDeclaration(node) || ts.isVariableDeclaration(node)) && node.name?.getText(ast) === name) found = node;
    ts.forEachChild(node, visit);
  };
  visit(ast);
  assert.ok(found, `${file}: ${name}`);
  let text = found.getText(ast).replace(/^export\s+/, '');
  if (ts.isVariableDeclaration(found)) text = `const ${text};`;
  const timing = name => value => production(timingFile, name)(value);
  return vm.runInNewContext(ts.transpileModule(`${text}\n${name};`, { compilerOptions: {
    target:ts.ScriptTarget.ES2022, jsx:ts.JsxEmit.React, module:ts.ModuleKind.CommonJS,
  }}).outputText, {
    React, canSaveSshTiming:timing('canSaveSshTiming'), sshTimingRequest:timing('sshTimingRequest'), sshTimingFromDto:timing('sshTimingFromDto'),
    canSaveSshProxy:() => true, canSaveSshJump:() => true, sshProxyRequest:() => undefined, sshJumpRequest:() => undefined,
    sshProxyFromDto:() => ({}), sshJumpFromDto:() => ({}), authenticationFactorsFromDto:() => [],
    canSaveSshConnection:value => production('components/remote/SshAuthenticationFields.tsx', 'canSaveSshConnection')(value),
    ...context,
  });
}

const wire = {connect_timeout_secs:7, inactivity_timeout_secs:120, keepalive_interval_secs:15, keepalive_max:4};
const timing = {connectTimeout:'7', inactivityTimeout:'120', keepaliveInterval:'15', keepaliveMax:'4'};
const form = {name:'fixture', host:'fixture.invalid', port:'22', username:'fixture', authType:'agent', timing};

test('inactivity and keepalive failures remain distinct from network connection timeout', () => {
  const normalize = production('components/settings/ConnectionsSettings.tsx', 'normalizeSshError');
  const t = key => key;
  assert.equal(normalize('SSH inactivity timeout', t), 'settings.connections.timing.inactivityExpired');
  assert.equal(normalize('SSH keepalive response limit exceeded', t), 'settings.connections.timing.keepaliveExpired');
  assert.equal(normalize('timeout after 7s', t), 'settings.connections.errors.timeout');
});

test('legacy defaults and disabled timers use null rather than zero', () => {
  const from = production(timingFile, 'sshTimingFromDto');
  const serialize = production(timingFile, 'sshTimingRequest');
  assert.deepEqual(JSON.parse(JSON.stringify(serialize(from()))), {
    connect_timeout_secs:20, inactivity_timeout_secs:null, keepalive_interval_secs:null, keepalive_max:3,
  });
  assert.throws(() => serialize({...timing, keepaliveInterval:'0'}), /Invalid SSH timing/);
});

test('the shared controls expose four labeled numeric fields and disable the unused limit', () => {
  const changes = [];
  const Fields = production(timingFile, 'SshTimingFields', {
    useId:() => 'timing', useTranslation:() => ({t:key=>key}), Input:'input', Label:'label',
  });
  const tree = Fields({value:{...timing, keepaliveInterval:''}, onChange:value=>changes.push(value)});
  const markup = renderToStaticMarkup(tree);
  assert.equal((markup.match(/type="number"/g) ?? []).length, 4);
  assert.ok(!markup.includes('<small'));
  const nodes = [];
  const visit = node => { if (!node || typeof node !== 'object') return; if (Array.isArray(node)) return node.forEach(visit); nodes.push(node); visit(node.props?.children); };
  visit(tree);
  const maximum = nodes.find(node => node.props?.id === 'timing-keepaliveMax');
  assert.equal(maximum.props.disabled, true);
  assert.equal(maximum.props.max, 100);
  nodes.find(node => node.props?.id === 'timing-inactivityTimeout').props.onChange({target:{value:''}});
  assert.equal(production(timingFile, 'sshTimingRequest')(changes.at(-1)).inactivity_timeout_secs, null);
});

test('save validation rejects malformed timing before either editor submits', () => {
  const validate = production('components/remote/SshAuthenticationFields.tsx', 'canSaveSshConnection');
  for (const [field, bad] of [['connectTimeout','0'],['connectTimeout','86401'],['connectTimeout',''],
    ['inactivityTimeout','-1'],['keepaliveInterval','1.5'],['keepaliveMax','101'],['keepaliveMax','0']]) {
    assert.equal(validate({...form, timing:{...timing, [field]:bad}}), false, `${field}=${bad}`);
  }
  assert.equal(validate(form), true);
});

test('both API calls forward timing through IPC', async () => {
  for (const name of ['sshCreateConnection','sshUpdateConnection']) {
    const args = ['fixture','fixture.invalid',22,'fixture','agent',undefined,undefined,undefined,undefined,undefined,undefined,undefined,undefined,undefined,wire];
    if (name.includes('Update')) args.unshift('id');
    let input;
    await production('api.ts', name, {getInvoke:() => async (_, value) => {input=value; return {id:'saved'};}})(...args);
    assert.deepEqual(JSON.parse(JSON.stringify(input.timing)), wire);
  }
});

for (const file of ['components/settings/ConnectionsSettings.tsx','components/remote/RemoteSidebar.tsx']) {
  test(`${file}: editing restores timing and both save routes submit it`, async () => {
    let restored;
    const noop = () => {};
    production(file, file.includes('RemoteSidebar') ? 'startEdit':'openEdit', {
      setForm:value => {restored=value;}, setEditingId:noop, setIsModalOpen:noop, setTestResult:noop, setEditingConn:noop, setManagerMode:noop,
    })({id:'id', name:'fixture', host:'fixture.invalid', port:22, username:'fixture', auth_type:'agent', timing:wire});
    assert.deepEqual(JSON.parse(JSON.stringify(restored.timing)), timing);
    for (const editing of [false,true]) {
      let submitted;
      await production(file, 'handleSave', {form, editingId:editing?'id':null, managerMode:editing?'edit':'create', editingConn:{id:'id'},
        sshCreateConnection:async (...args) => {submitted=args; return {id:'saved'};}, sshUpdateConnection:async (...args) => {submitted=args;},
        closeModal:noop, load:noop, loadConnections:noop, setForm:noop, setEditingConn:noop, setManagerMode:noop, handleSelect:noop, emptyForm:{},
        normalizeSshError:e=>e, message:{error:e=>{throw Error(e);}}, t:key=>key,
      })();
      assert.deepEqual(JSON.parse(JSON.stringify(submitted[14+Number(editing)])), wire);
    }
  });
}
