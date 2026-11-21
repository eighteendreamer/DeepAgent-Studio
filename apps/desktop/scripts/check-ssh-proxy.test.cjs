const assert = require('node:assert/strict');
const { test } = require('node:test');
const { readFileSync } = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const ts = require('typescript');
const React = require('react');
const root = path.resolve(__dirname, '../src');

test('all host-trust DTOs accept the complete SSH route contract and reject unknown routes', () => {
  const fixture = path.resolve(__dirname, 'ssh-route-contract.fixture.ts');
  const source = `
    import type { SshRoute, SshHostTrustInspection, SshKnownHostsImportPreview,
      SshHostTrustAudit, SshHostKeyChallenge } from '../src/api';
    declare const route: SshRoute;
    const inspection: SshHostTrustInspection['route'] = route;
    const preview: SshKnownHostsImportPreview['route'] = route;
    const audit: SshHostTrustAudit['route'] = route;
    const challenge: SshHostKeyChallenge['route'] = route;
    // @ts-expect-error An unverified route is not a supported transport.
    const invalidInspection: SshHostTrustInspection['route'] = 'unverified';
    // @ts-expect-error Import previews must also reject unknown transports.
    const invalidPreview: SshKnownHostsImportPreview['route'] = 'unverified';
  `;
  const options = { noEmit:true, strict:true, skipLibCheck:true, types:[], jsx:ts.JsxEmit.ReactJSX,
    target:ts.ScriptTarget.ES2022, module:ts.ModuleKind.ESNext, moduleResolution:ts.ModuleResolutionKind.Bundler,
    allowSyntheticDefaultImports:true };
  const host = ts.createCompilerHost(options);
  const readSource = host.getSourceFile.bind(host);
  host.getSourceFile = (file, languageVersion, ...rest) => path.resolve(file) === fixture
    ? ts.createSourceFile(file, source, languageVersion, true)
    : readSource(file, languageVersion, ...rest);
  const program = ts.createProgram([fixture], options, host);
  const diagnostics = program.getSemanticDiagnostics(program.getSourceFile(fixture));
  assert.deepEqual(diagnostics.map(item => ts.flattenDiagnosticMessageText(item.messageText, '\n')), []);
});

function production(file, name, context = {}) {
  const source = readFileSync(path.join(root, file), 'utf8');
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
  return vm.runInNewContext(ts.transpileModule(`${text}\n${name};`, { compilerOptions: {
    module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.React,
  } }).outputText, {
    canSaveSshConnection: value => production('components/remote/SshAuthenticationFields.tsx', 'canSaveSshConnection')(value),
    canSaveSshProxy: value => production('components/remote/SshProxyFields.tsx', 'canSaveSshProxy')(value),
    canSaveSshJump: value => production('components/remote/SshJumpFields.tsx', 'canSaveSshJump')(value),
    sshJumpRequest: value => production('components/remote/SshJumpFields.tsx', 'sshJumpRequest')(value),
    sshJumpFromDto: value => production('components/remote/SshJumpFields.tsx', 'sshJumpFromDto')(value),
    canSaveSshTiming: value => production('components/remote/SshTimingFields.tsx', 'canSaveSshTiming')(value),
    sshTimingRequest: value => production('components/remote/SshTimingFields.tsx', 'sshTimingRequest')(value),
    sshTimingFromDto: value => production('components/remote/SshTimingFields.tsx', 'sshTimingFromDto')(value),
    authenticationFactorRequests: value => production('components/remote/SshAuthenticationFields.tsx', 'authenticationFactorRequests')(value),
    authenticationFactorsFromDto: value => production('components/remote/SshAuthenticationFields.tsx', 'authenticationFactorsFromDto')(value),
    sshProxyRequest: value => production('components/remote/SshProxyFields.tsx', 'sshProxyRequest')(value),
    sshProxyFromDto: value => production('components/remote/SshProxyFields.tsx', 'sshProxyFromDto')(value),
    mockProxy: value => production('api.ts', 'mockProxy')(value),
    mockJump: value => production('api.ts', 'mockJump')(value),
    isSshRoute: value => production('components/remote/SshProxyFields.tsx', 'isSshRoute')(value),
    mockAuthenticationFactors: value => production('api.ts', 'mockAuthenticationFactors')(value),
    TextEncoder,
    React,
    ...context,
  });
}

