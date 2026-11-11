const assert = require("node:assert/strict");
const { test } = require("node:test");
const { readFileSync } = require("node:fs");
const path = require("node:path");
const Module = require("node:module");
const ts = require("typescript");

function deferred() {
  let resolve, reject;
  const promise = new Promise((ok, fail) => { resolve = ok; reject = fail; });
  return { promise, resolve, reject };
}

// Production handlers + hooks, with no network or DOM/file-viewer replacement.
function mount() {
  const cells = [], effects = [], imports = [], previews = [], writes = [];
  let cursor = 0, initial = true, tree, value = [], nodeId = "llm";
  const hooks = {
    useState(initialValue) {
      const index = cursor++;
      if (!(index in cells)) cells[index] = typeof initialValue === "function" ? initialValue() : initialValue;
      return [cells[index], next => { cells[index] = typeof next === "function" ? next(cells[index]) : next; }];
    },
    useRef(initialValue) { return hooks.useState(() => ({ current: initialValue }))[0]; },
    useEffect(effect) { if (initial) effects.push(effect); },
  };
  const filename = path.resolve(__dirname, "../src/canvas/workflow/components/DocumentAttachments.tsx");
  const compiled = ts.transpileModule(readFileSync(filename, "utf8"), {
    fileName: filename, compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.ReactJSX },
  });
  const loaded = new Module(filename, module);
  loaded.filename = filename; loaded.paths = Module._nodeModulePaths(path.dirname(filename));
  const requireNative = loaded.require.bind(loaded);
  const components = new Proxy({}, { get: (_, name) => name });
  loaded.require = name => {
    if (name === "react") return hooks;
    if (name.includes("/shadcn/")) return components;
    if (name === "./CanvasFields") return { CANVAS_BUTTON_CLASS: "" };
    if (name.endsWith("/FilePreviewPlugin")) return { PreviewBody: "PreviewBody" };
    if (name === "../utils/canvasMedia") return { importCanvasMediaFile: (kind, file, workspace) => {
      const gate = deferred(); imports.push({ kind, file, workspace, ...gate }); return gate.promise;
    } };
    if (name === "@tauri-apps/api/core") return { invoke: (command, args) => {
      const gate = deferred(); previews.push({ command, args, ...gate }); return gate.promise;
    } };
    return requireNative(name);
  };
  loaded._compile(compiled.outputText, filename);
  const { DocumentAttachments } = loaded.exports;
  function render() {
    cursor = 0;
    tree = DocumentAttachments({ nodeId, value, onChange: next => { value = next; writes.push(next); } });
    initial = false; return tree;
  }
  function all() {
    const nodes = [];
    function visit(node) {
      if (Array.isArray(node)) return node.forEach(visit);
      if (!node?.props) return;
      nodes.push(node); visit(node.props.children);
    }
    visit(tree); return nodes;
  }
  render(); const cleanup = effects.map(effect => effect());
  return { imports, previews, writes, render, all,
    setValue(next) { value = next; render(); },
    unmount() { cleanup.forEach(fn => fn?.()); },
    upload(files) { return all().find(node => node.type === "input").props.onChange({ currentTarget: { files, value: "picked" } }); },
    button(label) { const node = all().find(node => node.props["aria-label"] === label); assert.ok(node, label); return node.props; },
  };
}
const file = (name = "one.txt", size = 5) => ({ name, size });

test("H179 validates the complete selection before importing any bytes", async () => {
  for (const files of [[file("old.doc")], [file("big.txt", 10*1024*1024+1)], [file("empty.md",0)], Array.from({length:6},()=>file()), [file(),file("bad.exe")]]) {
    const ui = mount(); await ui.upload(files); ui.render();
    assert.equal(ui.imports.length,0); assert.equal(ui.writes.length,0);
    assert.ok(ui.all().some(node=>node.props.role === "alert"));
  }
});

test("H179 stores only selected artifact references and preview uses original bytes without a model request", async () => {
  const ui = mount(); const pending = ui.upload([file()]);
  assert.equal(ui.imports.length,1); assert.equal(ui.imports[0].kind,"document");
  ui.imports[0].resolve("artifact://one"); await pending; ui.render();
  assert.deepEqual(ui.writes,[[{reference:"artifact://one",name:"one.txt"}]]);
  const preview = ui.button("预览 one.txt").onClick();
  assert.deepEqual(ui.previews[0].args,{reference:"artifact://one",workspaceId:null});
  assert.equal(ui.previews[0].command,"canvas_document_preview");
  ui.previews[0].resolve("data:text/plain;base64,b3JpZ2luYWw="); await preview; ui.render();
  const viewer=ui.all().find(node=>node.type==="PreviewBody");
  assert.ok(viewer); assert.equal(await viewer.props.fileBlob.text(),"original");
  assert.equal(ui.writes.length,1,"preview bytes never persist in graph");
  ui.button("移除 one.txt").onClick(); ui.render(); assert.deepEqual(ui.writes.at(-1),[]);
});

test("H179 failed batches and stale uploads never overwrite the node selection", async () => {
  const ui=mount(); const pending=ui.upload([file()]); ui.imports[0].reject(Error("synthetic import failure"));
  await pending; ui.render(); assert.equal(ui.writes.length,0);
  const next=ui.upload([file()]); ui.unmount(); ui.imports[1].resolve("artifact://stale"); await next;
  assert.equal(ui.writes.length,0);
  const changed=mount(); const inflight=changed.upload([file()]);
  changed.setValue([{reference:"artifact://new",name:"new.txt"}]);
  changed.imports[0].resolve("artifact://late"); await inflight; assert.equal(changed.writes.length,0);
});

test("H179 late preview responses cannot reopen a dismissed preview", async () => {
  const ui=mount(); ui.setValue([{reference:"artifact://one",name:"one.txt"}]);
  const pending=ui.button("预览 one.txt").onClick(); ui.render();
  ui.all().find(node=>node.type==="Dialog").props.onOpenChange(false);
  ui.previews[0].resolve("data:text/plain;base64,b3JpZ2luYWw="); await pending; ui.render();
  assert.equal(ui.all().find(node=>node.type==="Dialog").props.open,false);
  assert.ok(!ui.all().some(node=>node.type==="PreviewBody"));
});

test("H179 invalid restored attachment entries remain removable instead of crashing settings", async () => {
  const ui=mount(); ui.setValue([null,{reference:17,name:false}]);
  assert.equal(ui.all().filter(node=>node.props.role==="alert").length,2);
  assert.equal(ui.previews.length,0);
  ui.button("移除无效文档 1").onClick(); ui.render();
  assert.equal(ui.writes[0].length,1);
});
