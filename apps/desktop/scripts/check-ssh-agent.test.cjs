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

for (const file of ['components/settings/ConnectionsSettings.tsx', 'components/remote/RemoteSidebar.tsx']) {
  test(`${file}: Agent identity selection survives editing and reaches both save commands`, async () => {
    const fingerprint = 'SHA256:fixture-selected-identity';
    let form;
    production(file, file.includes('RemoteSidebar') ? 'startEdit' : 'openEdit', {
      setForm: value => { form = value; }, setEditingId() {}, setIsModalOpen() {}, setTestResult() {}, setEditingConn() {}, setManagerMode() {},
    })({ id:'id', name:'fixture', host:'fixture.invalid', port:22, username:'fixture', auth_type:'agent', agent_fingerprint:fingerprint });
    assert.equal(form.agentFingerprint, fingerprint);
    for (const editing of [false, true]) {
      const calls = [];
      const noop = () => {};
      await production(file, 'handleSave', {
        form, editingId:editing ? 'id' : null, managerMode:editing ? 'edit' : 'create', editingConn:{id:'id'},
        sshCreateConnection:async (...args) => { calls.push(args); return {id:'id'}; }, sshUpdateConnection:async (...args) => calls.push(args),
        closeModal:noop, load:noop, loadConnections:noop, setForm:noop, setEditingConn:noop, setManagerMode:noop, handleSelect:noop, emptyForm:{},
        normalizeSshError:error=>error, message:{error:error=>{throw Error(error);}}, t:key=>key,
      })();
      assert.equal(calls[0][editing ? 16 : 15], fingerprint);
    }
  });
}

test('Agent identity selection crosses the IPC boundary without occupying keyPath', async () => {
  const calls = [];
  const create = production('api.ts', 'sshCreateConnection', {getInvoke:()=>async (...args)=>{ calls.push(args); return {}; }});
  await create('fixture','fixture.invalid',22,'fixture','agent',undefined,undefined,undefined,'strict',undefined,undefined,undefined,undefined,undefined,undefined,'SHA256:selected');
  assert.equal(calls[0][1].agentFingerprint, 'SHA256:selected');
  assert.equal(calls[0][1].keyPath, undefined);
});

test('automatic public-key mode is separate from Agent inspection and provider changes clear the pin', () => {
  const noop = () => null;
  const Select = props => React.createElement('section', null, props.children);
  const control = props => React.createElement('div', null, props.children);
  const fingerprint = 'SHA256:selected';
  const changes=[];
  const Fields=production('components/remote/SshAuthenticationFields.tsx','SshAuthenticationFields', {
    React,useId:()=> 'fixture',useState:()=> [null,noop],useEffect:noop,useTranslation:()=>({t:key=>key}),
    Select,SelectTrigger:control,SelectValue:control,SelectItem:control,SelectContent:control,Label:control,Button:control,Input:control,Textarea:control,
  });
  function selects(element,result=[]) {
    if (!React.isValidElement(element)) return result;
    if(element.type===Select) result.push(element);
    React.Children.forEach(element.props.children,child=>selects(child,result));
    return result;
  }
  const props={onChange:value=>changes.push(value),onPickKeyFile:noop,onPickCertificateFile:noop};
  const automatic=selects(Fields({...props,value:{authType:'auto_public_key',agentFingerprint:fingerprint}}));
  assert.equal(automatic.length,1);
  assert.equal(automatic[0].props.value,'auto_public_key');
  const agent=selects(Fields({...props,value:{authType:'agent',agentFingerprint:fingerprint}}));
  assert.equal(agent.length,3);
  assert.equal(agent[2].props.value,fingerprint);
  agent[1].props.onValueChange('pageant');
  assert.equal(changes[0].agentFingerprint,'');
  agent[2].props.onValueChange('automatic');
  assert.equal(changes[1].agentFingerprint,'');
});

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
  }).outputText, {
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
    SshDefaultIdentityPicker: () => null,
    ...context,
  });
}

