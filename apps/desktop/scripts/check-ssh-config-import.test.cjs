const assert = require('node:assert/strict');
const { test } = require('node:test');
const { readFileSync } = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const ts = require('typescript');

function load(file, imports = {}) {
  const source = readFileSync(path.join(__dirname, '../src', file), 'utf8');
  const module = { exports: {} };
  vm.runInNewContext(ts.transpileModule(source, { compilerOptions: {
    module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.ReactJSX,
  } }).outputText, { exports: module.exports, module, require: name => imports[name] ?? {}, console });
  return module.exports;
}

function productionFunction(file, name, context) {
  const source = readFileSync(path.join(__dirname, '../src',file), 'utf8');
  const ast = ts.createSourceFile(file, source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  let node;
  function visit(candidate) {
    if ((ts.isFunctionDeclaration(candidate) || ts.isVariableDeclaration(candidate)) && candidate.name?.getText(ast) === name) node=candidate;
    ts.forEachChild(candidate, visit);
  }
  visit(ast);
  assert.ok(node);
  const declaration=node.getText(ast).replace(/^export\s+/, '');
  return vm.runInNewContext(ts.transpileModule(`${ts.isVariableDeclaration(node)?'const ':''}${declaration};\n${name};`, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
  }).outputText, context);
}
const apiFunction=(name,context)=>productionFunction('api.ts',name,context);

const endpoint = (host = 'imported.invalid', keys = []) => ({ host, port:2202, username:'imported-user', identity_files:keys, default_identities:keys.length === 0 });
function form() {
  return { name:'', host:'', port:'22', username:'', authType:'password', keyPath:'', keyContent:'',
    hasKeyContent:false, certificatePath:'', password:'', authenticationFactors:[], agentFingerprint:'',
    knownHostsPath:'trust-file', hostKeyPolicy:'strict', proxy:{mode:'socks5',host:'proxy'}, timing:{connectTimeout:'10'},
    jump:{ enabled:false, host:'', port:'22', username:'', authType:'password', keyPath:'', keyContent:'',
      hasKeyContent:false, certificatePath:'', password:'', authenticationFactors:[], knownHostsPath:'jump-trust',hostKeyPolicy:'strict' } };
}
const helpers = () => load('components/remote/SshConfigImport.tsx');

test('config import requires desktop IPC and forwards only the declared request', async () => {
  await assert.rejects(apiFunction('sshPreviewConfigImport', {getInvoke:()=>null})(), /desktop/i);
  const calls=[];
  await apiFunction('sshPreviewConfigImport', {getInvoke:()=>async (...args)=>{calls.push(args);return {aliases:[]};}})('D:/config','app',{host:'override',port:22});
  assert.equal(calls[0][0], 'ssh_preview_config_import');
  assert.equal(JSON.stringify(calls[0][1]), JSON.stringify({path:'D:/config',alias:'app',overrides:{host:'override',port:22}}));
});

test('explicit connection fields override imports without converting default port 22 into an override', () => {
  const {sshConfigOverridesFromForm:overrides}=helpers();
  assert.equal(overrides(form()).port, undefined);
  const value={...form(),host:'explicit.invalid',username:'explicit',importPortExplicit:true,importAuthExplicit:true,importJumpExplicit:true};
  assert.equal(JSON.stringify(overrides(value)),JSON.stringify({host:'explicit.invalid',username:'explicit',port:22,identity_files:[],proxy_jump:'none'}));
});

test('invalid explicit ports cannot become null in JSON and silently inherit the imported port',()=>{
  for(const port of ['invalid','22.5','0','65536','']) {
    assert.throws(()=>helpers().sshConfigOverridesFromForm({...form(),port,importPortExplicit:true}),/port|端口/i);
  }
});

test('all imported identities reach the common authentication chain and independent jump', () => {
  const {applySshConfigImport:apply}=helpers();
  const original=form();
  const updated=apply(original,{alias:'app',target:endpoint('target.invalid',['D:/first','D:/second']),jump:endpoint('jump.invalid',['D:/jump'])});
  assert.equal(updated.host,'target.invalid');
  assert.equal(updated.port,'2202');
  assert.equal(updated.name,'app');
  assert.equal(updated.authType,'chain');
  assert.equal(updated.authenticationFactors.map(f=>f.keyPath).join(','),'D:/first,D:/second');
  assert.equal(updated.jump.keyPath,'D:/jump');
  assert.equal(updated.jump.authType,'file');
  assert.equal(updated.jump.knownHostsPath,'jump-trust');
  assert.equal(updated.hostKeyPolicy,'strict');
  assert.equal(updated.proxy,original.proxy);
  assert.equal(updated.timing,original.timing);
  assert.equal(original.host,'');
});

test('IdentityFile none cannot re-enable default private files', () => {
  const updated=helpers().applySshConfigImport(form(),{alias:'app',target:{...endpoint(),default_identities:false}});
  assert.equal(updated.authType,'agent');
  assert.equal(updated.keyPath,'');
});

test('explicit authentication is retained while endpoint changes clear stale secrets', () => {
  const original={...form(),host:'same.invalid',port:'2202',username:'imported-user',authType:'file',keyPath:'D:/chosen',password:'secret',certificatePath:'D:/cert',importAuthExplicit:true};
  const apply=helpers().applySshConfigImport;
  const same=apply(original,{alias:'app',target:endpoint('same.invalid',['D:/imported'])});
  assert.equal(same.keyPath,'D:/chosen');
  assert.equal(same.password,'secret');
  const changed=apply(original,{alias:'app',target:endpoint('other.invalid',['D:/imported'])});
  assert.equal(changed.keyPath,'D:/chosen');
  assert.equal(changed.password,'');
  assert.equal(changed.certificatePath,'');
});

