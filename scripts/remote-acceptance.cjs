#!/usr/bin/env node
// Opt-in real-service runner. Credentials are inherited only through the child
// environment; the report stores metrics, assertion summaries and exit codes.
'use strict';
const fs = require('node:fs');
const path = require('node:path');
const { spawn } = require('node:child_process');
const { performance } = require('node:perf_hooks');

function option(name) {
  const index = process.argv.indexOf(name);
  if (index < 0 || !process.argv[index + 1]) throw new Error(`Required option: ${name}`);
  return path.resolve(process.argv[index + 1]);
}

function summarize(rows) {
  const groups = new Map();
  for (const row of rows) {
    if (typeof row.operation !== 'string' || !Number.isFinite(row.ms) || row.ms < 0) {
      throw new Error('Invalid metric');
    }
    if (!groups.has(row.operation)) groups.set(row.operation, []);
    groups.get(row.operation).push(row);
  }
  return Object.fromEntries([...groups].map(([operation, samples]) => {
    const times = samples.map(row => row.ms).sort((a, b) => a - b);
    const percentile = p => times[Math.max(0, Math.ceil(times.length * p) - 1)];
    return [operation, {
      samples: times.length, p50_ms: percentile(0.5), p95_ms: percentile(0.95),
      p99_ms: percentile(0.99), max_ms: times.at(-1),
      at_least_1000ms: times.filter(ms => ms >= 1000).length,
      deadline_expired: samples.filter(row => !row.completed).length,
    }];
  }));
}

function execute(binary, args, env, filename, deadlineMs) {
  return new Promise((resolve, reject) => {
    const started = performance.now();
    const stream = fs.createWriteStream(filename, { flags: 'wx' });
    const child = spawn(binary, args, { env, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] });
    let output = '', expired = false;
    const timer = setTimeout(() => { expired = true; child.kill(); }, deadlineMs);
    stream.on('error', error => { child.kill(); reject(error); });
    const append = chunk => {
      stream.write(chunk);
      if (output.length + chunk.length <= 2 * 1024 * 1024) output += chunk.toString();
    };
    child.stdout.on('data', append);
    child.stderr.on('data', append);
    child.on('error', error => { clearTimeout(timer); stream.end(); reject(error); });
    child.on('close', (exitCode, signal) => {
      clearTimeout(timer);
      stream.end(() => {
        const metrics = [...output.matchAll(/RESPONSE_METRIC (\{[^\r\n]+\})/g)]
          .map(match => JSON.parse(match[1]));
        resolve({ exitCode, signal, deadlineExpired: expired,
          processElapsed_ms: performance.now() - started,
          testSummary: output.match(/test result:[^\r\n]+/g) || [],
          metrics, operations: summarize(metrics),
          log: filename,
        });
      });
    });
  });
}

async function main() {
  const output = option('--output');
  // This repository's acceptance environment explicitly requires D-drive writes.
  if (!/^D:\\/i.test(output)) throw new Error('Acceptance output must be on D:');
  const binaries = { response: option('--response-binary'),
    functional: option('--functional-binary'), files: option('--file-binary') };
  for (const binary of Object.values(binaries)) {
    if (!/^D:\\/i.test(binary) || !fs.statSync(binary).isFile()) {
      throw new Error('Supply existing D-drive test executables from the current source build');
    }
  }
  const requests = JSON.parse(process.env.DEEPAGENT_REMOTE_TEST_REQUESTS || 'null');
  if (!Array.isArray(requests) || requests.length === 0 || requests.length > 8) {
    throw new Error('Supply 1-8 authorized requests in DEEPAGENT_REMOTE_TEST_REQUESTS');
  }
  for (const request of requests) {
    if (request.host_key_policy !== 'strict' || !request.known_hosts_path
      || !fs.statSync(request.known_hosts_path).isFile()) {
      throw new Error('Every endpoint requires an independently verified strict host pin');
    }
  }
  fs.mkdirSync(output, { recursive: true });
  const temp = path.join(output, 'tmp');
  fs.mkdirSync(temp, { recursive: true });
  const env = { ...process.env, TEMP: temp, TMP: temp, TMPDIR: temp,
    RUST_TEST_THREADS: '1', RUST_BACKTRACE: '0', RUST_MIN_STACK: '16777216' };
  // Do not expose the complete endpoint list to unrelated child tests.
  delete env.DEEPAGENT_REMOTE_TEST_REQUESTS;
  const report = { started: new Date().toISOString(), binaries, endpoints: [],
    percentileMethod: 'nearest rank; fewer than 100 samples do not establish a stable p99',
    measurement: 'SshService wall time; PTY marker polling adds 2ms plus scheduling; no native IPC/render measurement',
  };
  for (const [index, request] of requests.entries()) {
    const childEnv = { ...env, DEEPAGENT_SSH_EXTERNAL_REQUEST: JSON.stringify(request) };
    const endpoint = { index, host: request.host, port: request.port, tests: {} };
    report.endpoints.push(endpoint);
    const checks = [ ['response', 'real_response_profile', 300000],
      ['functional', 'real_auth_pty_and_forwarding', 165000],
      ['terminal', 'real_terminal_lock_and_metadata', 75000] ];
    for (const [kind, test, deadline] of checks) {
      const binary = kind === 'response' ? binaries.response : binaries.functional;
      endpoint.tests[kind] = await execute(binary,
        [test, '--exact', '--ignored', '--nocapture'], childEnv,
        path.join(output, `endpoint-${index}-${kind}.log`), deadline);
      console.log(`endpoint ${index} ${kind}: exit ${endpoint.tests[kind].exitCode}`);
      fs.writeFileSync(path.join(output, 'report.json'), JSON.stringify(report, null, 2));
    }
  }
  report.files = await execute(binaries.files,
    ['real_file_workbench', '--exact', '--ignored', '--nocapture'],
    { ...env, DEEPAGENT_FILE_TEST_REQUESTS: JSON.stringify(requests) },
    path.join(output, 'file-workbench.log'), 220000);
  report.finished = new Date().toISOString();
  report.passed = report.files.exitCode === 0 && report.endpoints.every(endpoint =>
    Object.values(endpoint.tests).every(test => test.exitCode === 0 && !test.deadlineExpired));
  fs.writeFileSync(path.join(output, 'report.json'), JSON.stringify(report, null, 2));
  if (!report.passed) process.exitCode = 1;
}

if (require.main === module) main().catch(error => {
  // Never serialize request data or full error objects containing input data.
  console.error(error.message);
  process.exitCode = 1;
});
module.exports = { summarize };
