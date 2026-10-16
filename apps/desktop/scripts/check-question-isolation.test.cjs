const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const { join } = require("node:path");
const test = require("node:test");
const vm = require("node:vm");
const ts = require("typescript");

const source = readFileSync(join(__dirname, "../src/questionQueue.ts"), "utf8");
const compiled = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
}).outputText;
const moduleExports = {};
vm.runInNewContext(compiled, { exports: moduleExports, module: { exports: moduleExports } });
const {
  clearQuestionSession,
  enqueueQuestion,
  questionsForSession,
  removeQuestion,
} = moduleExports;

const request = (session_id, question_id) => ({
  session_id,
  question_id,
  questions: [],
});

test("question queues stay isolated by session and preserve FIFO order", () => {
  let queues = new Map();
  queues = enqueueQuestion(queues, request("A", "a1"));
  queues = enqueueQuestion(queues, request("B", "b1"));
  queues = enqueueQuestion(queues, request("A", "a2"));
  queues = enqueueQuestion(queues, request("A", "a2"));

  assert.deepEqual(Array.from(questionsForSession(queues, "A"), (item) => item.question_id), ["a1", "a2"]);
  assert.deepEqual(Array.from(questionsForSession(queues, "B"), (item) => item.question_id), ["b1"]);
  assert.equal(questionsForSession(queues, "C").length, 0);
});

test("answering, stopping, or completing one session does not clear another", () => {
  let queues = new Map([
    ["A", [request("A", "a1")]],
    ["B", [request("B", "b1")]],
  ]);
  queues = removeQuestion(queues, "A", "a1");
  assert.deepEqual(Array.from(questionsForSession(queues, "B"), (item) => item.question_id), ["b1"]);
  queues = clearQuestionSession(queues, "A");
  assert.deepEqual(Array.from(questionsForSession(queues, "B"), (item) => item.question_id), ["b1"]);
  queues = clearQuestionSession(queues, "B");
  assert.equal(questionsForSession(queues, "B").length, 0);
});
