const assert = require('node:assert/strict');
const { test } = require('node:test');
const fs = require('node:fs/promises');
const os = require('node:os');
const path = require('node:path');

test('Vite watches frontend edits without watching native resources or copied viewer assets', async () => {
  const { createServer, resolveConfig } = await import('vite');
  const config = await resolveConfig({ configFile: path.resolve(__dirname, '../vite.config.ts') }, 'serve');
  const root = await fs.mkdtemp(path.join(os.tmpdir(), 'deepagent-watch-'));
  let server;
  try {
    for (const dir of ['src', 'src-tauri/resources/skills', 'public/file-viewer']) {
      await fs.mkdir(path.join(root, dir), { recursive: true });
    }
    const frontend = path.join(root, 'src/view.ts');
    const native = path.join(root, 'src-tauri/resources/skills/backend-only.rs');
    const asset = path.join(root, 'public/file-viewer/copied.js');
    await Promise.all([frontend, native, asset].map(file => fs.writeFile(file, '// disposable watch fixture\n')));
    server = await createServer({
      configFile: false, root, logLevel: 'silent',
      cacheDir: path.join(root, '.vite'), publicDir: false,
      server: { middlewareMode: true, watch: config.server.watch },
      optimizeDeps: { noDiscovery: true },
    });
    const watched = () => Object.entries(server.watcher.getWatched()).flatMap(([dir, entries]) => entries.map(entry => path.join(dir, entry)));
    const began = Date.now();
    while (!watched().includes(frontend)) {
      assert.ok(Date.now() - began < 5000, 'frontend source must be watched');
      await new Promise(resolve => setTimeout(resolve, 20));
    }
    const observed = watched();
    assert.ok(!observed.includes(native), 'native resources must not enter the frontend watcher');
    assert.ok(!observed.includes(asset), 'copied viewer assets must remain excluded');
  } finally {
    await server?.close();
    await fs.rm(root, { recursive: true, force: true });
  }
});