test('connection save validation rejects an incomplete proxy instead of silently connecting directly', () => {
  const validate = production('components/remote/SshAuthenticationFields.tsx', 'canSaveSshConnection');
  const base = { name: 'fixture', host: 'fixture.invalid', username: 'fixture', authType: 'password', password: 'ssh-fixture' };
  assert.equal(validate({ ...base, proxy: { mode: 'http_connect', host: '', port: '8080' } }), false);
  assert.equal(validate({ ...base, proxy: { mode: 'socks5', host: 'proxy.invalid', port: '0' } }), false);
});

for (const file of ['components/settings/ConnectionsSettings.tsx', 'components/remote/RemoteSidebar.tsx']) {
  test(`${file}: create/edit submits proxy secrets separately and restores only presence metadata`, async () => {
    const noop = () => {};
    for (const editing of [false, true]) {
      const calls = [];
      const form = { name: 'fixture', host: 'fixture.invalid', port: '22', username: 'fixture', authType: 'password',
        password: 'ssh-fixture', knownHostsPath: '', hostKeyPolicy: 'strict', proxy: {
          mode: 'http_connect', host: 'proxy.invalid', port: '8080', username: 'proxy-user', password: 'proxy-secret', hasPassword: false,
        } };
      await production(file, 'handleSave', {
        form, editingId: editing ? 'id' : null, managerMode: editing ? 'edit' : 'create', editingConn: { id: 'id' },
        sshCreateConnection: async (...args) => { calls.push(args); return { id: 'created' }; }, sshUpdateConnection: async (...args) => calls.push(args),
        closeModal: noop, load: noop, loadConnections: noop, setForm: noop, setEditingConn: noop, setManagerMode: noop, handleSelect: noop, emptyForm: {},
        normalizeSshError: error => error, message: { error: error => { throw Error(error); } }, t: key => key,
      })();
      assert.equal(calls[0][12 + Number(editing)]?.password, 'proxy-secret');
      assert.equal(calls[0][6 + Number(editing)], 'ssh-fixture');
    }
    let restored;
    production(file, file.includes('RemoteSidebar') ? 'startEdit' : 'openEdit', {
      setForm: value => { restored = value; }, setEditingId: noop, setIsModalOpen: noop, setTestResult: noop, setEditingConn: noop, setManagerMode: noop,
      authenticationFactorsFromDto: () => [],
    })({ id: 'id', name: 'fixture', host: 'fixture.invalid', port: 22, username: 'fixture', auth_type: 'password', proxy: {
      mode: 'socks5', host: 'proxy.invalid', port: 1080, username: 'proxy-user', has_password: true, password: 'must-not-read',
    } });
    assert.equal(restored.proxy?.password, '');
    assert.equal(restored.proxy.hasPassword, true);
    assert.equal(restored.proxy.host, 'proxy.invalid');
  });
}

test('proxy save IPC includes the requested secret but browser DTOs never echo it', async () => {
  for (const name of ['sshCreateConnection', 'sshUpdateConnection']) {
    const proxy = { mode: 'socks5', host: 'proxy.invalid', port: 1080, username: 'fixture', password: 'proxy-secret-only' };
    const args = ['fixture', 'fixture.invalid', 22, 'fixture', 'password', undefined, 'ssh-fixture', '', 'strict', undefined, undefined, undefined, proxy];
    if (name.includes('Update')) args.unshift('id');
    const calls = [];
    await production('api.ts', name, { getInvoke: () => async (_command, input) => { calls.push(input); return { id: 'saved' }; } })(...args);
    assert.equal(calls[0].proxy?.password, 'proxy-secret-only');
    const dto = await production('api.ts', name, { getInvoke: () => null })(...args);
    assert.equal(dto.proxy?.has_password, true);
    assert.ok(!JSON.stringify(dto).includes('proxy-secret-only'));
  }
});