for (const file of ['components/settings/ConnectionsSettings.tsx', 'components/remote/RemoteSidebar.tsx']) {
  test(`${file}: empty private-key content cannot be saved and an existing secret can be preserved`, async () => {
    for (const [editing, keyContent, hasKeyContent, expected] of [
      [false, '', false, false], [false, '   \n', false, false],
      [false, 'fixture-private-body', false, true], [true, '', true, true], [true, '', false, false],
    ]) {
      const calls = [];
      const noop = () => {};
      await production(file, 'handleSave', {
        form: { name: 'fixture', host: 'fixture.invalid', port: '22', username: 'fixture', authType: 'content', keyPath: '', keyContent, hasKeyContent, certificatePath: '', password: '', knownHostsPath: '', hostKeyPolicy: 'strict' },
        editingId: editing ? 'id' : null, managerMode: editing ? 'edit' : 'create', editingConn: { id: 'id' },
        sshCreateConnection: async (...args) => { calls.push(args); return { id: 'created' }; }, sshUpdateConnection: async (...args) => calls.push(args),
        closeModal: noop, load: noop, loadConnections: noop, setForm: noop, setEditingConn: noop, setManagerMode: noop, handleSelect: noop, emptyForm: {},
        normalizeSshError: error => error, message: { error: error => { throw Error(error); } }, t: key => key,
      })();
      assert.equal(calls.length, expected ? 1 : 0, `editing=${editing}, saved=${hasKeyContent}, body=${JSON.stringify(keyContent)}`);
    }
  });

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

test('explicit chains require bounded complete factors before save', () => {
  const canSave = production('components/remote/SshAuthenticationFields.tsx','canSaveSshConnection');
  const form = {name:'fixture',host:'fixture.invalid',username:'fixture',authType:'chain'};
  const saved = {authType:'password',password:'',hasPassword:true};
  for (const [authenticationFactors,expected] of [
    [[],false],[[{authType:'password',password:''}],false],[[saved],true],
    [[{authType:'content',keyContent:'',hasKeyContent:false}],false],
    [[{authType:'content',keyContent:'',hasKeyContent:true}],true],
    [[{authType:'file',keyPath:' '}],false],[Array(7).fill(saved),false],
  ]) assert.equal(canSave({...form,authenticationFactors}),expected);
});

for (const file of ['components/settings/ConnectionsSettings.tsx','components/remote/RemoteSidebar.tsx']) {
  test(`${file}: chain save carries factor-specific secrets and editing preserves opaque metadata`, async () => {
    const noop=()=>{};
    for (const editing of [false,true]) {
      const calls=[];
      const form={name:'fixture',host:'fixture.invalid',port:'22',username:'fixture',authType:'chain',password:'stale-single-password',keyPath:'stale-single-path',knownHostsPath:'',hostKeyPolicy:'strict',authenticationFactors:[
        {authType:'password',password:'chain-password',keyPath:'',keyContent:'',certificatePath:''},
        {id:'key-id',authType:'content',password:'chain-key-unlock',keyPath:'',keyContent:'chain-private-body',hasKeyContent:true,certificatePath:''},
      ]};
      await production(file,'handleSave',{
        form,editingId:editing?'id':null,managerMode:editing?'edit':'create',editingConn:{id:'id'},
        sshCreateConnection:async(...args)=>{calls.push(args);return {id:'saved'};},sshUpdateConnection:async(...args)=>calls.push(args),
        closeModal:noop,load:noop,loadConnections:noop,setForm:noop,setEditingConn:noop,setManagerMode:noop,handleSelect:noop,emptyForm:{},
        normalizeSshError:error=>error,message:{error:error=>{throw Error(error);}},t:key=>key,
      })();
      assert.equal(calls.length,1);
      const offset=Number(editing);
      assert.equal(calls[0][4+offset],'chain');
      assert.equal(calls[0][5+offset],undefined);
      assert.equal(calls[0][6+offset],undefined);
      const factors=calls[0][11+offset];
      assert.equal(factors?.length,2);
      assert.equal(factors[0].password,'chain-password');
      assert.equal(factors[0].passphrase,undefined);
      assert.equal(factors[1].passphrase,'chain-key-unlock');
      assert.equal(factors[1].password,undefined);
      assert.equal(factors[1].id,'key-id');
    }
    let form;
    production(file,file.includes('RemoteSidebar')?'startEdit':'openEdit',{
      setForm:value=>{form=value;},setEditingId:noop,setIsModalOpen:noop,setTestResult:noop,setEditingConn:noop,setManagerMode:noop,
    })({id:'id',name:'fixture',host:'fixture.invalid',port:22,username:'fixture',auth_type:'chain',authentication_factors:[
      {id:'opaque',auth_type:'key_content',has_key_content:true,has_passphrase:true,key_content:'must-not-read',passphrase:'must-not-read'},
    ]});
    assert.equal(form.authenticationFactors?.[0].id,'opaque');
    assert.equal(form.authenticationFactors[0].hasKeyContent,true);
    assert.equal(form.authenticationFactors[0].password,'');
    assert.equal(form.authenticationFactors[0].keyContent,'');
  });
}

test('chain save IPC carries secrets but browser DTOs only project presence flags', async () => {
  for (const name of ['sshCreateConnection','sshUpdateConnection']) {
    const factors=[{auth_type:'password',password:'chain-password-only'},{auth_type:'key_content',key_content:'chain-private-only',passphrase:'chain-unlock-only'}];
    const args=['fixture','fixture.invalid',22,'fixture','chain',undefined,undefined,'','strict',undefined,undefined,factors];
    if (name.includes('Update'))args.unshift('id');
    const calls=[];
    await production('api.ts',name,{getInvoke:()=>async(_command,input)=>{calls.push(input);return {id:'saved'};}})(...args);
    assert.equal(calls[0].authenticationFactors?.[0].password,'chain-password-only');
    const dto=await production('api.ts',name,{getInvoke:()=>null})(...args);
    assert.equal(dto.authentication_factors?.[0].has_password,true);
    assert.equal(dto.authentication_factors[1].has_key_content,true);
    assert.ok(!JSON.stringify(dto).includes('chain-password-only'));
    assert.ok(!JSON.stringify(dto).includes('chain-private-only'));
    assert.ok(!JSON.stringify(dto).includes('chain-unlock-only'));
  }
});

test('private-key content crosses only the save IPC and is never echoed in a connection DTO', async () => {
  for (const name of ['sshCreateConnection', 'sshUpdateConnection']) {
    const calls = [];
    const args = ['fixture', '127.0.0.1', 2222, 'fixture', 'key_content', undefined, undefined, '', 'strict', '', 'fixture-private-content'];
    if (name.includes('Update')) args.unshift('id');
    await production('api.ts', name, { getInvoke: () => async (_command, input) => { calls.push(input); return { id: 'saved' }; } })(...args);
    assert.equal(calls[0].keyContent, 'fixture-private-content');
    const wire = await production('api.ts', name, { getInvoke: () => null })(...args);
    assert.equal(wire.has_key_content, true);
    assert.ok(!JSON.stringify(wire).includes('fixture-private-content'));
  }
});

for (const file of ['components/settings/ConnectionsSettings.tsx', 'components/remote/RemoteSidebar.tsx']) {
  test(`${file}: private-key content saves use the explicit source and edits never read back the secret`, async () => {
    for (const editing of [false, true]) {
      const calls = [];
      const form = { name: 'fixture', host: '127.0.0.1', port: '2222', username: 'fixture', authType: 'content', keyPath: 'stale-path', keyContent: 'fixture-private-content', hasKeyContent: true, certificatePath: 'identity-cert.pub', password: 'fixture-unlock-only', knownHostsPath: '', hostKeyPolicy: 'strict' };
      const noop = () => {};
      await production(file, 'handleSave', {
        form, editingId: editing ? 'id' : null, managerMode: editing ? 'edit' : 'create', editingConn: { id: 'id' },
        sshCreateConnection: async (...args) => { calls.push(args); return { id: 'created' }; }, sshUpdateConnection: async (...args) => { calls.push(args); },
        closeModal: noop, load: noop, loadConnections: noop, setForm: noop, setEditingConn: noop, setManagerMode: noop, handleSelect: noop, emptyForm: {},
        normalizeSshError: error => error, message: { error: error => { throw Error(error); } }, t: key => key,
      })();
      const offset = Number(editing);
      assert.equal(calls[0][4 + offset], 'key_content');
      assert.equal(calls[0][5 + offset], undefined);
      assert.equal(calls[0][6 + offset], 'fixture-unlock-only');
      assert.equal(calls[0][9 + offset], 'identity-cert.pub');
      assert.equal(calls[0][10 + offset], 'fixture-private-content');
    }
    let form;
    production(file, file.includes('RemoteSidebar') ? 'startEdit' : 'openEdit', {
      setForm: value => { form = value; }, setEditingId() {}, setIsModalOpen() {}, setTestResult() {}, setEditingConn() {}, setManagerMode() {},
    })({ id: 'id', name: 'fixture', host: '127.0.0.1', port: 22, username: 'fixture', auth_type: 'key_content', has_key_content: true });
    assert.equal(form.authType, 'content');
    assert.equal(form.keyContent, '');
    assert.equal(form.hasKeyContent, true);
    assert.equal(form.password, '');
  });
}

test('changing a chain identity clears its certificate and unlock without changing other factors', () => {
  const passthrough = ({ children }) => React.createElement('div', null, children);
  const Fields = production('components/remote/SshAuthenticationFields.tsx', 'SshAuthenticationFields', {
    React, useId: () => 'fixture', useState: () => [null, () => {}], useEffect() {}, useTranslation: () => ({ t: key => key }),
    Select: passthrough, SelectTrigger: passthrough, SelectContent: passthrough, SelectItem: passthrough, SelectValue: passthrough,
    Label: passthrough, Button: passthrough, Input: passthrough, Textarea: passthrough,
  });
  for (const [authType, change] of [['file', { keyPath: 'new-identity' }], ['content', { keyContent: 'new-private-body' }]]) {
    const first = { id: 'opaque-key', authType, keyPath: 'old-identity', keyContent: 'old-private-body', hasKeyContent: true,
      certificatePath: 'old-identity-cert.pub', password: 'old-unlock-only', hasPassphrase: true, clearPassphrase: true };
    const second = { id: 'opaque-password', authType: 'password', password: '', hasPassword: true };
    const changes = [];
    const tree = Fields({ value: { authType: 'chain', authenticationFactors: [first, second] },
      onChange: value => changes.push(value), onPickKeyFile() {}, onPickCertificateFile() {} });
    const factors = [];
    const visit = element => {
      if (!React.isValidElement(element)) return;
      if (element.type === Fields) factors.push(element);
      React.Children.forEach(element.props.children, visit);
    };
    visit(tree);
    assert.equal(factors.length, 2);
    factors[0].props.onChange(change);
    const changed = changes[0].authenticationFactors;
    assert.equal(changed[0].id, first.id);
    assert.equal(changed[0].certificatePath, '');
    assert.equal(changed[0].password, '');
    assert.equal(changed[0].hasPassphrase, false);
    assert.equal(changed[0].clearPassphrase, false);
    assert.equal(changed[1], second);
  }
});

test('private-key content uses the shared multiline control and switching sources clears all credential fields', () => {
  let index = 0;
  const states = [0, null];
  const changes = [];
  const passthrough = ({ children }) => React.createElement('div', null, children);
  const Input = props => React.createElement('input', props);
  const Textarea = props => React.createElement('textarea', props);
  const Select = props => React.createElement('section', null, props.children);
  const Fields = production('components/remote/SshAuthenticationFields.tsx', 'SshAuthenticationFields', {
    React, useId: () => 'fixture', useState: () => [states[index++], () => {}], useEffect() {}, useTranslation: () => ({ t: key => key }),
    Select, SelectTrigger: passthrough, SelectContent: passthrough, SelectItem: passthrough, SelectValue: passthrough,
    Label: passthrough, Button: passthrough, Input, Textarea,
  });
  const tree = Fields({ value: { authType: 'content', keyPath: '', keyContent: '', hasKeyContent: true, certificatePath: '', password: '' }, onChange: value => changes.push(value), onPickKeyFile() {}, onPickCertificateFile() {} });
  const elements = [];
  const visit = element => {
    if (!React.isValidElement(element)) return;
    elements.push(element);
    React.Children.forEach(element.props.children, visit);
  };
  visit(tree);
  const content = elements.find(element => element.type === Textarea);
  assert.ok(content, 'private-key content must not be mistaken for an Agent provider');
  assert.equal(content.props.autoComplete, 'off');
  assert.equal(content.props.spellCheck, false);
  content.props.onChange({ target: { value: 'fixture-private-content' } });
  assert.equal(changes[0].keyContent, 'fixture-private-content');
  assert.ok(elements.some(element => element.type === Input && element.props.type === 'password'));
  elements.find(element => element.type === Select).props.onValueChange('file');
  assert.equal(changes[1].keyContent, '');
  assert.equal(changes[1].hasKeyContent, false);
  assert.equal(changes[1].password, '');
  assert.equal(changes[1].certificatePath, '');
});

for (const file of ['components/settings/ConnectionsSettings.tsx', 'components/remote/RemoteSidebar.tsx']) {
  test(`${file}: encrypted private key saves forward its passphrase for create and edit`, async () => {
    for (const editing of [false, true]) {
      const calls = [];
      const form = { name: 'fixture', host: '127.0.0.1', port: '2222', username: 'fixture', authType: 'file', keyPath: 'identity', certificatePath: 'identity-cert.pub', password: 'fixture-unlock-only', knownHostsPath: '', hostKeyPolicy: 'strict' };
      const noop = () => {};
      await production(file, 'handleSave', {
        form, editingId: editing ? 'id' : null, managerMode: editing ? 'edit' : 'create', editingConn: { id: 'id' },
        sshCreateConnection: async (...args) => { calls.push(args); return { id: 'created' }; }, sshUpdateConnection: async (...args) => { calls.push(args); },
        closeModal: noop, load: noop, loadConnections: noop, setForm: noop, setEditingConn: noop, setManagerMode: noop, handleSelect: noop, emptyForm: {},
        normalizeSshError: error => error, message: { error: error => { throw Error(error); } }, t: key => key,
      })();
      assert.equal(calls[0][editing ? 7 : 6], form.password);
    }
  });
}

test('private key fields provide a masked passphrase control and clear it on method changes', () => {
  let index = 0;
  const states = [0, null];
  const changes = [];
  const passthrough = ({ children }) => React.createElement('div', null, children);
  const Input = props => React.createElement('input', props);
  const Select = props => React.createElement('section', null, props.children);
  const Fields = production('components/remote/SshAuthenticationFields.tsx', 'SshAuthenticationFields', {
    React, useId: () => 'fixture', useState: () => [states[index++], () => {}], useEffect() {}, useTranslation: () => ({ t: key => key }),
    Select, SelectTrigger: passthrough, SelectContent: passthrough, SelectItem: passthrough, SelectValue: passthrough,
    Label: passthrough, Button: passthrough, Input,
  });
  const tree = Fields({ value: { authType: 'file', keyPath: 'identity', certificatePath: '', password: '' }, onChange: value => changes.push(value), onPickKeyFile() {}, onPickCertificateFile() {} });
  const elements = [];
  const visit = element => {
    if (!React.isValidElement(element)) return;
    elements.push(element);
    React.Children.forEach(element.props.children, visit);
  };
  visit(tree);
  const passphrase = elements.find(element => element.type === Input && element.props.type === 'password');
  assert.ok(passphrase, 'encrypted key must have an unlock control');
  assert.equal(passphrase.props.autoComplete, 'new-password');
  passphrase.props.onChange({ target: { value: 'temporary-fixture-unlock' } });
  assert.equal(changes[0].password, 'temporary-fixture-unlock');
  elements.find(element => element.type === Select).props.onValueChange('agent');
  assert.equal(changes[1].password, '');
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

test('default identity discovery requires native IPC and never invents browser candidates', async () => {
  await assert.rejects(production('api.ts', 'sshDiscoverDefaultIdentities', { getInvoke: () => null })(), /desktop|桌面/i);
  const calls = [];
  const expected = [{ name: 'id_ed25519', path: 'D:/fixture/.ssh/id_ed25519', state: 'available' }];
  const result = await production('api.ts', 'sshDiscoverDefaultIdentities', {
    getInvoke: () => async (...args) => { calls.push(args); return expected; },
  })();
  assert.equal(calls[0][0], 'ssh_discover_default_identities');
  assert.equal(calls[0][1], undefined, 'frontend cannot substitute a home directory');
  assert.deepEqual(result, expected);
});

function defaultIdentityPicker(discover) {
  let state;
  let cleanup;
  const request = { current: 0 };
  const selected = [];
  const Picker = production('components/remote/SshDefaultIdentityPicker.tsx', 'SshDefaultIdentityPicker', {
    React, useTranslation: () => ({ t: key => key }), useId: () => 'default-identity-picker',
    useState: initial => [state ??= initial, next => { state = next; }],
    useRef: () => request,
    useEffect: effect => { cleanup ??= effect(); },
    sshDiscoverDefaultIdentities: discover,
    Button: 'button', Select: 'select', SelectTrigger: 'trigger', SelectContent: 'content',
    SelectItem: 'option', SelectValue: 'value', Label: 'label',
  });
  return {
    render() {
      const elements = [];
      const visit = element => {
        if (!React.isValidElement(element)) return;
        elements.push(element);
        React.Children.forEach(element.props.children, visit);
      };
      visit(Picker({ value: '', onSelect: value => selected.push(value) }));
      return elements;
    },
    state: () => state, selected, unmount: () => cleanup?.(),
  };
}

test('default identity picker waits for an explicit usable choice and preserves error explanations', async () => {
  let resolve;
  const picker = defaultIdentityPicker(() => new Promise(done => { resolve = done; }));
  const button = picker.render().find(element => element.type === 'button');
  assert.ok(button, 'discovery must be user initiated');
  const pending = button.props.onClick();
  assert.equal(picker.state().status, 'loading');
  assert.equal(picker.render().find(element => element.type === 'button').props.disabled, true);
  resolve([
    { name: 'id_ed25519', path: 'D:/fixture/.ssh/id_ed25519', state: 'available' },
    { name: 'id_rsa', path: 'D:/fixture/.ssh/id_rsa', state: 'unavailable', reason: 'fixture read denied' },
    { name: 'id_ecdsa', path: 'D:/fixture/.ssh/id_ecdsa', state: 'missing' },
  ]);
  await pending;
  assert.equal(picker.selected.length, 0);
  const elements = picker.render();
  assert.equal(elements.filter(element => element.type === 'option').length, 1);
  assert.ok(elements.some(element => element.props.children === 'id_rsa: fixture read denied'));
  const select = elements.find(element => element.type === 'select');
  select.props.onValueChange('D:/fixture/.ssh/id_rsa');
  assert.equal(picker.selected.length, 0, 'an unusable row cannot be selected');
  select.props.onValueChange('D:/fixture/.ssh/id_ed25519');
  assert.deepEqual(picker.selected, ['D:/fixture/.ssh/id_ed25519']);
});

test('default identity picker reports rejection and ignores a result after unmount', async () => {
  const failed = defaultIdentityPicker(async () => { throw Error('native discovery unavailable'); });
  await failed.render().find(element => element.type === 'button').props.onClick();
  assert.equal(failed.state().status, 'error');
  assert.match(failed.state().error, /native discovery unavailable/);
  let resolve;
  const stale = defaultIdentityPicker(() => new Promise(done => { resolve = done; }));
  const pending = stale.render().find(element => element.type === 'button').props.onClick();
  stale.unmount();
  resolve([]);
  await pending;
  assert.equal(stale.state().status, 'loading', 'unmounted requests cannot publish a result');
});

test('default identity selection in shared fields clears credentials only when the identity changes', () => {
  for (const next of ['old-key', 'D:/fixture/.ssh/id_ed25519']) {
    const changes = [];
    const passthrough = ({ children }) => React.createElement('div', null, children);
    const Picker = () => null;
    const Fields = production('components/remote/SshAuthenticationFields.tsx', 'SshAuthenticationFields', {
      React, useId: () => 'fixture', useState: initial => [initial, () => {}], useEffect() {}, useTranslation: () => ({ t: key => key }),
      SshDefaultIdentityPicker: Picker, Select: passthrough, SelectTrigger: passthrough, SelectContent: passthrough,
      SelectItem: passthrough, SelectValue: passthrough, Label: passthrough, Button: passthrough, Input: passthrough,
    });
    const elements = [];
    const visit = element => {
      if (!React.isValidElement(element)) return;
      elements.push(element);
      React.Children.forEach(element.props.children, visit);
    };
    visit(Fields({ value: { authType: 'file', keyPath: 'old-key', password: 'old-unlock', hasPassphrase: true, certificatePath: 'old-cert.pub' },
      onChange: value => changes.push(value), onPickKeyFile() {}, onPickCertificateFile() {} }));
    const picker = elements.find(element => element.type === Picker);
    assert.ok(picker, 'both editors and nested factors need the shared picker');
    picker.props.onSelect(next);
    assert.equal(changes[0].keyPath, next);
    if (next === 'old-key') {
      assert.equal(changes[0].password, undefined, 'same identity preserves the saved credential');
    } else {
      assert.equal(changes[0].password, '');
      assert.equal(changes[0].certificatePath, '');
      assert.equal(changes[0].hasPassphrase, false);
      assert.equal(changes[0].clearPassphrase, true);
    }
  }
});
