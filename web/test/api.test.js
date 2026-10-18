'use strict';

process.env.SUPABASE_URL = 'https://db.test';
process.env.SUPABASE_SERVICE_ROLE_KEY = 'service-key';
process.env.SUPABASE_ANON_KEY = 'anon-key';

const nodeCrypto = require('node:crypto');
const dnsPromises = require('node:dns').promises;
dnsPromises.resolveMx = async (domain) => {
  if (domain === 'nomx.invalid') throw Object.assign(new Error('nx'), { code: 'ENOTFOUND' });
  if (domain === 'flaky.test') throw Object.assign(new Error('timeout'), { code: 'ETIMEOUT' });
  return [{ exchange: `mx.${domain}`, priority: 10 }];
};
dnsPromises.resolve4 = async () => {
  throw Object.assign(new Error('nx'), { code: 'ENOTFOUND' });
};
const keyPair = nodeCrypto.generateKeyPairSync('ec', { namedCurve: 'prime256v1' });
process.env.DEV_KEY_PRIVATE = keyPair.privateKey.export({ type: 'pkcs8', format: 'der' }).toString('base64');

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
  refreshAal: new Map(),
  open: true,
  codes: new Map(),
  keys: [],
  factors: new Map(),
  verifications: [],
  backup: [],
  mails: [],
  challenges: new Map(),
  seq: 0,
};

const GOOD_CODE = '123456';

function jsonResponse(status, data) {
  return { ok: status < 400, status, text: async () => (data === undefined ? '' : JSON.stringify(data)) };
}

function addUser(email, password, username, role) {
  const id = `00000000-0000-0000-0000-${String(++db.seq).padStart(12, '0')}`;
  db.users.set(id, { id, email, password });
  db.profiles.set(id, { id, username, role, email_verified: true });
  return id;
}

function userView(userId) {
  return { ...db.users.get(userId), factors: db.factors.get(userId) || [] };
}