test('SOCKS credentials use byte bounds and editing an endpoint discards saved credential presence', () => {
  const validate = production('components/remote/SshProxyFields.tsx', 'canSaveSshProxy');
  const base = { mode: 'socks5', host: 'proxy.invalid', port: '1080', username: '用'.repeat(85), password: '密'.repeat(85), hasPassword: false };
  assert.equal(validate(base), true);
  assert.equal(validate({ ...base, username: '用'.repeat(86) }), false);
  assert.equal(validate({ ...base, password: '密'.repeat(86) }), false);
  assert.equal(validate({ ...base, password: '', hasPassword: true }), true);
  assert.equal(validate({ ...base, password: '', hasPassword: true, clearPassword: true }), false);
  const changes = [];
  const fields = production('components/remote/SshProxyFields.tsx', 'SshProxyFields', {
    useId: () => 'proxy', useTranslation: () => ({ t: key => key }), Button: 'button', Input: 'input', Label: 'label',
    Select: 'select', SelectContent: 'option-list', SelectItem: 'option', SelectTrigger: 'trigger', SelectValue: 'selected',
  });
  const nodes = [];
  const visit = node => { if (!node || typeof node !== 'object') return; if (Array.isArray(node)) return node.forEach(visit); nodes.push(node); visit(node.props?.children); };
  visit(fields({ value: { ...base, password: '', hasPassword: true }, onChange: value => changes.push(value) }));
  nodes.find(node => node.props?.id === 'proxy-host').props.onChange({ target: { value: 'other.invalid' } });
  assert.equal(changes.at(-1).hasPassword, false);
  assert.equal(changes.at(-1).password, '');
  nodes.find(node => node.type === 'button').props.onClick();
  assert.equal(production('components/remote/SshProxyFields.tsx', 'sshProxyRequest')(changes.at(-1)).password, '');
});

test('jump save IPC and browser projections separate the two credential sets', async () => {
  for (const name of ['sshCreateConnection', 'sshUpdateConnection']) {
    const jump = {mode:'ssh', host:'jump.invalid', port:2222, username:'jump-user', auth_type:'password', password:'jump-secret-only'};
    const args = ['fixture', 'target.invalid', 22, 'target-user', 'password', undefined, 'target-secret-only', '', 'strict', undefined, undefined, undefined, undefined, jump];
    if (name.includes('Update')) args.unshift('id');
    const calls = [];
    await production('api.ts', name, {getInvoke:() => async (_, input) => {calls.push(input); return {id:'saved'}}})(...args);
    assert.equal(calls[0].jump?.password, 'jump-secret-only');
    assert.equal(calls[0].password, 'target-secret-only');
    const dto = await production('api.ts', name, {getInvoke:() => null})(...args);
    assert.equal(dto.jump?.host, 'jump.invalid');
    assert.equal(dto.jump?.has_password, true);
    assert.ok(!JSON.stringify(dto).includes('jump-secret-only'));
  }
});

test('jump route matching rejects missing, foreign or hidden jump endpoints', () => {
  const jump = {host:'jump.invalid', port:2222, username:'jump-user'};
  const matches = production('components/remote/SshProxyFields.tsx', 'matchesSshRoute');
  const valid = production('components/remote/SshProxyFields.tsx', 'isSshRoute');
  assert.equal(valid({route:'jump', jump}), true);
  assert.equal(valid({route:'jump'}), false);
  assert.equal(valid({route:'direct', jump}), false);
  assert.equal(matches({jump}, {route:'jump', jump}), true);
  assert.equal(matches({jump}, {route:'jump', jump:{...jump, port:2200}}), false);
  assert.equal(matches({jump}, {route:'direct'}), false);
});

test('the shared flow accepts a verified jump-scoped host prompt before the target', async () => {
  const states = [];
  const challenge = {connection_id:'id', attempt_id:'attempt', challenge_id:'jump', role:'jump', kind:'unknown_host',
    host:'jump.invalid', port:2222, username:'jump-user', route:'jump', jump:{host:'jump.invalid', port:2222, username:'jump-user'}, can_save:false, expires_at_ms:1000};
  const factory = production('components/remote/SshConnectionFlow.ts', 'createSshConnectionFlow');
  let cancelled = 0;
  const flow = factory(state => states.push(state), {
    begin:async () => ({status:'confirmation_required', challenge}),
    respond:async () => ({status:'connected'}), cancel:async () => {cancelled++},
    respondAuthentication:async () => ({status:'connected'}), cancelAuthentication:async () => {},
  }, () => 10);
  const completion = flow.connect('id');
  const checked = completion.catch(error => error);
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(states.at(-1)?.challenge?.role, 'jump');
  await flow.cancel();
  assert.equal(await checked, false);
  assert.equal(cancelled, 1);
});