for(const [file,callback] of [['components/settings/ConnectionsSettings.tsx','openEdit'],['components/remote/RemoteSidebar.tsx','startEdit']]) {
  test(`${file}: editing treats saved port, authentication and route as explicit fields`,()=>{
    let value;
    const noop=()=>{};
    productionFunction(file,callback,{
      setForm:next=>{value=next;},setEditingId:noop,setIsModalOpen:noop,setTestResult:noop,setEditingConn:noop,setManagerMode:noop,
      authenticationFactorsFromDto:()=>[],sshProxyFromDto:()=>({}),sshJumpFromDto:()=>({}),sshTimingFromDto:()=>({}),
    })({id:'fixture',host:'saved.invalid',port:22,username:'saved',auth_type:'agent'});
    assert.equal(value.importPortExplicit,true);
    assert.equal(value.importAuthExplicit,true);
    assert.equal(value.importJumpExplicit,true);
  });
}

function importer(invoke) {
  const React=require('react');
  const slots=[];
  let cursor=0,cleanup;
  const applied=[];
  const hooks={
    useId:()=> 'config-import-fixture',
    useState:initial=>{const i=cursor++;if(!(i in slots))slots[i]=initial;return [slots[i],next=>{slots[i]=typeof next==='function'?next(slots[i]):next;}];},
    useRef:initial=>{const i=cursor++;return slots[i]??={current:initial};},
    useEffect:effect=>{cleanup??=effect();},
  };
  const Importer=load('components/remote/SshConfigImport.tsx',{
    react:hooks,'react/jsx-runtime':require('react/jsx-runtime'),'react-i18next':{useTranslation:()=>({t:key=>key})},
    '../../api':{sshPreviewConfigImport:invoke,pickSshIdentityFile:async()=>null},
    '../shadcn/button':{Button:'button'},'../shadcn/input':{Input:'input'},'../shadcn/label':{Label:'label'},
    '../shadcn/select':{Select:'select',SelectContent:'content',SelectItem:'option',SelectTrigger:'trigger',SelectValue:'value'},
  }).SshConfigImporter;
  return {
    render(value=form()) {
      cursor=0;
      const elements=[];
      function visit(element) {
        if(!React.isValidElement(element))return;
        elements.push(element);React.Children.forEach(element.props.children,visit);
      }
      visit(Importer({value,onApply:next=>applied.push(next)}));
      return elements;
    },
    unmount:()=>cleanup?.(),applied,
  };
}
const flush=()=>new Promise(resolve=>setImmediate(resolve));
const button=(elements,key)=>elements.find(element=>element.type==='button'&&element.props.children===`settings.connections.configImport.${key}`);

test('import preview only applies explicitly and a changed form invalidates the exact preview',async()=>{
  const calls=[];
  const component=importer(async(...args)=>{calls.push(args);return {path:'D:/config',aliases:['app'],connection:{alias:'app',target:endpoint()}};});
  button(component.render(),'preview').props.onClick();
  assert.equal(button(component.render(),'loading').props.disabled,true);
  await flush();
  assert.equal(component.applied.length,0);
  const oldApply=button(component.render(),'apply');
  assert.ok(oldApply);
  component.render({...form(),username:'changed'});
  oldApply.props.onClick();
  assert.equal(component.applied.length,0,'a retained old callback cannot apply stale fields');
  button(component.render(),'apply').props.onClick();
  assert.equal(component.applied[0].host,'imported.invalid');
  assert.equal(calls.length,1);
});

test('late results are discarded after alias changes or unmount',async()=>{
  for(const unmount of [false,true]) {
    let complete;
    const component=importer(()=>new Promise(resolve=>{complete=resolve;}));
    button(component.render(),'preview').props.onClick();
    if(unmount)component.unmount();
    else {
      component.render().find(element=>element.type==='input'&&element.props.id.endsWith('-alias')).props.onChange({target:{value:'changed'}});
      component.render();
    }
    complete({path:'D:/config',aliases:['old'],connection:{alias:'old',target:endpoint()}});
    await flush();
    assert.equal(button(component.render(),'apply'),undefined);
    assert.equal(component.applied.length,0);
  }
});

test('errors belong to the previewed input and disappear after that input changes',async()=>{
  const component=importer(async()=>{throw Error('line 2: unsupported proxycommand');});
  button(component.render(),'preview').props.onClick();
  await flush();
  assert.ok(component.render().some(element=>String(element.props.children).includes('line 2: unsupported')));
  component.render().find(element=>element.type==='input'&&element.props.id.endsWith('-alias')).props.onChange({target:{value:'another'}});
  assert.equal(component.render().some(element=>String(element.props.children).includes('line 2: unsupported')),false);
});

test('a resolved literal alias remains visible before opening its menu',async()=>{
  const component=importer(async()=>({path:'D:/config',aliases:['app'],connection:null}));
  component.render().find(element=>element.type==='input'&&element.props.id.endsWith('-alias')).props.onChange({target:{value:'app'}});
  button(component.render(),'preview').props.onClick();
  await flush();
  assert.equal(component.render().find(element=>element.type==='value').props.placeholder,'app');
});
