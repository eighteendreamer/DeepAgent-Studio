// Fixed synthetic fixture only; this is not an executor or a security boundary.
// Timeouts, connection refusal and unsupported APIs are NOT accepted as denial.
async function main({ fixture }) {
  const fs = require('node:fs');
  const net = require('node:net');
  const dgram = require('node:dgram');
  function fileProbe(operation) {
    try { operation(); return 'ALLOWED'; }
    catch (error) { return error.code || 'UNKNOWN'; }
  }
  async function socketProbe(kind) {
    return new Promise((resolve) => {
      let socket;
      let finished = false;
      const timer = setTimeout(() => finish('TIMEOUT'), 1500);
      function finish(value) {
        if (finished) return;
        finished = true;
        clearTimeout(timer);
        if (socket) {
          // UDP may not yet be bound; cleanup errors are not probe results.
          try { if (kind === 'tcpConnect') socket.destroy(); else socket.close(); }
          catch (_) {}
        }
        resolve(value);
      }
      try {
        if (kind === 'tcpConnect') {
          socket = new net.Socket();
          socket.once('error', error => finish(error.code || 'UNKNOWN'));
          socket.once('connect', () => finish('ALLOWED'));
          socket.connect({ host: fixture.host, port: fixture.tcpPort });
        } else if (kind === 'tcpListen') {
          socket = net.createServer();
          socket.once('error', error => finish(error.code || 'UNKNOWN'));
          socket.listen({ host: fixture.host, port: 0 }, () => finish('ALLOWED'));
        } else {
          socket = dgram.createSocket(fixture.host === '::1' ? 'udp6' : 'udp4');
          socket.once('error', error => finish(error.code || 'UNKNOWN'));
          if (kind === 'udpBind') {
            socket.bind({ address: fixture.host, port: 0 }, () => finish('ALLOWED'));
          } else {
            socket.send(Buffer.from('H178_FIXED_PROBE'), fixture.udpPort, fixture.host,
              error => finish(error ? (error.code || 'UNKNOWN') : 'ALLOWED'));
          }
        }
      } catch (error) { finish(error.code || 'UNKNOWN'); }
    });
  }
  return {
    hostRead: fileProbe(() => fs.readFileSync(fixture.canary)),
    hostWrite: fileProbe(() => fs.writeFileSync(fixture.outsideWrite, 'fixed')),
    stageWrite: fileProbe(() => fs.writeFileSync(require('node:path').join(process.cwd(), 'must-not-create.txt'), 'fixed')),
    tcpConnect: await socketProbe('tcpConnect'),
    tcpListen: await socketProbe('tcpListen'),
    udpSend: await socketProbe('udpSend'),
    udpBind: await socketProbe('udpBind'),
  };
}
