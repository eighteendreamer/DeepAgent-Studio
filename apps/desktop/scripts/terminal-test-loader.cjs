const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const ts = require('typescript');
const { File } = require('node:buffer');

function terminalLoader(overrides = {}) {
  const cache = new Map();
  function load(name) {
    if (cache.has(name)) return cache.get(name).exports;
    const file = path.join(__dirname, '../src/components/terminal', name + '.ts');
    const module = { exports: {} }; cache.set(name, module);
    const source = fs.readFileSync(file, 'utf8').replace('import.meta.url', JSON.stringify(require('node:url').pathToFileURL(file).href));
    const code = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, esModuleInterop: true } }).outputText;
    vm.runInNewContext(code, {
      module, exports: module.exports, require: id => id.startsWith('./') ? load(id.slice(2)) : require(id),
      URL, TextDecoder, TextEncoder, Uint8Array, performance, Date, setTimeout, clearTimeout,
      crypto: require('node:crypto').webcrypto, btoa, atob, AbortController, Blob, File, ...overrides,
    }, { filename: file });
    return module.exports;
  }
  return load;
}
module.exports = { terminalLoader };
