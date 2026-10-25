/** H-277 reference: exercise the actual SDK installed by DeepSeek Harness, without modifying it. */
import assert from 'node:assert/strict'
import { createRequire } from 'node:module'

const referenceRequire = createRequire(new URL(
  '../../../../借鉴/deepseek-harness/packages/mcp/mcp-client/package.json', import.meta.url,
))
const { Client, InMemoryTransport } = referenceRequire('@modelcontextprotocol/client')
const version = '2024-11-05'
const modes = ['valid', 'empty', 'missing-version', 'missing-capabilities', 'missing-info',
  'wrong-version-type', 'unsupported', 'error']

for (const mode of modes) {
  const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair()
  const methods = []
  serverTransport.onmessage = (request) => {
    methods.push(request.method)
    if (request.id === undefined) return
    assert.equal(request.method, 'initialize')
    assert.equal(request.params.protocolVersion, version)
    if (mode === 'error') {
      void serverTransport.send({ jsonrpc: '2.0', id: request.id,
        error: { code: -32602, message: 'synthetic handshake rejection' } })
      return
    }
    let result = { protocolVersion: version, capabilities: { tools: {} },
      serverInfo: { name: 'initialize-contract-fixture', version: '1' } }
    if (mode === 'empty') result = {}
    if (mode === 'missing-version') delete result.protocolVersion
    if (mode === 'missing-capabilities') delete result.capabilities
    if (mode === 'missing-info') delete result.serverInfo
    if (mode === 'wrong-version-type') result.protocolVersion = 42
    if (mode === 'unsupported') result.protocolVersion = '2099-01-01'
    void serverTransport.send({ jsonrpc: '2.0', id: request.id, result })
  }
  await serverTransport.start()
  // Pin the legacy path to isolate this system's declared version, not SDK 2's modern discovery.
  const client = new Client({ name: 'initialize-reference-audit', version: '1' }, {
    capabilities: {}, supportedProtocolVersions: [version], versionNegotiation: { mode: 'legacy' },
  })
  let connected = false
  let error
  try {
    await client.connect(clientTransport, { timeout: 1000 })
    connected = true
  } catch (caught) {
    error = caught
  } finally {
    await client.close()
    await serverTransport.close()
  }
  const initialized = methods.includes('notifications/initialized')
  console.log(JSON.stringify({ mode, connected, initialized, error: error?.constructor.name }))
  assert.equal(connected, mode === 'valid', `${mode}: SDK connect result`)
  assert.equal(initialized, mode === 'valid', `${mode}: initialized must follow a valid result`)
  if (mode !== 'valid') assert.ok(error instanceof Error)
}
