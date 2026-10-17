'use strict';

process.env.SUPABASE_URL = 'https://db.test';
process.env.SUPABASE_SERVICE_ROLE_KEY = 'service-key';
process.env.SUPABASE_ANON_KEY = 'anon-key';

const test = require('node:test');
const assert = require('node:assert/strict');

const ID = 'a'.repeat(32);

const db = {
  users: new Map(),
  profiles: new Map(),
  rows: new Map(),
  links: new Map(),
  tokens: new Map(),
  refresh: new Map(),
  open: true,
  codes: new Map(),
  seq: 0,
};

function jsonResponse(status, data) {
  return { ok: status < 400, status, text: async () => (data === undefined ? '' : JSON.stringify(data)) };
}

function addUser(email, password, username, role) {
  const id = `00000000-0000-0000-0000-${String(++db.seq).padStart(12, '0')}`;
  db.users.set(id, { id, email, password });
  db.profiles.set(id, { id, username, role });
  return id;
}

function issue(userId) {
  const access = `at-${++db.seq}`;
  const refresh = `rt-${++db.seq}`;
  db.tokens.set(access, userId);
  db.refresh.set(refresh, userId);
  return { access_token: access, refresh_token: refresh, user: db.users.get(userId) };
}

function bearer(init) {
  return String(init.headers.Authorization || '').replace('Bearer ', '');
}

