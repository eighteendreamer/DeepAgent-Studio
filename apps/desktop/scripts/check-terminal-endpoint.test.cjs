const assert = require('node:assert/strict');
const { test } = require('node:test');
const { readFileSync } = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const ts = require('typescript');
const React = require('react');
const { renderToStaticMarkup } = require('react-dom/server');

function workspace(context) {
  const file = path.join(__dirname, '../src/components/terminal/TerminalWorkspace.tsx');
  const ast = ts.createSourceFile(file, readFileSync(file, 'utf8'), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const node = ast.statements.find(node => ts.isFunctionDeclaration(node) && node.name?.text === 'TerminalWorkspace');
  assert.ok(node);
  const source = node.getText(ast).replace(/^export\s+/, '') + '\nTerminalWorkspace;';
  return vm.runInNewContext(ts.transpileModule(source, { compilerOptions: {
    target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.React,
  } }).outputText, { React, useTranslation: () => ({ t: key => key }),
    scopeOf: () => 'local', useState: () => { throw new Error('unexpected session initialization'); }, ...context });
}

test('remote mode without a selected connection never initializes a local terminal workspace', () => {
  const Workspace = workspace({ TerminalWorkspaceSession: () => { throw new Error('unexpected session initialization'); } });
  for (const connectionId of [undefined, null, '']) {
    const html = renderToStaticMarkup(React.createElement(Workspace, { mode: 'remote', connectionId }));
    assert.match(html, /remote.emptyTitle/);
  }
});

test('local and selected remote modes preserve workspace props and isolate endpoint changes', () => {
  const calls = [];
  const Workspace = workspace({ TerminalWorkspaceSession: props => { calls.push(props); return null; } });
  for (const props of [{}, { mode: 'local', active: false }, { mode: 'remote', connectionId: 'fixture', active: true }]) {
    renderToStaticMarkup(React.createElement(Workspace, props));
    assert.deepEqual(calls.at(-1), props);
  }
  const local = Workspace({ mode: 'local' }), remote = Workspace({ mode: 'remote', connectionId: 'fixture' });
  assert.notEqual(local.key, remote.key);
});
