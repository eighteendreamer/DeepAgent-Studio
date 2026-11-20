// One-time asset import. Runtime and release builds use only resources/pets.
const fs = require("node:fs");
const path = require("node:path");
const crypto = require("node:crypto");
const ts = require("typescript");

const source = process.argv[2];
if (!source) throw new Error("Usage: node scripts/import-bundled-pet.cjs <dsh-pet repository>");
const root = path.resolve(source);
const assets = path.join(root, "dsh-pet/assets");
const output = path.resolve(__dirname, "../src-tauri/resources/pets/dsh-pet");
const parsed = ts.parseConfigFileTextToJson("config.jsonc", fs.readFileSync(path.join(assets, "config.jsonc"), "utf8"));
if (parsed.error) throw new Error("Invalid source pet configuration");
const config = parsed.config;
const previews = new Map([...fs.readFileSync(path.join(root, "scripts/encode_preview_gifs.py"), "utf8")
  .matchAll(/^\s+"([^"]+)": "([^"]+)",?$/gm)].map(match => [match[1], match[2]]));
const files = [];
function copy(from, relative) {
  const bytes = fs.readFileSync(from);
  const target = path.join(output, relative);
  fs.mkdirSync(path.dirname(target), { recursive: true });
  fs.writeFileSync(target, bytes);
  files.push({ path: relative, bytes: bytes.length, sha256: crypto.createHash("sha256").update(bytes).digest("hex") });
}
const groups = [
  { id: "idle", actions: config.animations.idle },
  { id: "turn", actions: config.animations.turn },
  { id: "drag", actions: config.animations.drag },
  { id: "click", actions: config.animations.clicks },
  { id: "move", actions: config.animations.moves.actions.map(action => action.name) },
  ...config.animations.categories.map(category => ({ id: category.id, actions: category.actions })),
  ...Object.entries(config.animations.events).map(([id, actions]) => ({ id, actions })),
  { id: "sleep", actions: config.animations.sleep.flatMap(sleep => [...sleep.start, ...sleep.loop, ...sleep.end]) },
  { id: "baifan", actions: config.animations.baifan },
];
const work = config.animations.events.workStatus;
const states = {
  idle: { actions: config.animations.idle, loop: true },
  thinking: { actions: [work[0]], loop: true, fallback: "idle" },
  working: { actions: [work[1]], loop: true, fallback: "idle" },
  result: { actions: [work[2]], loop: true, fallback: "working" },
  waiting: { actions: [work[3]], loop: true, fallback: "idle" },
  success: { actions: [work[4]], loop: false, fallback: "idle" },
  error: { actions: [work[5]], loop: false, fallback: "idle" },
  drag: { actions: config.animations.drag, loop: true, fallback: "idle" },
  click: { actions: config.animations.clicks, loop: false, fallback: "idle" },
  sleep: { actions: config.animations.sleep[0].loop, loop: true, fallback: "idle" },
};
const actions = {};
for (const filename of fs.readdirSync(path.join(assets, "webm")).sort()) {
  if (!filename.endsWith(".webm")) continue;
  const name = filename.slice(0, -5);
  const previewName = previews.get(name);
  const preview = previewName && fs.existsSync(path.join(assets, "preview", previewName + ".gif"))
    ? `preview/${previewName}.gif` : null;
  actions[name] = { file: `webm/${filename}`, preview, width: 640, height: 360 };
  copy(path.join(assets, "webm", filename), `webm/${filename}`);
  if (preview) copy(path.join(assets, preview), preview);
}
copy(path.join(assets, "main-character.png"), "character.png");
copy(path.join(root, "LICENSE"), "LICENSE.code.txt");
const attribution = `# dsh-pet 素材来源\n\n原项目：https://github.com/PC2005-cloud/dsh-pet\n\n角色：蓝毛小女仆（社区二创的 DeepSeek「大肥鱼」，非 DeepSeek 官方形象）。\n原型「溟月」：B 站 @上善无形；女仆鲸鱼娘设计：B 站 @ZipZipPipe。\n\n代码：MIT（见 LICENSE.code.txt）。角色形象及素材按原项目 README 的授权说明处理：\nCC BY-NC-SA 4.0（署名、非商业、相同方式共享）。\n形象授权声明：https://www.bilibili.com/opus/1247156596114456581\n\n本资源包保留原角色、动画和预览图；未包含原项目的模型、会话、提示词或 Electron 宿主实现。\n`;
fs.writeFileSync(path.join(output, "ATTRIBUTION.md"), attribution);
const attributionBytes = Buffer.from(attribution);
files.push({ path: "ATTRIBUTION.md", bytes: attributionBytes.length, sha256: crypto.createHash("sha256").update(attributionBytes).digest("hex") });
const catalog = {
  version: 1, packId: "dsh-pet", resourceVersion: "1",
  characters: [{
    id: "blue-maid", name: config.pets[0].name,
    thumbnail: "character.png", preview: actions[config.animations.idle[0]].preview,
    canvas: { width: 640, height: 360 },
    hitBox: { x: 200, y: 50, width: 240, height: 285 },
    defaultSize: config.pets[0].size,
    actions, states, groups,
  }],
  files,
};
fs.writeFileSync(path.join(output, "catalog.json"), JSON.stringify(catalog, null, 2) + "\n");
console.log(`Imported ${catalog.characters.length} character, ${Object.keys(actions).length} animations, ${files.length} files.`);