globalThis.fetch = async (url, init = {}) => {
  const u = new URL(url);
  const body = init.body ? JSON.parse(init.body) : undefined;
  const p = u.pathname;

  if (u.host === 'challenges.cloudflare.com') {
    return jsonResponse(200, { success: String(init.body).includes('response=good') });
  }

  if (p === '/auth/v1/token') {
    if (u.searchParams.get('grant_type') === 'password') {
      const user = [...db.users.values()].find((x) => x.email === body.email && x.password === body.password);
      return user ? jsonResponse(200, issue(user.id)) : jsonResponse(400, { error_code: 'invalid_credentials' });
    }
    const userId = db.refresh.get(body.refresh_token);
    if (!userId) return jsonResponse(400, { error_code: 'refresh_token_not_found' });
    db.refresh.delete(body.refresh_token);
    return jsonResponse(200, issue(userId));
  }
  if (p === '/auth/v1/user') {
    const userId = db.tokens.get(bearer(init));
    return userId ? jsonResponse(200, db.users.get(userId)) : jsonResponse(401, { msg: 'invalid JWT' });
  }
  if (p === '/auth/v1/logout') {
    db.tokens.delete(bearer(init));
    return jsonResponse(204);
  }
  if (p === '/auth/v1/admin/users' && init.method === 'POST') {
    assert.equal(init.headers.apikey, 'service-key');
    if ([...db.users.values()].some((x) => x.email === body.email)) return jsonResponse(422, { error_code: 'email_exists' });
    const id = addUser(body.email, body.password, body.user_metadata.username, 'user');
    return jsonResponse(200, { id });
  }
  if (p.startsWith('/auth/v1/admin/users/') && init.method === 'PUT') {
    db.users.get(p.split('/').pop()).password = body.password;
    return jsonResponse(200, {});
  }

  const path = p.replace('/rest/v1/', '');
  assert.equal(init.headers.apikey, 'service-key');
  if (path === 'profiles') {
    let rows = [...db.profiles.values()];
    const id = u.searchParams.get('id');
    const name = u.searchParams.get('username');
    if (id) rows = rows.filter((r) => r.id === id.slice(3));
    if (name) rows = rows.filter((r) => r.username.toLowerCase() === name.slice(6).toLowerCase());
    return jsonResponse(200, rows);
  }
  if (path === 'rpc/email_for_username') {
    const row = [...db.profiles.values()].find((r) => r.username.toLowerCase() === body.p_username.toLowerCase());
    return jsonResponse(200, row ? db.users.get(row.id).email : null);
  }
  if (path === 'rpc/verify_user_password') {
    return jsonResponse(200, db.users.get(body.p_user_id).password === body.p_password);
  }
  if (path === 'dev_settings') {
    if (init.method === 'POST') {
      db.open = body.value;
      return jsonResponse(201);
    }
    return jsonResponse(200, [{ value: db.open }]);
  }
  if (path === 'dev_unlocks') {
    if (init.method === 'DELETE') {
      db.rows.delete(u.searchParams.get('device_id').slice(3));
      return jsonResponse(204);
    }
    let rows = [...db.rows.values()];
    const owner = u.searchParams.get('user_id');
    if (owner) rows = rows.filter((r) => r.user_id === owner.slice(3));
    return jsonResponse(
      200,
      rows.map((r) => ({ ...r, profiles: r.user_id ? { username: db.profiles.get(r.user_id).username } : null })),
    );
  }
  if (path === 'device_links') {
    const owner = u.searchParams.get('user_id').slice(3);
    return jsonResponse(200, [...db.links.values()].filter((l) => l.user_id === owner));
  }
  if (path === 'rpc/link_device') {
    const entry = [...db.codes].find(([, c]) => c === body.p_code.replace('-', '').toUpperCase());
    if (!entry) return jsonResponse(400, { code: 'P0001', message: 'invalid or expired code' });
    db.codes.delete(entry[0]);
    db.links.set(entry[0], { device_id: entry[0], user_id: body.p_user_id, linked_at: new Date().toISOString() });
    return jsonResponse(200, { device_id: entry[0] });
  }
  if (path === 'rpc/unlink_device') {
    const link = db.links.get(body.p_device_id);
    const ok = Boolean(link && link.user_id === body.p_user_id);
    if (ok) db.links.delete(body.p_device_id);
    return jsonResponse(200, ok);
  }
  if (path === 'rpc/web_request_dev_unlock') {
    const link = db.links.get(body.p_device_id);
    if (!link || link.user_id !== body.p_user_id) {
      return jsonResponse(400, { code: 'P0001', message: 'this device is not linked to your account' });
    }
    if (!db.open) return jsonResponse(400, { code: 'P0001', message: 'closed' });
    const cur = db.rows.get(body.p_device_id);
    if (cur) return jsonResponse(200, { result: 'exists', status: cur.status });
    db.rows.set(body.p_device_id, {
      device_id: body.p_device_id,
      hostname: body.p_name,
      note: body.p_note,
      source: 'web',
      status: 'pending',
      user_id: body.p_user_id,
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
  throw new Error(`unexpected fetch ${url}`);
};

const authHandler = require('../api/auth');
const adminHandler = require('../api/admin');
const requestHandler = require('../api/request');
const devicesHandler = require('../api/devices');

function call(handler, body, { cookies = {}, headers } = {}) {
  return new Promise((resolve) => {
    const out = { headers: {}, statusCode: 200 };
    const cookie = Object.entries(cookies)
      .map(([k, v]) => `${k}=${v}`)
      .join('; ');
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

function sessionOf(response) {
  const jar = {};
  for (const line of response.headers['set-cookie'] || []) {
    const [pair] = line.split(';');
    const idx = pair.indexOf('=');
    jar[pair.slice(0, idx)] = pair.slice(idx + 1);
  }
  return jar;
}

const REG = { action: 'register', username: 'Alice_1', email: 'Alice@Example.com', password: 'secret-pass', password2: 'secret-pass' };

test.before(() => {
  addUser('owner@example.com', 'owner-pass', 'Atimenka', 'owner');
});

test('registration validates input and creates a signed-in account', async () => {
  assert.equal((await call(authHandler, { ...REG, password2: 'other-pass' })).status, 400);
  assert.equal((await call(authHandler, { ...REG, username: 'a b' })).status, 400);
  assert.equal((await call(authHandler, { ...REG, email: 'nope' })).status, 400);
  assert.equal((await call(authHandler, { ...REG, password: 'short', password2: 'short' })).status, 400);
  assert.equal((await call(authHandler, { ...REG, website: 'bot' })).status, 400);
  assert.equal((await call(authHandler, { ...REG, username: 'ATIMENKA' })).status, 409);

  const ok = await call(authHandler, REG);
  assert.equal(ok.status, 200);
  assert.equal(ok.json.user.username, 'Alice_1');
  const jar = sessionOf(ok);
  assert.ok(jar.deix_at && jar.deix_rt);
  assert.ok(ok.headers['set-cookie'].every((c) => /HttpOnly; Secure; SameSite=Strict; Path=\/; Max-Age=2592000/.test(c)));

  assert.equal((await call(authHandler, { ...REG, username: 'Alice_2' })).status, 409);
});

test('login accepts username or email and rejects wrong passwords', async () => {
  assert.equal((await call(authHandler, { action: 'login', identifier: 'alice_1', password: 'secret-pass' })).status, 200);
  assert.equal((await call(authHandler, { action: 'login', identifier: 'alice@example.com', password: 'secret-pass' })).status, 200);
  assert.equal((await call(authHandler, { action: 'login', identifier: 'alice_1', password: 'bad' })).status, 401);
  assert.equal((await call(authHandler, { action: 'login', identifier: 'nobody', password: 'bad' })).status, 401);
});

test('expired access token is refreshed from the refresh cookie', async () => {
  const login = await call(authHandler, { action: 'login', identifier: 'Alice_1', password: 'secret-pass' });
  const jar = sessionOf(login);
  db.tokens.delete(jar.deix_at);
  const me = await call(authHandler, { action: 'me' }, { cookies: jar });
  assert.equal(me.json.user.username, 'Alice_1');
  const renewed = sessionOf(me);
  assert.notEqual(renewed.deix_at, jar.deix_at);
  const stale = await call(authHandler, { action: 'me' }, { cookies: { deix_at: 'garbage', deix_rt: 'garbage' } });
  assert.equal(stale.json.user, null);
  assert.match(stale.headers['set-cookie'][0], /Max-Age=0/);
});

test('requests need an account; owner panel needs the owner role', async () => {
  assert.equal((await call(requestHandler, { action: 'mine' })).status, 401);
  assert.equal((await call(adminHandler, { action: 'list' })).status, 401);

  const user = sessionOf(await call(authHandler, { action: 'login', identifier: 'Alice_1', password: 'secret-pass' }));
  assert.equal((await call(adminHandler, { action: 'list' }, { cookies: user })).status, 403);

  assert.equal((await call(requestHandler, { action: 'submit', device_id: 'zz' }, { cookies: user })).status, 400);
  const unlinked = await call(requestHandler, { action: 'submit', device_id: ID }, { cookies: user });
  assert.equal(unlinked.status, 400);
  assert.match(unlinked.json.error, /не привязано/);

  db.codes.set(ID, 'ABCD2345');
  assert.equal((await call(devicesHandler, { action: 'link', code: 'zzzz-zzzz' }, { cookies: user })).status, 400);
  assert.equal((await call(devicesHandler, { action: 'link', code: 'bad' }, { cookies: user })).status, 400);
  assert.equal((await call(devicesHandler, { action: 'link', code: 'abcd-2345' }, { cookies: user })).json.device_id, ID);
  assert.equal((await call(devicesHandler, { action: 'list' }, { cookies: user })).json.devices.length, 1);

  const sub = await call(requestHandler, { action: 'submit', device_id: ID.toUpperCase(), name: 'qemu' }, { cookies: user });
  assert.equal(sub.json.result, 'created');
  const mine = await call(requestHandler, { action: 'mine' }, { cookies: user });
  assert.equal(mine.json.requests.length, 1);

  const owner = sessionOf(await call(authHandler, { action: 'login', identifier: 'Atimenka', password: 'owner-pass' }));
  const list = await call(adminHandler, { action: 'list' }, { cookies: owner });
  assert.equal(list.json.counts.pending, 1);
  assert.equal(list.json.requests[0].requester, 'Alice_1');

  assert.equal((await call(adminHandler, { action: 'decide', device_id: ID, decision: 'approve' }, { cookies: owner })).json.status, 'approved');
  assert.equal(db.rows.get(ID).decided_by, 'Atimenka');
  assert.equal((await call(adminHandler, { action: 'decide', device_id: ID, decision: 'maybe' }, { cookies: owner })).status, 400);
  assert.equal((await call(adminHandler, { action: 'set_open', open: false }, { cookies: owner })).json.requests_open, false);
  await call(adminHandler, { action: 'set_open', open: true }, { cookies: owner });
  await call(adminHandler, { action: 'remove', device_id: ID }, { cookies: owner });
  assert.equal(db.rows.size, 0);
});

test('devices need login and are unlinkable only by their owner', async () => {
  assert.equal((await call(devicesHandler, { action: 'list' })).status, 401);
  const owner = sessionOf(await call(authHandler, { action: 'login', identifier: 'Atimenka', password: 'owner-pass' }));
  assert.equal((await call(devicesHandler, { action: 'unlink', device_id: ID }, { cookies: owner })).status, 404);
  const user = sessionOf(await call(authHandler, { action: 'login', identifier: 'Alice_1', password: 'secret-pass' }));
  assert.equal((await call(devicesHandler, { action: 'unlink', device_id: ID }, { cookies: user })).status, 200);
  assert.equal((await call(devicesHandler, { action: 'list' }, { cookies: user })).json.devices.length, 0);
  assert.equal((await call(requestHandler, { action: 'submit', device_id: ID }, { cookies: user })).status, 400);
});

test('wrong link codes are rate limited', async () => {
  const user = sessionOf(await call(authHandler, { action: 'login', identifier: 'Alice_1', password: 'secret-pass' }));
  for (let i = 0; i < 9; i++) {
    assert.equal((await call(devicesHandler, { action: 'link', code: 'ZZZZ-ZZZZ' }, { cookies: user })).status, 400);
  }
  assert.equal((await call(devicesHandler, { action: 'link', code: 'ZZZZ-ZZZZ' }, { cookies: user })).status, 429);
});

test('csrf header and origin are enforced', async () => {
  assert.equal((await call(authHandler, { action: 'me' }, { headers: { 'x-deix-csrf': '0' } })).status, 403);
  assert.equal((await call(authHandler, { action: 'me' }, { headers: { origin: 'https://evil.test' } })).status, 403);
});

test('password change verifies the current password and keeps the session', async () => {
  const jar = sessionOf(await call(authHandler, { action: 'login', identifier: 'Alice_1', password: 'secret-pass' }));
  assert.equal((await call(authHandler, { action: 'change_password', current: 'bad', next: 'brand-new-pass' }, { cookies: jar })).status, 401);
  assert.equal((await call(authHandler, { action: 'change_password', current: 'secret-pass', next: 'short' }, { cookies: jar })).status, 400);
  const changed = await call(authHandler, { action: 'change_password', current: 'secret-pass', next: 'brand-new-pass' }, { cookies: jar });
  assert.equal(changed.status, 200);
  assert.ok(sessionOf(changed).deix_at);
  assert.equal((await call(authHandler, { action: 'login', identifier: 'Alice_1', password: 'secret-pass' })).status, 401);
  assert.equal((await call(authHandler, { action: 'login', identifier: 'Alice_1', password: 'brand-new-pass' })).status, 200);
});

test('logout clears cookies', async () => {
  const jar = sessionOf(await call(authHandler, { action: 'login', identifier: 'Alice_1', password: 'brand-new-pass' }));
  const out = await call(authHandler, { action: 'logout' }, { cookies: jar });
  assert.equal(out.status, 200);
  assert.ok(out.headers['set-cookie'].every((c) => /Max-Age=0/.test(c)));
});

test('login is rate limited after repeated failures', async () => {
  const headers = { 'x-forwarded-for': '9.9.9.9' };
  for (let i = 0; i < 8; i++) {
    assert.equal((await call(authHandler, { action: 'login', identifier: 'Alice_1', password: 'x' }, { headers })).status, 401);
  }
  assert.equal((await call(authHandler, { action: 'login', identifier: 'Alice_1', password: 'brand-new-pass' }, { headers })).status, 429);
});
