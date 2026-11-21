// Shared synthetic fixture: never returns raw environment values.
const fs = require('node:fs');
const [nonce, mode, report] = process.argv.slice(2);
const has = name => Object.prototype.hasOwnProperty.call(process.env, name);
const expected = mode === 'forwarded' ? 'ambient' : mode;
const observe = () => ({
  ambient_secret_present: has('DEEPAGENT_MCP_AUDIT_SECRET'),
  ambient_secret_matches: process.env.DEEPAGENT_MCP_AUDIT_SECRET === `ambient-${nonce}`,
  explicit_secret_matches: process.env.DEEPAGENT_MCP_AUDIT_SECRET === `explicit-${nonce}`,
  lowercase_secret_matches: process.env.deepagent_mcp_audit_secret === `explicit-${nonce}`,
  lowercase_token_present: has('deepagent_mcp_audit_token'),
  lowercase_token_matches: process.env.deepagent_mcp_audit_token === `lowercase-${nonce}`,
  plain_present: has('DEEPAGENT_MCP_AUDIT_PLAIN'),
  plain_matches: process.env.DEEPAGENT_MCP_AUDIT_PLAIN === `${expected}-plain`,
  opaque_present: has('MCP_CONTEXT_BLOB'),
  proxy_present: has('HTTP_PROXY'),
  proxy_matches: process.env.HTTP_PROXY === `http://user:synthetic-${nonce}@127.0.0.1:9`,
  ca_present: has('SSL_CERT_FILE'),
  ca_matches: process.env.SSL_CERT_FILE === `${nonce}-ca.pem`,
  bootstrap_present: has('NODE_OPTIONS'),
  fixture_runtime_present: has('DEEPAGENT_MCP_FIXTURE_NODE'),
  runtime_base_present: Boolean(process.env.PATH && process.env.HOME && process.env.TEMP),
  locale_matches: process.env.LANG === 'C.UTF-8',
  mixed_path_matches: process.env.pAtH?.split(require('node:path').delimiter).at(-1) === `${nonce}-explicit-path`,
  empty_matches: has('MCP_EMPTY') && process.env.MCP_EMPTY === '',
  unicode_matches: process.env.MCP_LABEL === '本地 环境 🚀',
});
if (report) fs.writeFileSync(report, JSON.stringify(observe()));
require('node:readline').createInterface({input: process.stdin}).on('line', line => {
  const request = JSON.parse(line);
  if (!Object.prototype.hasOwnProperty.call(request, 'id')) return;
  const audit = observe();
  let result = {};
  if (request.method === 'initialize') result = {
    protocolVersion: request.params.protocolVersion, capabilities: {tools: {}},
    serverInfo: {name: 'environment-audit', version: '1'}, audit,
  };
  else if (request.method === 'tools/list') result = {tools: [{
    name: 'env_probe', description: 'Boolean synthetic environment probe',
    inputSchema: {type: 'object'},
  }]};
  else if (request.method === 'tools/call') result = {
    content: [{type: 'text', text: JSON.stringify(audit)}],
    structuredContent: audit, isError: false,
  };
  process.stdout.write(JSON.stringify({jsonrpc: '2.0', id: request.id, result}) + '\n');
}).on('close', () => process.exit(0));
setTimeout(() => process.exit(2), 15000).unref();
