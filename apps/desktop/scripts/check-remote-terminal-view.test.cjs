const assert = require('node:assert/strict');
const { test } = require('node:test');
const { readFileSync } = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const ts = require('typescript');
const React = require('react');
const { renderToStaticMarkup } = require('react-dom/server');
const file = path.join(__dirname, '../src/components/remote/RemoteView.tsx');
const ast = ts.createSourceFile(file, readFileSync(file, 'utf8'), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);

function production(name, context) {
  let found;
  function visit(node) { if ((ts.isFunctionDeclaration(node) || ts.isVariableDeclaration(node)) && node.name?.getText(ast) === name) found = node; ts.forEachChild(node, visit); }
  visit(ast); assert.ok(found, name);
  let source = found.getText(ast).replace(/^export\s+/, '');
  if (ts.isVariableDeclaration(found)) source = `const ${source};`;
  return vm.runInNewContext(ts.transpileModule(`${source}\n${name};`, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.React } }).outputText, context);
}

test('inactive terminal tabs stay mounted even when a file preview is selected', () => {
  for (const active of ['terminal-1', 'terminal-2', '/fixture.txt']) {
    for (const forwarding of [false, true]) {
    let index = 0;
    const states = [null, undefined, forwarding, [{ id: 'terminal-1', name: 'one' }, { id: 'terminal-2', name: 'two' }], [{ id: '/fixture.txt', connectionId: 'fixture', path: '/fixture.txt', name: 'fixture.txt' }], active, { '/fixture.txt': { status: 'loaded', kind: 'text', content: 'fixture text', truncated: false } }];
    const pass = ({ children }) => React.createElement('div', null, children);
    const context = {
      React, useState: () => [states[index++], () => {}], useRef: () => ({ current: 1 }), useEffect() {}, useTranslation: () => ({ t: key => key }),
      HoverInfo: pass, ContextMenu: pass, ContextMenuTrigger: pass, ContextMenuContent: pass, ContextMenuItem: pass,
      Button: pass, SshForwardPanel: () => React.createElement('output', { 'data-forwarding': true }),
      TerminalPlugin: ({ connectionId, active }) => React.createElement('output', { 'data-connection': connectionId, 'data-active': active }, 'terminal fixture'),
      FileWorkspace: () => null, FileOperationDialog: () => null, ExternalEditorPanel: () => null,
      FloatingChat: () => null, RemoteTransfersPanel: () => null, SyntaxHighlightedCode: () => null, PreviewBody: () => null, languageForFile: () => 'text',
    };
    for (const name of ['Server', 'Terminal', 'Plus', 'X', 'FileText', 'Loader2']) context[name] = () => null;
    const View = production('RemoteView', context);
    const html = renderToStaticMarkup(React.createElement(View, { connection: { id: 'fixture', status: 'connected' }, openFile: null }));
    assert.equal((html.match(/data-connection="fixture"/g) ?? []).length, 2);
    assert.equal((html.match(/data-active="true"/g) ?? []).length, active.startsWith('terminal-') ? 1 : 0);
    assert.equal((html.match(/data-forwarding="true"/g) ?? []).length, forwarding ? 1 : 0);
    }
  }
});

test('adding a terminal does not mutate counters or active state inside a replayable updater', () => {
  const counter = { current: 1 }; let updater; const selected = [];
  const add = production('addTerminalTab', { terminalCounter: counter, t: key => key, setTerminalTabs: value => { updater = value; }, setActiveKey: key => selected.push(key) });
  add();
  const initial = [{ id: 'terminal-1' }];
  const first = updater(initial); const replay = updater(initial);
  assert.equal(counter.current, 2);
  assert.equal(first[1].id, replay[1].id);
  assert.equal(selected.length, 1);
});
