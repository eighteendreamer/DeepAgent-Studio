const assert = require('node:assert/strict');
const { test } = require('node:test');
const { Worker: NodeWorker } = require('node:worker_threads');
const { terminalLoader } = require('./terminal-test-loader.cjs');

test('broadcast preserves captured targets and reports individual failures without replaying successful targets', async () => {
  const { dispatchTerminalInput } = terminalLoader()('terminalInputDispatch');
  const calls = [], a = { paste: async (...args) => calls.push(['a', ...args]) }, b = { paste: async () => { throw Error('locked'); } };
  const failures = await dispatchTerminalInput([{ id:'a', label:'host A', handle:a }, { id:'b', label:'host B', handle:b }], id => id==='a'?a:b, 'echo hi', false, true);
  assert.equal(calls.length, 1); assert.equal(calls[0][2], true, 'paste and Enter use one operation');
  assert.equal(failures.length, 1); assert.match(failures[0], /host B.*locked/);
  const replaced = await dispatchTerminalInput([{ id:'a', label:'host A', handle:a }], () => ({ paste:async()=>calls.push('wrong') }), 'payload', false, false);
  assert.equal(calls.length, 1); assert.match(replaced[0], /changed/);
});

test('recording enforces shared budgets and discarding releases only its own allocation', () => {
  const load = terminalLoader(), { TerminalBudget } = load('terminalBudget'), { TerminalRecording, parseCast } = load('terminalRecording');
  const budget = new TerminalBudget(300), a = new TerminalRecording(80,24,()=>0,budget), b = new TerminalRecording(80,24,()=>0,budget);
  a.output('x'.repeat(100)); assert.throws(()=>b.output('x'.repeat(100)), /shared/);
  a.dispose(); a.dispose(); b.output('x'.repeat(100)); b.dispose();
  assert.throws(()=>b.output('closed'), /discarded/);
  const invalid = ['{"version":2,"width":0,"height":24}', '{"version":2,"width":80,"height":24}\n[0,"r","99999x24"]', '{"version":2,"width":80,"height":24}\n[-1,"o","bad"]'];
  for (const cast of invalid) assert.throws(()=>parseCast(cast));
});

test('regex uses display cells for emoji and wide characters; disabling clears existing decorations', async () => {
  let pending; const disposed = [];
  class Worker { constructor() { pending=this; } postMessage() {} terminate() {} }
  const { TerminalHighlighter } = terminalLoader({ Worker })('terminalHighlight');
  const chars = [{c:'😀',w:2},{c:'',w:0},{c:'中',w:2},{c:'',w:0},{c:'X',w:1}];
  const row = { length:5, translateToString:()=> '😀中X', getCell:i=>({getChars:()=>chars[i].c,getWidth:()=>chars[i].w}) };
  const decorations=[];
  const term={ cols:80,rows:1, buffer:{active:{type:'normal',viewportY:0,length:1,baseY:0,cursorY:0,getLine:()=>row}},
    registerMarker:()=>({dispose:()=>disposed.push('marker')}),
    registerDecoration:options=>{decorations.push(options);return {dispose:()=>disposed.push('decoration')};} };
  const highlighter=new TerminalHighlighter(term,[{id:'x',pattern:'X',enabled:true,color:'#ef4444'}],message=>assert.fail(message));
  highlighter.render(); pending.onmessage({data:{hits:[{line:0,start:2,end:3,color:'#ef4444'}]}});
  assert.equal(decorations[0].x,2); assert.equal(decorations[0].width,2);
  highlighter.configure([]); assert.equal(disposed.length,2); highlighter.dispose();
});

test('catastrophic regex is terminated in a real worker while the caller stays responsive', { timeout:3000 }, async () => {
  const { findHighlights } = terminalLoader()('highlightRegex');
  const workers = [];
  class Worker {
    constructor() {
      this.inner = new NodeWorker("const {parentPort}=require('node:worker_threads'); const find="+findHighlights.toString()+"; parentPort.on('message',m=>parentPort.postMessage({hits:find(m.lines,m.rules)}));", {eval:true});
      this.inner.on('message',data=>this.onmessage?.({data}));
      this.inner.on('error',error=>this.onerror?.(error)); workers.push(this);
    }
    postMessage(data) { this.inner.postMessage(data); }
    terminate() { this.stopped=true; void this.inner.terminate(); }
  }
  const { TerminalHighlighter } = terminalLoader({ Worker })('terminalHighlight');
  const row={translateToString:()=> 'a'.repeat(2000)+'!'};
  const term={rows:1,buffer:{active:{type:'normal',viewportY:0,length:1,getLine:()=>row}}};
  let reported; const failure = new Promise(resolve=>{reported=resolve});
  const highlighter=new TerminalHighlighter(term,[{id:'evil',pattern:'(a+)+$',enabled:true,color:'#ef4444'}],reported);
  highlighter.render();
  assert.match(await failure, /time limit/); assert.equal(workers[0].stopped,true); highlighter.dispose();
});
