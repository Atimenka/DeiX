'use strict';

process.env.SUPABASE_URL = 'https://db.test';
process.env.SUPABASE_SERVICE_ROLE_KEY = 'service-key';
process.env.SESSION_SECRET = 'test-secret-test-secret-test-secret';

const test = require('node:test');
const assert = require('node:assert/strict');
const crypto = require('node:crypto');

const ID = 'a'.repeat(32);
const db = { owner: null, rows: new Map(), open: true };

function jsonResponse(status, data) {
  return { ok: status < 400, status, text: async () => (data === undefined ? '' : JSON.stringify(data)) };
}

globalThis.fetch = async (url, init) => {
  const u = new URL(url);
  const path = u.pathname.replace('/rest/v1/', '');
  assert.equal(init.headers.apikey, 'service-key');
  const body = init.body ? JSON.parse(init.body) : undefined;
  if (path === 'dev_settings') {
    const key = u.searchParams.get('key');
    if (init.method === 'POST') {
      if (body.key === 'owner') db.owner = body.value;
      if (body.key === 'requests_open') db.open = body.value;
      return jsonResponse(201);
    }
    if (key === 'eq.owner') return jsonResponse(200, db.owner ? [{ value: db.owner }] : []);
    return jsonResponse(200, [{ value: db.open }]);
  }
  if (path === 'dev_unlocks') {
    if (init.method === 'DELETE') {
      db.rows.delete(u.searchParams.get('device_id').slice(3));
      return jsonResponse(204);
    }
    const filter = u.searchParams.get('device_id');
    let rows = [...db.rows.values()];
    if (filter) rows = rows.filter((r) => r.device_id === filter.slice(3));
    return jsonResponse(200, rows);
  }
  if (path === 'rpc/request_dev_unlock') {
    const cur = db.rows.get(body.p_device_id);
    if (cur) return jsonResponse(200, { result: 'exists', status: cur.status });
    if (!db.open) return jsonResponse(400, { code: 'P0001', message: 'closed' });
    db.rows.set(body.p_device_id, {
      device_id: body.p_device_id,
      hostname: body.p_hostname,
      note: body.p_note,
      source: body.p_source,
      status: 'pending',
      created_at: new Date().toISOString(),
    });
    return jsonResponse(200, { result: 'created', status: 'pending' });
  }
  if (path === 'rpc/approve_dev_unlock' || path === 'rpc/deny_dev_unlock') {
    const row = db.rows.get(body.p_device_id);
    if (!row) return jsonResponse(400, { code: 'P0001', message: 'request not found' });
    row.status = path.includes('approve') ? 'approved' : 'denied';
    row.decided_by = body.p_by;
    return jsonResponse(200, row);
  }
  throw new Error(`unexpected fetch ${path}`);
};

const lib = require('../api/_lib');
const admin = require('../api/admin');
const request = require('../api/request');

function call(handler, body, { cookie, headers } = {}) {
  return new Promise((resolve) => {
    const out = { headers: {}, statusCode: 200 };
    const req = {
      method: 'POST',
      body,
      headers: { host: 'site.test', 'x-deix-csrf': '1', 'x-forwarded-for': '1.2.3.4', cookie, ...headers },
    };
    const res = {
      setHeader: (k, v) => {
        out.headers[k.toLowerCase()] = v;
      },
      end: (text) => resolve({ status: res.statusCode, headers: out.headers, json: JSON.parse(text) }),
    };
    Object.defineProperty(res, 'statusCode', { get: () => out.statusCode, set: (v) => (out.statusCode = v) });
    handler(req, res);
  });
}

test.before(async () => {
  db.owner = { user: 'Atimenka', hash: await lib.hashPassword('correct horse') };
});

test('login rejects wrong password and accepts the right one', async () => {
  const bad = await call(admin, { action: 'login', user: 'Atimenka', password: 'nope' });
  assert.equal(bad.status, 401);
  const wrongUser = await call(admin, { action: 'login', user: 'x', password: 'correct horse' });
  assert.equal(wrongUser.status, 401);
  const ok = await call(admin, { action: 'login', user: 'atimenka', password: 'correct horse' });
  assert.equal(ok.status, 200);
  assert.match(ok.headers['set-cookie'], /HttpOnly; Secure; SameSite=Strict/);
});

