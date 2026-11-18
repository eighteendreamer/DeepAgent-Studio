// Builds the real production renderer with only its API boundary instrumented.
// No Tauri globals, credentials, user database, or provider traffic are used.
const fs = require('node:fs');
const path = require('node:path');
const ts = require('typescript');

function instrument(source, id) {
  if (id.endsWith('/src/harnessModelsApi.ts')) return source.replace(
    /import\s*\{\s*invoke\s*\}\s*from\s*["'][^"']+["'];?/,
    'const invoke = (command, args) => window.__uiAudit.invoke(command, args);');
  if (!id.endsWith('/src/api.ts')) return source;
  const ast = ts.createSourceFile(id, source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
  const edits = [];
  for (const node of ast.statements) {
    if (!ts.isFunctionDeclaration(node) || !node.body || !node.name) continue;
    const name = node.name.text;
    if (name === 'getSettings') edits.push({ start: node.body.getStart(ast), end: node.body.end,
      text: `{ window.__uiAudit.calls.getSettings=(window.__uiAudit.calls.getSettings||0)+1; return {configured:true,available_models:['deepseek-flash'],model_capabilities:[],chat_model:'deepseek-flash',reasoning_effort:'high',sandbox_mode:'none'}; }` });
    else if (node.modifiers?.some(modifier => modifier.kind === ts.SyntaxKind.AsyncKeyword)) edits.push({
      start: node.body.getStart(ast) + 1, end: node.body.getStart(ast) + 1,
      text: `window.__uiAudit.calls[${JSON.stringify(name)}]=(window.__uiAudit.calls[${JSON.stringify(name)}]||0)+1;`,
    });
  }
  for (const edit of edits.sort((a, b) => b.start - a.start)) source = source.slice(0, edit.start) + edit.text + source.slice(edit.end);
  return source;
}

async function buildAudit(label) {
  const { build, preview } = await import('vite');
  const root = path.resolve(__dirname, '..');
  const baseline = path.join(root, 'target/ui-responsiveness-baseline');
  const outDir = path.join(root, 'target/ui-responsiveness', process.env.UI_AUDIT_BUILD_LABEL || label, 'dist');
  if (!process.env.UI_AUDIT_SKIP_BUILD) await build({ root, configFile: path.join(root, 'vite.config.ts'), logLevel: 'warn',
    plugins: [{ name: 'ui-audit-fixture', enforce: 'pre',
      load(id) {
        if (!label.startsWith('before')) return null;
        const relative = path.relative(path.resolve(root, '../..'), id);
        const snapshot = path.join(baseline, relative);
        return relative.startsWith('apps') && fs.existsSync(snapshot) ? fs.readFileSync(snapshot, 'utf8') : null;
      },
      transform(source, id) { const next = instrument(source, id.replace(/\\/g, '/')); return next === source ? null : next; },
    }],
    build: { outDir, copyPublicDir: false, reportCompressedSize: false },
  });
  // Only assets actually needed by the fixtures; exclude large file-viewer bundles.
  fs.cpSync(path.join(root, 'public/providers'), path.join(outDir, 'providers'), { recursive: true });
  return preview({ root, configFile: false, build: { outDir }, preview: { host: '127.0.0.1', port: 1421, strictPort: true } });
}

module.exports = { instrument, buildAudit };
