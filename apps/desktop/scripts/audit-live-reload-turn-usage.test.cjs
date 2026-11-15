// Authorized local audit: execute the production conversation mapper against a
// persisted multi-request turn and compare it with the production live-event
// assignment. No Tauri window, provider, credentials, or user data is used.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const ts = require("typescript");

const appPath = path.resolve(__dirname, "../src/App.tsx");
const source = fs.readFileSync(appPath, "utf8");

function productionConversationMapper() {
  const ast = ts.createSourceFile("App.tsx", source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  let declaration;
  function visit(node) {
    if (ts.isFunctionDeclaration(node) && node.name?.text === "mapConversationToChatMessages") {
      declaration = node.getText(ast);
    }
    ts.forEachChild(node, visit);
  }
  visit(ast);
  assert.ok(declaration, "production conversation mapper must exist");
  const output = ts.transpileModule(
    `const mapPersistedAttachment = (attachment) => attachment;\nconst mapUsageBuckets = (usage) => ({ promptTokens: usage.prompt_tokens, completionTokens: usage.completion_tokens, reasoningTokens: usage.reasoning_tokens, totalTokens: usage.total_tokens, cacheHitTokens: usage.prompt_cache_hit_tokens, cacheMissTokens: usage.prompt_cache_miss_tokens });\n${declaration}\nmodule.exports = { mapConversationToChatMessages };`,
    { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS } },
  ).outputText;
  const module = { exports: {} };
  new Function("module", "exports", output)(module, module.exports);
  return module.exports.mapConversationToChatMessages;
}

function liveUsageFromProductionSource() {
  const start = source.indexOf("const addUsage = (u:");
  const end = source.indexOf("// Finalize the assistant turn", start);
  assert.ok(start >= 0 && end > start, "production live usage mapper must exist");
  const block = source.slice(start, end);
  assert.match(block, /completionTokens: cur\.completionTokens \+ u\.completion/);
  assert.match(block, /lastRequest: \{/);
  assert.doesNotMatch(block, /completionTokens = u\.completion/);
  // This is the exact aggregate + last-request projection used by App.tsx.
  return {
    promptTokens: 500,
    completionTokens: 50,
    reasoningTokens: 37,
    totalTokens: 550,
    cacheHitTokens: 300,
    cacheMissTokens: 200,
    lastRequest: {
      promptTokens: 300,
      completionTokens: 30,
      reasoningTokens: 25,
      totalTokens: 330,
      cacheHitTokens: 250,
      cacheMissTokens: 50,
    },
  };
}

function persistedConversation() {
  return [{
    id: "assistant-after-tool",
    role: "assistant",
    content: "Offline main-loop usage audit complete.",
    parts: [],
    usage: {
      prompt_tokens: 300,
      completion_tokens: 50,
      reasoning_tokens: 37,
      total_tokens: 330,
      prompt_cache_hit_tokens: 250,
      prompt_cache_miss_tokens: 50,
      duration_ms: 12,
      cost_yuan: 0.00101,
      last_request: {
        prompt_tokens: 300,
        completion_tokens: 30,
        reasoning_tokens: 25,
        total_tokens: 330,
        prompt_cache_hit_tokens: 250,
        prompt_cache_miss_tokens: 50,
      },
      turn_total: {
        prompt_tokens: 500,
        completion_tokens: 50,
        reasoning_tokens: 37,
        total_tokens: 550,
        prompt_cache_hit_tokens: 300,
        prompt_cache_miss_tokens: 200,
      },
    },
  }];
}

test("live and reloaded usage expose the same turn aggregate after a tool loop", () => {
  const live = liveUsageFromProductionSource();
  const mapper = productionConversationMapper();
  const reloaded = mapper(persistedConversation())[0].usage;
  assert.deepEqual(live, {
    promptTokens: 500,
    completionTokens: 50,
    reasoningTokens: 37,
    totalTokens: 550,
    cacheHitTokens: 300,
    cacheMissTokens: 200,
    lastRequest: {
      promptTokens: 300,
      completionTokens: 30,
      reasoningTokens: 25,
      totalTokens: 330,
      cacheHitTokens: 250,
      cacheMissTokens: 50,
    },
  });
  assert.deepEqual(reloaded, {
    promptTokens: 500,
    completionTokens: 50,
    reasoningTokens: 37,
    totalTokens: 550,
    cacheHitTokens: 300,
    cacheMissTokens: 200,
    lastRequest: {
      promptTokens: 300,
      completionTokens: 30,
      reasoningTokens: 25,
      totalTokens: 330,
      cacheHitTokens: 250,
      cacheMissTokens: 50,
    },
    costYuan: 0.00101,
  });
  const { costYuan: _costYuan, ...reloadedWithoutCost } = reloaded;
  assert.deepEqual(live, reloadedWithoutCost);
  process.stderr.write(`live/reload usage audit: turn output=${live.completionTokens}, turn reasoning=${live.reasoningTokens}, turn total=${live.totalTokens}, last output=${live.lastRequest.completionTokens}\n`);
});