const jumpFields = 'components/remote/SshJumpFields.tsx';
const jumpForm = () => ({enabled:true, host:'jump.invalid', port:'2222', username:'jump-user', authType:'password',
  password:'jump-secret', hasPassword:false, keyPath:'', keyContent:'', hasKeyContent:false, certificatePath:'',
  knownHostsPath:'D:/fixtures/jump-known-hosts', hostKeyPolicy:'strict', authenticationFactors:[]});

test('connection validation rejects an incomplete jump identity and its missing credentials', () => {
  const validate = production('components/remote/SshAuthenticationFields.tsx', 'canSaveSshConnection');
  const base = {name:'target', host:'target.invalid', username:'target-user', authType:'password', password:'target-secret'};
  assert.equal(validate({...base, jump:jumpForm()}), true);
  for (const change of [{host:''}, {host:'bad/host'}, {port:'0'}, {port:'22.5'}, {port:'65536'}, {username:''},
    {password:'', hasPassword:false}, {authType:'content', keyContent:'', hasKeyContent:false}, {authType:'file', keyPath:''}]) {
    assert.equal(validate({...base, jump:{...jumpForm(), ...change}}), false, JSON.stringify(change));
  }
  assert.equal(validate({...base, jump:{...jumpForm(), password:'', hasPassword:true}}), true);
  assert.equal(validate({...base, jump:{...jumpForm(), enabled:false, host:'', username:''}}), true);
});

for (const file of ['components/settings/ConnectionsSettings.tsx', 'components/remote/RemoteSidebar.tsx']) {
  test(`${file}: both editor actions save the jump independently and restore no secrets`, async () => {
    const noop = () => {};
    for (const editing of [false, true]) {
      const calls = [];
      await production(file, 'handleSave', {
        form:{name:'target', host:'target.invalid', port:'22', username:'target-user', authType:'password',
          password:'target-secret', knownHostsPath:'', hostKeyPolicy:'strict', jump:jumpForm()},
        editingId:editing ? 'id' : null, managerMode:editing ? 'edit' : 'create', editingConn:{id:'id'},
        sshCreateConnection:async (...args) => {calls.push(args); return {id:'created'}}, sshUpdateConnection:async (...args) => calls.push(args),
        closeModal:noop, load:noop, loadConnections:noop, setForm:noop, setEditingConn:noop, setManagerMode:noop, handleSelect:noop, emptyForm:{},
        normalizeSshError:error=>error, message:{error:error=>{throw Error(error)}}, t:key=>key,
      })();
      assert.equal(calls[0][13 + Number(editing)]?.password, 'jump-secret');
      assert.equal(calls[0][6 + Number(editing)], 'target-secret');
    }
    let restored;
    production(file, file.includes('RemoteSidebar') ? 'startEdit' : 'openEdit', {
      setForm:value=>{restored=value}, setEditingId:noop, setIsModalOpen:noop, setTestResult:noop, setEditingConn:noop, setManagerMode:noop,
    })({id:'id', name:'target', host:'target.invalid', port:22, username:'target-user', auth_type:'password',
      jump:{host:'jump.invalid', port:2222, username:'jump-user', auth_type:'password', has_password:true, password:'do-not-read',
        key_content:'do-not-read', known_hosts_path:'jump-known-hosts', host_key_policy:'accept_new', authentication_factors:[]}});
    assert.equal(restored.jump.password, '');
    assert.equal(restored.jump.keyContent, '');
    assert.equal(restored.jump.hasPassword, true);
    assert.equal(restored.jump.hostKeyPolicy, 'accept_new');
    assert.equal(restored.jump.knownHostsPath, 'jump-known-hosts');
    assert.ok(!JSON.stringify(restored).includes('do-not-read'));
  });
}

