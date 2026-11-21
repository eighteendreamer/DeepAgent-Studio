const assert = require('node:assert/strict');
const { test } = require('node:test');
const Zmodem = require('zmodem.js');
const { File } = require('node:buffer');
const { terminalLoader } = require('./terminal-test-loader.cjs');
const turn = () => new Promise(setImmediate);

function pair() {
  const { TerminalZmodem } = terminalLoader()('terminalZmodem');
  const network = [], notices = [], received = [], errors = [], output = [];
  let peer;
  const adapter = new TerminalZmodem(
    { writeBytes: async bytes => { network.push(() => peer.consume([...bytes])); } },
    async text => { output.push(text); },
    notice => notices.push(notice), (name, chunks) => received.push({ name, data: Buffer.concat(chunks.map(b => Buffer.from(b))) }),
    error => errors.push(String(error)),
  );
  return {
    adapter, notices, received, errors, output, network,
    peer(value) { peer = value; value.set_sender(bytes => { network.push(() => adapter.consume(Uint8Array.from(bytes))); }); },
    async until(predicate) {
      for (let i = 0; i < 3000 && !predicate(); i++) {
        while (network.length) await network.shift()();
        await turn();
      }
      assert.ok(predicate(), 'protocol did not settle: ' + errors.join(','));
    },
  };
}

test('ZMODEM upload uses real protocol peers and preserves every binary byte', { timeout: 10000 }, async () => {
  const h = pair(), peer = new Zmodem.Session.Receive(), payload = Buffer.from(Array.from({ length: 70000 }, (_, i) => i % 256));
  let got, acceptance;
  peer.on('offer', offer => { acceptance = offer.accept({ on_input: 'spool_uint8array' }).then(chunks => { got = Buffer.concat(chunks.map(x => Buffer.from(x))); }); });
  h.peer(peer); peer.start();
  await h.until(() => h.notices.at(-1)?.phase === 'detected');
  h.adapter.confirm();
  let done = false, failed;
  const upload = h.adapter.sendFiles([new File([payload], 'binary.dat')]).then(() => { done = true; }, e => { done = true; failed = e; });
  await h.until(() => done); await upload; await acceptance;
  assert.equal(failed, undefined); assert.deepEqual(got, payload); assert.equal(h.errors.length, 0);
  h.adapter.dispose();
});

test('ZMODEM download requires detection and file approval; unsafe names are skipped', { timeout: 10000 }, async () => {
  const h = pair(), peerSentry = new Zmodem.Sentry({
    to_terminal() {}, on_retract() {}, sender: bytes => h.network.push(() => h.adapter.consume(Uint8Array.from(bytes))),
    on_detect(detection) { remote = detection.confirm(); },
  });
  let remote;
  h.peer({ set_sender() {}, consume: bytes => peerSentry.consume(bytes) });
  // ZRQINIT asks this adapter to become the receiver.
  await h.adapter.consume(Uint8Array.from([42,42,24,66,...Buffer.from('00000000000000'),13,10,17]));
  assert.equal(h.notices.at(-1)?.phase, 'detected'); assert.equal(h.received.length, 0);
  h.adapter.confirm(); await h.until(() => !!remote);
  let offered = false, transfer;
  const request = remote.send_offer({ name: '../escape', size: 1 }).then(value => { transfer = value; offered = true; });
  await h.until(() => offered); await request;
  assert.equal(transfer, undefined); assert.ok(h.errors.some(error => error.includes('filename')));
  offered = false;
  const payload = Buffer.from([0, 255, 24, 13, 10, 27, 200, 42]);
  const valid = remote.send_offer({ name: 'safe.dat', size: payload.length }).then(value => { transfer = value; offered = true; });
  await h.until(() => h.notices.at(-1)?.phase === 'offer');
  assert.equal(h.received.length, 0);
  const accepted = h.adapter.acceptReceive();
  await h.until(() => offered); await valid; transfer.send(payload);
  let ended = false; const end = transfer.end([]).then(() => { ended = true; });
  await h.until(() => ended && h.received.length === 1); await end; await accepted;
  assert.deepEqual(h.received[0].data, payload); assert.equal(h.received[0].name, 'safe.dat');
  h.adapter.cancel(); await h.adapter.consume(Buffer.from('shell still works'));
  assert.ok(h.output.join('').endsWith('shell still works')); h.adapter.dispose();
});

test('cancel settles an outstanding upload without freezing shell output', { timeout: 10000 }, async () => {
  const h = pair(), peer = new Zmodem.Session.Receive(); h.peer(peer); peer.start();
  await h.until(() => h.notices.at(-1)?.phase === 'detected'); h.adapter.confirm();
  const upload = h.adapter.sendFiles([new File(['body'], 'test.dat')]);
  await turn(); h.adapter.cancel(); await upload;
  await h.adapter.consume(Buffer.from('after cancel'));
  assert.ok(h.output.join('').endsWith('after cancel')); assert.equal(h.adapter.busy, false);
  h.adapter.dispose();
});

test('a corrupt frame aborts the protocol and resumes shell output without a partial download', { timeout: 10000 }, async () => {
  const h = pair(), peer = new Zmodem.Session.Receive(); h.peer(peer); peer.start();
  await h.until(() => h.notices.at(-1)?.phase === 'detected'); h.adapter.confirm();
  // Deliberately corrupt the CRC of a hex header after a session is confirmed.
  await h.adapter.consume(Uint8Array.from([42, 42, 24, 66, ...Buffer.from('01000000000001'), 13, 10, 17]));
  assert.ok(h.errors.length > 0, 'invalid CRC must be surfaced');
  assert.equal(h.adapter.busy, false); assert.equal(h.received.length, 0);
  await h.adapter.consume(Buffer.from('after malformed frame'));
  assert.ok(h.output.join('').endsWith('after malformed frame')); h.adapter.dispose();
});

test('ZMODEM names and input declarations are bounded before allocation', () => {
  const { safeZmodemName } = terminalLoader()('terminalZmodem');
  for (const name of ['../x', '/etc/passwd', 'CON', 'a:b', 'name.', 'x\\y', 'bad\0name']) assert.throws(() => safeZmodemName(name));
  assert.equal(safeZmodemName('日志.bin'), '日志.bin');
});