function issue(userId, aal = 'aal1') {
  const payload = Buffer.from(JSON.stringify({ aal })).toString('base64url');
  const access = `h.${payload}.${++db.seq}`;
  const refresh = `rt-${++db.seq}`;
  db.tokens.set(access, userId);
  db.refresh.set(refresh, userId);
  db.refreshAal.set(refresh, aal);
  return { access_token: access, refresh_token: refresh, user: userView(userId) };
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
    return jsonResponse(200, issue(userId, db.refreshAal.get(body.refresh_token)));
  }
  if (p === '/auth/v1/user') {
    const userId = db.tokens.get(bearer(init));
    return userId ? jsonResponse(200, userView(userId)) : jsonResponse(401, { msg: 'invalid JWT' });
  }
  if (p === '/auth/v1/factors') {
    const userId = db.tokens.get(bearer(init));
    if (!userId) return jsonResponse(401, {});
    const factor = { id: `f-${++db.seq}`, factor_type: 'totp', status: 'unverified' };
    db.factors.set(userId, [...(db.factors.get(userId) || []), factor]);
    return jsonResponse(200, { id: factor.id, totp: { qr_code: '<svg></svg>', secret: 'JBSWY3DPEHPK3PXP', uri: 'otpauth://totp/x' } });
  }
  const factorCall = /^\/auth\/v1\/factors\/([^/]+)\/(challenge|verify)$/.exec(p);
  if (factorCall) {
    const userId = db.tokens.get(bearer(init));
    if (!userId) return jsonResponse(401, {});
    if (factorCall[2] === 'challenge') {
      const id = `c-${++db.seq}`;
      db.challenges.set(id, factorCall[1]);
      return jsonResponse(200, { id });
    }
    if (db.challenges.get(body.challenge_id) !== factorCall[1] || body.code !== GOOD_CODE) {
      return jsonResponse(400, { error_code: 'mfa_verification_failed' });
    }
    for (const f of db.factors.get(userId)) if (f.id === factorCall[1]) f.status = 'verified';
    return jsonResponse(200, issue(userId, 'aal2'));
  }
  const factorDelete = /^\/auth\/v1\/admin\/users\/([^/]+)\/factors\/([^/]+)$/.exec(p);
  if (factorDelete && init.method === 'DELETE') {
    db.factors.set(factorDelete[1], (db.factors.get(factorDelete[1]) || []).filter((f) => f.id !== factorDelete[2]));
    return jsonResponse(200, {});
  }
  if (u.host === 'api.resend.com') {
    db.mails.push(JSON.parse(init.body));
    return jsonResponse(200, { id: 'm1' });
  }
  if (p === '/auth/v1/logout') {
    db.tokens.delete(bearer(init));
    return jsonResponse(204);
  }
  if (p === '/auth/v1/admin/users' && init.method === 'POST') {
    assert.equal(init.headers.apikey, 'service-key');
    if ([...db.users.values()].some((x) => x.email === body.email)) return jsonResponse(422, { error_code: 'email_exists' });
    const id = addUser(body.email, body.password, body.user_metadata.username, 'user');
    db.profiles.get(id).email_verified = false;
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
    if (init.method === 'PATCH') {
      for (const row of rows) Object.assign(row, body);
      return jsonResponse(204);
    }
    return jsonResponse(200, rows);
  }
  if (path === 'email_verifications') {
    if (init.method === 'POST') {
      db.verifications.push({ id: ++db.seq, used_at: null, ...body });
      return jsonResponse(201);
    }
    if (init.method === 'PATCH') {
      const row = db.verifications.find((r) => r.id === Number(u.searchParams.get('id').slice(3)));
      Object.assign(row, body);
      return jsonResponse(204);
    }
    const hash = u.searchParams.get('token_hash').slice(3);
    return jsonResponse(
      200,
      db.verifications.filter((r) => r.token_hash === hash && !r.used_at && new Date(r.expires_at) > new Date()),
    );
  }
  if (path === 'mfa_backup_codes') {
    const owner = (u.searchParams.get('user_id') || 'eq.').slice(3);
    if (init.method === 'DELETE') {
      db.backup = db.backup.filter((r) => r.user_id !== owner);
      return jsonResponse(204);
    }
    if (init.method === 'POST') {
      db.backup.push(...body.map((r) => ({ id: ++db.seq, used_at: null, ...r })));
      return jsonResponse(201);
    }
    if (init.method === 'PATCH') {
      const row = db.backup.find((r) => r.id === Number(u.searchParams.get('id').slice(3)));
      Object.assign(row, body);
      return jsonResponse(204);
    }
    const hash = u.searchParams.get('code_hash');
    return jsonResponse(
      200,
      db.backup.filter((r) => r.user_id === owner && !r.used_at && (!hash || r.code_hash === hash.slice(3))),
    );
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
  if (path.startsWith('dev_keys')) {
    if (init.method === 'POST') {
      db.keys.push({ ...body, created_at: new Date().toISOString() });
      return jsonResponse(201);
    }
    const owner = u.searchParams.get('user_id').slice(3);
    return jsonResponse(200, db.keys.filter((k) => k.user_id === owner));
  }
  if (path === 'device_links') {
    const owner = u.searchParams.get('user_id').slice(3);
    return jsonResponse(200, [...db.links.values()].filter((l) => l.user_id === owner));
  }
  if (path === 'rpc/begin_device_link') {
    db.codes.set(body.p_device_id, 'ABCD2345');
    return jsonResponse(200, { status: 'code', code: 'ABCD-2345', expires_in: 600 });
  }
  if (path === 'rpc/request_dev_unlock') {
    const link = db.links.get(body.p_device_id);
    if (!link) return jsonResponse(400, { code: 'P0001', message: "device is not linked to an account: run 'dev link' in DeiX" });
    assert.equal(body.p_source, 'os');
    return jsonResponse(200, { result: 'created', status: 'pending' });
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
const osHandler = require('../api/os');
const keysHandler = require('../api/keys');
const { signedMessage } = require('../api/_keys');

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

let ownerId;

test.before(() => {
  ownerId = addUser('owner@example.com', 'owner-pass', 'Atimenka', 'owner');
  db.factors.set(ownerId, [{ id: 'owner-factor', factor_type: 'totp', status: 'verified' }]);
});

async function loginOwner() {
  const step = await call(authHandler, { action: 'login', identifier: 'Atimenka', password: 'owner-pass' });
  assert.equal(step.json.mfa, true);
  const pending = sessionOf(step);
  const done = await call(authHandler, { action: 'mfa_verify', code: GOOD_CODE }, { cookies: pending });
  assert.equal(done.status, 200, JSON.stringify(done.json));
  return sessionOf(done);
}

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

  const owner = await loginOwner();
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
  const owner = await loginOwner();
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

function callOs(fn, body, method = 'POST') {
  return new Promise((resolve) => {
    const out = { statusCode: 200 };
    const req = { method, url: `/api/os?fn=${fn}`, body, headers: { host: 'site.test', 'x-forwarded-for': '7.7.7.7' } };
    const res = {
      setHeader: () => {},
      end: (text) => resolve({ status: out.statusCode, json: JSON.parse(text) }),
    };
    Object.defineProperty(res, 'statusCode', { get: () => out.statusCode, set: (v) => (out.statusCode = v) });
    osHandler(req, res);
  });
}

test('OS gateway forwards only whitelisted functions and reports refusals as PostgREST errors', async () => {
  const dev = 'c'.repeat(32);
  assert.equal((await callOs('begin_device_link', {}, 'GET')).status, 405);
  assert.equal((await callOs('drop_everything', { p_device_id: dev })).status, 404);
  assert.equal((await callOs('begin_device_link', { p_device_id: 'zz' })).status, 400);

  const code = await callOs('begin_device_link', { p_device_id: dev });
  assert.equal(code.status, 200);
  assert.equal(code.json.code, 'ABCD-2345');

  const refused = await callOs('request_dev_unlock', { p_device_id: dev, p_hostname: 'DeiX', p_source: 'web' });
  assert.equal(refused.status, 400);
  assert.match(refused.json.message, /not linked/);

  db.links.set(dev, { device_id: dev, user_id: 'u' });
  const ok = await callOs('request_dev_unlock', { p_device_id: dev, p_hostname: 'DeiX', p_source: 'web' });
  assert.equal(ok.json.result, 'created');
  db.links.delete(dev);
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

function decode32(text) {
  const alphabet = 'ABCDEFGHJKLMNPQRSTUVWXYZ23456789';
  let bits = 0;
  let acc = 0;
  const out = [];
  for (const ch of text.replace(/-/g, '')) {
    acc = (acc << 5) | alphabet.indexOf(ch);
    bits += 5;
    if (bits >= 8) {
      out.push((acc >> (bits - 8)) & 255);
      bits -= 8;
      acc &= (1 << bits) - 1;
    }
  }
  return Buffer.from(out);
}

test('dev keys: need login, are signed for the device, and are rate limited', async () => {
  assert.equal((await call(keysHandler, { action: 'issue', device_id: ID })).status, 401);
  const user = sessionOf(await call(authHandler, { action: 'login', identifier: 'Alice_1', password: 'brand-new-pass' }));
  assert.equal((await call(keysHandler, { action: 'issue', device_id: 'zz' }, { cookies: user })).status, 400);

  const out = await call(keysHandler, { action: 'issue', device_id: ID.toUpperCase() }, { cookies: user });
  assert.equal(out.status, 200);
  assert.match(out.json.key, /^([A-Z2-9]{4}-){28}[A-Z2-9]{4}$/);
  const raw = decode32(out.json.key).subarray(0, 72);
  const nonce = raw.subarray(0, 8);
  const signature = raw.subarray(8);
  const verify = (deviceId) =>
    nodeCrypto.verify('sha256', signedMessage(deviceId, nonce), { key: keyPair.publicKey, dsaEncoding: 'ieee-p1363' }, signature);
  assert.equal(verify(ID), true);
  assert.equal(verify('b'.repeat(32)), false);
  assert.equal(db.keys.length, 1);
  assert.equal(db.keys[0].nonce, nonce.toString('hex'));
  assert.equal((await call(keysHandler, { action: 'list' }, { cookies: user })).json.keys.length, 1);

  const second = await call(keysHandler, { action: 'issue', device_id: ID }, { cookies: user });
  assert.notEqual(second.json.key, out.json.key);

  db.open = false;
  assert.equal((await call(keysHandler, { action: 'issue', device_id: ID }, { cookies: user })).status, 400);
  db.open = true;

  for (let i = 0; i < 3; i++) {
    assert.equal((await call(keysHandler, { action: 'issue', device_id: ID }, { cookies: user })).status, 200);
  }
  assert.equal((await call(keysHandler, { action: 'issue', device_id: ID }, { cookies: user })).status, 429);
});

const FRESH = { 'x-forwarded-for': '5.5.5.5' };

test('email domain check rejects disposable and mail-less domains but tolerates resolver failures', async () => {
  const reg = (email, username) => call(authHandler, { ...REG, email, username }, { headers: FRESH });
  const disposable = await reg('x@mailinator.com', 'Dom_1');
  assert.equal(disposable.status, 400);
  assert.match(disposable.json.error, /Одноразовые/);
  assert.equal((await reg('x@sub.yopmail.com', 'Dom_2')).status, 400);
  const nomx = await reg('x@nomx.invalid', 'Dom_3');
  assert.equal(nomx.status, 400);
  assert.match(nomx.json.error, /почтового сервера/);
  assert.equal((await reg('x@flaky.test', 'Dom_4')).status, 200);
});

test('verification mail: sent with a provider, gates keys and requests until the link is opened', async () => {
  process.env.RESEND_API_KEY = 're_test';
  process.env.MAIL_FROM = 'DeiX <noreply@site.test>';
  try {
    const cfg = await call(authHandler, { action: 'config' });
    assert.equal(cfg.json.email_verification, true);

    const headers = { 'x-forwarded-for': '6.6.6.6' };
    const reg = await call(authHandler, { ...REG, email: 'bob@example.org', username: 'Bob_1' }, { headers });
    assert.equal(reg.status, 200);
    assert.equal(reg.json.verification, 'sent');
    assert.equal(db.mails.length, 1);
    assert.deepEqual(db.mails[0].to, ['bob@example.org']);
    const token = /verify\?token=([0-9a-f]{64})/.exec(db.mails[0].text)[1];
    assert.match(db.mails[0].text, /^.*Bob_1/);
    assert.ok(db.mails[0].text.includes(`https://site.test/verify?token=${token}`));
    assert.equal(db.verifications[0].token_hash, nodeCrypto.createHash('sha256').update(token).digest('hex'));

    const jar = sessionOf(reg);
    const me = await call(authHandler, { action: 'me' }, { cookies: jar });
    assert.equal(me.json.user.email_verified, false);
    const blocked = await call(keysHandler, { action: 'issue', device_id: ID }, { cookies: jar });
    assert.equal(blocked.status, 403);
    assert.equal(blocked.json.code, 'email_unverified');
    assert.equal((await call(requestHandler, { action: 'submit', device_id: ID }, { cookies: jar })).json.code, 'email_unverified');

    assert.equal((await call(authHandler, { action: 'verify_email', token: 'f'.repeat(64) })).status, 400);
    assert.equal((await call(authHandler, { action: 'verify_email', token: 'bad' })).status, 400);
    assert.equal((await call(authHandler, { action: 'verify_email', token })).status, 200);
    assert.equal((await call(authHandler, { action: 'verify_email', token })).status, 400);
    assert.equal((await call(authHandler, { action: 'me' }, { cookies: jar })).json.user.email_verified, true);
    assert.notEqual((await call(keysHandler, { action: 'issue', device_id: ID }, { cookies: jar })).json.code, 'email_unverified');

    assert.equal((await call(authHandler, { action: 'resend_verification' }, { cookies: jar })).status, 400);
  } finally {
    delete process.env.RESEND_API_KEY;
    delete process.env.MAIL_FROM;
  }
});

test('without a mail provider nothing is gated and registration reports it', async () => {
  const reg = await call(authHandler, { ...REG, email: 'dan@example.org', username: 'Dan_1' }, { headers: { 'x-forwarded-for': '6.6.6.7' } });
  assert.equal(reg.json.verification, 'off');
  assert.equal((await call(authHandler, { action: 'resend_verification' }, { cookies: sessionOf(reg) })).status, 503);
});

test('owner panel needs a two-factor session', async () => {
  const saved = db.factors.get(ownerId);
  db.factors.set(ownerId, []);
  try {
    const jar = sessionOf(await call(authHandler, { action: 'login', identifier: 'Atimenka', password: 'owner-pass' }));
    const denied = await call(adminHandler, { action: 'list' }, { cookies: jar });
    assert.equal(denied.status, 403);
    assert.equal(denied.json.code, 'mfa_required');
  } finally {
    db.factors.set(ownerId, saved);
  }
});

test('two-factor lifecycle: enroll, login step, backup code, disable', async () => {
  const headers = { 'x-forwarded-for': '8.8.4.4' };
  const reg = await call(authHandler, { ...REG, email: 'carol@example.net', username: 'Carol_1' }, { headers });
  let jar = sessionOf(reg);

  assert.equal((await call(authHandler, { action: 'mfa_disable', code: GOOD_CODE }, { cookies: jar })).status, 400);
  assert.equal((await call(authHandler, { action: 'mfa_enroll_start' })).status, 401);
  const start = await call(authHandler, { action: 'mfa_enroll_start' }, { cookies: jar });
  assert.equal(start.status, 200);
  assert.ok(start.json.qr.startsWith('data:image/svg+xml'));
  assert.ok(start.json.secret);

  assert.equal((await call(authHandler, { action: 'mfa_enroll_finish', factor_id: 'nope', code: GOOD_CODE }, { cookies: jar })).status, 400);
  assert.equal((await call(authHandler, { action: 'mfa_enroll_finish', factor_id: start.json.factor_id, code: 'abc' }, { cookies: jar })).status, 400);
  assert.equal((await call(authHandler, { action: 'mfa_enroll_finish', factor_id: start.json.factor_id, code: '000000' }, { cookies: jar })).status, 400);
  const done = await call(authHandler, { action: 'mfa_enroll_finish', factor_id: start.json.factor_id, code: GOOD_CODE }, { cookies: jar });
  assert.equal(done.status, 200);
  assert.equal(done.json.backup_codes.length, 8);
  assert.ok(done.json.backup_codes.every((c) => /^[A-Z2-9]{5}-[A-Z2-9]{5}$/.test(c)));
  assert.ok(db.backup.every((r) => /^[0-9a-f]{64}$/.test(r.code_hash)));
  jar = sessionOf(done);
  const me = await call(authHandler, { action: 'me' }, { cookies: jar });
  assert.equal(me.json.user.mfa, true);
  assert.equal(me.json.user.aal, 'aal2');
  assert.equal((await call(authHandler, { action: 'mfa_enroll_start' }, { cookies: jar })).status, 400);

  const step = await call(authHandler, { action: 'login', identifier: 'carol_1', password: 'secret-pass' });
  assert.equal(step.json.mfa, true);
  assert.equal(step.json.user, undefined);
  const pending = sessionOf(step);
  assert.ok(pending.deix_mfa);
  assert.equal(pending.deix_at, undefined);
  assert.match(step.headers['set-cookie'][0], /Path=\/api; Max-Age=300/);
  assert.equal((await call(authHandler, { action: 'me' }, { cookies: pending })).json.user, null);
  assert.equal((await call(authHandler, { action: 'mfa_verify', code: GOOD_CODE })).status, 401);
  assert.equal((await call(authHandler, { action: 'mfa_verify', code: '111111' }, { cookies: pending })).status, 401);
  assert.equal((await call(authHandler, { action: 'mfa_verify', code: 'zz' }, { cookies: pending })).status, 400);
  const ok = await call(authHandler, { action: 'mfa_verify', code: GOOD_CODE }, { cookies: pending });
  assert.equal(ok.status, 200);
  assert.equal(ok.json.user.aal, 'aal2');
  assert.ok(sessionOf(ok).deix_at);
  assert.ok(ok.headers['set-cookie'].some((c) => /^deix_mfa=; .*Max-Age=0/.test(c)));
  jar = sessionOf(ok);

  const regen = await call(authHandler, { action: 'mfa_backup_regenerate', code: GOOD_CODE }, { cookies: jar });
  assert.equal(regen.json.backup_codes.length, 8);
  assert.equal(db.backup.length, 8);
  const status = await call(authHandler, { action: 'mfa_status' }, { cookies: jar });
  assert.deepEqual([status.json.enabled, status.json.backup_left], [true, 8]);

  const second = sessionOf(await call(authHandler, { action: 'login', identifier: 'carol_1', password: 'secret-pass' }));
  const code = regen.json.backup_codes[0].toLowerCase();
  assert.equal((await call(authHandler, { action: 'mfa_verify', code: 'AAAAA-AAAAA' }, { cookies: second })).status, 401);
  const reset = await call(authHandler, { action: 'mfa_verify', code }, { cookies: second });
  assert.equal(reset.status, 200);
  assert.equal(reset.json.mfa_reset, true);
  assert.equal(reset.json.user.mfa, false);
  assert.equal(db.backup.length, 0);
  assert.equal((await call(authHandler, { action: 'login', identifier: 'carol_1', password: 'secret-pass' })).json.mfa, undefined);
});

test('two-factor can be disabled with a valid code', async () => {
  const headers = { 'x-forwarded-for': '8.8.4.5' };
  let jar = sessionOf(await call(authHandler, { ...REG, email: 'erin@example.net', username: 'Erin_1' }, { headers }));
  const start = await call(authHandler, { action: 'mfa_enroll_start' }, { cookies: jar });
  jar = sessionOf(await call(authHandler, { action: 'mfa_enroll_finish', factor_id: start.json.factor_id, code: GOOD_CODE }, { cookies: jar }));
  assert.equal((await call(authHandler, { action: 'mfa_disable', code: '000000' }, { cookies: jar })).status, 401);
  assert.equal((await call(authHandler, { action: 'mfa_disable', code: GOOD_CODE }, { cookies: jar })).status, 200);
  assert.equal(db.backup.length, 0);
  const again = await call(authHandler, { action: 'login', identifier: 'erin_1', password: 'secret-pass' });
  assert.equal(again.json.mfa, undefined);
});