test('admin API needs a session, the csrf header and same origin', async () => {
  assert.equal((await call(admin, { action: 'list' })).status, 401);
  assert.equal((await call(admin, { action: 'list' }, { headers: { 'x-deix-csrf': '0' } })).status, 403);
  assert.equal((await call(admin, { action: 'list' }, { headers: { origin: 'https://evil.test' } })).status, 403);
  const forged = await call(admin, { action: 'list' }, { cookie: 'deix_owner=eyJ1IjoiQXRpbWVua2EifQ.AAAA' });
  assert.equal(forged.status, 401);
});

test('public request then owner approve, deny and delete', async () => {
  const sub = await call(request, { action: 'submit', device_id: ID.toUpperCase(), name: 'qemu', note: 'hi' });
  assert.equal(sub.json.result, 'created');
  const again = await call(request, { action: 'submit', device_id: ID });
  assert.equal(again.json.result, 'exists');
  assert.equal((await call(request, { action: 'submit', device_id: 'zz' })).status, 400);
  assert.equal((await call(request, { action: 'submit', device_id: ID, website: 'bot' })).status, 400);
  const status = await call(request, { action: 'status', device_id: ID });
  assert.equal(status.json.status, 'pending');
  assert.equal(JSON.stringify(status.json).includes('unlock_key'), false);

  const login = await call(admin, { action: 'login', user: 'Atimenka', password: 'correct horse' });
  const cookie = login.headers['set-cookie'].split(';')[0];
  const list = await call(admin, { action: 'list' }, { cookie });
  assert.equal(list.json.counts.pending, 1);
  assert.equal(list.json.requests[0].source, 'web');

  const approved = await call(admin, { action: 'decide', device_id: ID, decision: 'approve' }, { cookie });
  assert.equal(approved.json.status, 'approved');
  assert.equal(db.rows.get(ID).decided_by, 'Atimenka');
  assert.equal((await call(admin, { action: 'decide', device_id: ID, decision: 'maybe' }, { cookie })).status, 400);
  const denied = await call(admin, { action: 'decide', device_id: ID, decision: 'deny' }, { cookie });
  assert.equal(denied.json.status, 'denied');

  assert.equal((await call(admin, { action: 'set_open', open: false }, { cookie })).json.requests_open, false);
  assert.equal((await call(request, { action: 'submit', device_id: 'b'.repeat(32) })).status, 400);
  await call(admin, { action: 'set_open', open: true }, { cookie });

  await call(admin, { action: 'remove', device_id: ID }, { cookie });
  assert.equal(db.rows.size, 0);
});

test('changing the password replaces the hash and kills old sessions', async () => {
  const login = await call(admin, { action: 'login', user: 'Atimenka', password: 'correct horse' });
  const oldCookie = login.headers['set-cookie'].split(';')[0];
  const short = await call(admin, { action: 'change_password', current: 'correct horse', next: 'short' }, { cookie: oldCookie });
  assert.equal(short.status, 400);
  const wrong = await call(admin, { action: 'change_password', current: 'bad', next: 'a-much-longer-pass' }, { cookie: oldCookie });
  assert.equal(wrong.status, 401);
  const changed = await call(admin, { action: 'change_password', current: 'correct horse', next: 'a-much-longer-pass' }, { cookie: oldCookie });
  assert.equal(changed.status, 200);
  assert.equal((await call(admin, { action: 'list' }, { cookie: oldCookie })).status, 401);
  const newCookie = changed.headers['set-cookie'].split(';')[0];
  assert.equal((await call(admin, { action: 'list' }, { cookie: newCookie })).status, 200);
  assert.equal(crypto.createHash('sha256').update(db.owner.hash).digest('hex').length, 64);
});

test('login is rate limited after repeated failures', async () => {
  const headers = { 'x-forwarded-for': '9.9.9.9' };
  for (let i = 0; i < 5; i++) {
    assert.equal((await call(admin, { action: 'login', user: 'Atimenka', password: 'x' }, { headers })).status, 401);
  }
  assert.equal((await call(admin, { action: 'login', user: 'Atimenka', password: 'a-much-longer-pass' }, { headers })).status, 429);
});