test('jump request mapping preserves only omitted credentials and sends direct explicitly', () => {
  const request = production(jumpFields, 'sshJumpRequest');
  const fromDto = production(jumpFields, 'sshJumpFromDto');
  assert.equal(request({...jumpForm(), enabled:false}).mode, 'direct');
  const saved = fromDto({host:'jump.invalid', port:22, username:'jump-user', auth_type:'key_content', has_password:true,
    has_key_content:true, key_content:'do-not-read', password:'do-not-read', authentication_factors:[]});
  assert.equal(saved.password, '');
  assert.equal(saved.keyContent, '');
  assert.equal(saved.hasPassphrase, true);
  assert.equal(request(saved).password, undefined);
  assert.equal(request(saved).key_content, undefined);
  assert.equal(request({...saved, clearPassphrase:true}).password, '');
  const chained = request({...jumpForm(), authType:'chain', password:'unused', authenticationFactors:[
    {id:'jump-factor', authType:'password', password:'factor-secret', keyPath:'', keyContent:'', certificatePath:''}]});
  assert.equal(chained.password, undefined);
  assert.equal(chained.authentication_factors[0].password, 'factor-secret');
});

function jumpNodes(value, changes) {
  const fields = production(jumpFields, 'SshJumpFields', {
    useId:()=> 'jump', useTranslation:()=>({t:key=>key}), pickSshIdentityFile:async()=>null,
    SshAuthenticationFields:'auth', SshHostTrustField:'trust', Button:'button', Input:'input', Label:'label',
    Select:'select', SelectContent:'options', SelectItem:'option', SelectTrigger:'trigger', SelectValue:'selected',
  });
  const nodes=[];
  const visit = node => {if (!node || typeof node !== 'object') return; if(Array.isArray(node))return node.forEach(visit); nodes.push(node); visit(node.props?.children)};
  visit(fields({value, onChange:change=>changes.push(change)}));
  return nodes;
}

test('jump fields reuse authentication and discard saved credentials when endpoint or identity changes', () => {
  const changes=[];
  const value={...jumpForm(), password:'', hasPassword:true, hasPassphrase:true, hasKeyContent:true,
    authenticationFactors:[{id:'old', password:'', hasPassword:true}]};
  const nodes=jumpNodes(value, changes);
  nodes.find(node=>node.props?.id==='jump-host').props.onChange({target:{value:'other.invalid'}});
  assert.equal(changes.at(-1).hasPassword, false);
  assert.equal(changes.at(-1).hasKeyContent, false);
  assert.equal(changes.at(-1).authenticationFactors[0].id, undefined);
  assert.equal(changes.at(-1).authenticationFactors[0].hasPassword, false);
  nodes.find(node=>node.type==='auth').props.onChange({keyContent:'replacement-private-key'});
  assert.equal(changes.at(-1).password, '');
  assert.equal(changes.at(-1).hasPassphrase, false);
  assert.equal(changes.at(-1).certificatePath, '');
  const trust=nodes.find(node=>node.type==='trust');
  assert.equal(trust.props.host, 'jump.invalid');
  trust.props.onPolicyChange('accept_new');
  assert.equal(changes.at(-1).hostKeyPolicy, 'accept_new');
  assert.equal(changes.at(-1).hasPassword, true);
});

test('all trust surfaces render the verified jump route and reset on a jump change', () => {
  const paths=['SshHostTrustManager.tsx','SshHostTrustAuditDialog.tsx','SshHostKeyRotationDialog.tsx','SshHostKeyPrompt.tsx'];
  for(const file of paths) {
    const source=ts.createSourceFile(file, readFileSync(path.join(root,'components/remote',file),'utf8'),ts.ScriptTarget.Latest,true,ts.ScriptKind.TSX);
    let labels=0;
    const visit=node=>{
      if(ts.isJsxSelfClosingElement(node) && node.tagName.getText(source)==='SshRouteLabel'){
        labels++;
        assert.ok(node.attributes.properties.some(prop=>prop.name?.getText(source)==='jump'),file);
      }
      ts.forEachChild(node,visit);
    };visit(source);assert.ok(labels>0,file);
  }
  const manager=production('components/remote/SshHostTrustManager.tsx','SshHostTrustManager',{HostTrustPanel:'panel'});
  const connection={id:'id',host:'target.invalid',port:22,jump:{host:'jump.invalid',port:22,username:'first'}};
  assert.notEqual(manager({connection}).key, manager({connection:{...connection,jump:{...connection.jump,username:'second'}}}).key);
});
