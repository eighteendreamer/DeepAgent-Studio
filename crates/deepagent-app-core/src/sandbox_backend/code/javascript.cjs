// This is a transport/entrypoint adapter, NOT the security boundary. The host
// launches Node in an OS-enforced LPAC with no network or workspace grant.
// Source is compiled literally; workflow templates are resolved only in inputs.
'use strict';
const fs = require('node:fs');
const input = JSON.parse(fs.readFileSync(0, 'utf8'));
const send = process.stdout.write.bind(process.stdout);
const exit = process.exit.bind(process);
const stringify = JSON.stringify.bind(JSON);
const limit = input.logLimit;
function logBuffer() {
  const chunks = [];
  let size = 0, truncated = false;
  return {
    write(chunk, encoding, callback) {
      const data = Buffer.isBuffer(chunk) ? chunk : Buffer.from(String(chunk), typeof encoding === 'string' ? encoding : 'utf8');
      const remaining = Math.max(0, limit - size);
      if (data.length > remaining) truncated = true;
      if (remaining) { chunks.push(Buffer.from(data.subarray(0, remaining))); size += Math.min(data.length, remaining); }
      if (typeof encoding === 'function') encoding();
      if (typeof callback === 'function') callback();
      return true;
    },
    value() {
      // Decode only complete UTF-8 code points so the host byte budget holds.
      let bytes = Buffer.concat(chunks);
      while (bytes.length && Buffer.byteLength(bytes.toString('utf8')) > limit) bytes = bytes.subarray(0, bytes.length - 1);
      return { text: bytes.toString('utf8'), truncated };
    }
  };
}
const stdout = logBuffer(), stderr = logBuffer();
process.stdout.write = stdout.write;
process.stderr.write = stderr.write;
(async () => {
  let main;
  try {
    main = new Function('require', '"use strict";\n' + input.script + '\n;return main;')(require);
  } catch (_) {
    send(stringify({ ok: false, kind: 'syntax' }), () => exit(1));
    return;
  }
  try {
    const outputs = await main(input.inputs);
    if (outputs === null || typeof outputs !== 'object' || Array.isArray(outputs)) throw new Error();
    const a = stdout.value(), b = stderr.value();
    const frame = stringify({ ok: true, outputs, stdout: a.text, stderr: b.text, stdout_truncated: a.truncated, stderr_truncated: b.truncated });
    if (Buffer.byteLength(frame) > input.responseLimit) throw new Error();
    send(frame, () => exit(0));
  } catch (_) {
    // Error messages/stacks may contain private source, inputs and host paths.
    send(stringify({ ok: false, kind: 'runtime' }), () => exit(1));
  }
})();
