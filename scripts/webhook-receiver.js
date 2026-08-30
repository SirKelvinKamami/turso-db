// webhook-receiver.js — reference recipient for turso-service webhooks (zero dependencies).
//
// Verifies the X-Turso-Signature (HMAC-SHA256) header when WEBHOOK_SECRET is set, logs the
// event, and keeps the latest payload for inspection via GET /last.
//
// Usage:
//   node scripts/webhook-receiver.js
//   PORT=7777 WEBHOOK_SECRET=your-shared-secret node scripts/webhook-receiver.js
//   BAD=1 node scripts/webhook-receiver.js   # respond 500 to first N requests (retry testing)
//
// Signing is done by the sender as: "sha256=<hex hmac-sha256(secret, raw request body)>"

'use strict';

const http = require('http');
const crypto = require('crypto');

const PORT = Number(process.env.PORT || 7777);
const SECRET = process.env.WEBHOOK_SECRET || '';
const BAD = Number(process.env.BAD || 0);

let lastPayload = null;
let lastVerification = null;
let requestCount = 0;

function verifySignature(headers, body) {
  const sig = headers['x-turso-signature'];
  if (!sig) return { ok: false, reason: 'missing X-Turso-Signature' };
  if (!sig.startsWith('sha256=')) return { ok: false, reason: 'unexpected signature prefix' };
  const expected = crypto
    .createHmac('sha256', SECRET)
    .update(body)
    .digest('hex');
  if (!crypto.timingSafeEqual(Buffer.from(expected, 'hex'), Buffer.from(sig.slice('sha256='.length), 'hex'))) {
    return { ok: false, reason: 'signature mismatch' };
  }
  return { ok: true };
}

const server = http.createServer((req, res) => {
  if (req.method === 'GET' && req.url === '/last') {
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify({ payload: lastPayload, verification: lastVerification }));
    return;
  }
  if (req.method === 'GET' && req.url === '/count') {
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify({ requests: requestCount }));
    return;
  }
  if (req.method !== 'POST') {
    res.writeHead(405).end();
    return;
  }

  const chunks = [];
  req.on('data', (c) => chunks.push(c));
  req.on('end', () => {
    requestCount += 1;
    const body = Buffer.concat(chunks);
    const text = body.toString('utf8');

    let parsed = null;
    try { parsed = JSON.parse(text); } catch { /* keep null */ }

    let verification = { ok: true, reason: 'no secret configured' };
    if (SECRET) verification = verifySignature(req.headers, body);

    const summary = {
      event: parsed && parsed.event,
      database: parsed && parsed.database,
      owner: parsed && parsed.owner,
      rows_affected: parsed && parsed.rows_affected,
      changes: parsed && parsed.changes,
    };
    const line = [
      `event=${summary.event}`,
      summary.database ? `db=${summary.database.id}` : '',
      `rows=${summary.rows_affected}`,
      verification.ok ? 'signature=OK' : `signature=FAIL(${verification.reason})`,
      parsed && parsed.changes ? `ops=${parsed.changes.map((c) => c.op).join(',')}` : '',
    ]
      .filter(Boolean)
      .join(' ');
    console.log(`[webhook-receiver] ${line}`);

    if (parsed) lastPayload = summary;
    lastVerification = verification;

    if (BAD > 0 && requestCount <= BAD) {
      res.writeHead(500).end('{"ok":false,"retry":true}');
      return;
    }
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end('{"ok":true}');
  });
});

server.listen(PORT, '127.0.0.1', () => {
  console.log(`[webhook-receiver] listening on http://127.0.0.1:${PORT} (secret ${SECRET ? 'set' : 'UNSET'}, BAD=${BAD})`);
});