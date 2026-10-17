'use strict';

const {
  DEVICE_ID_RE,
  HttpError,
  clearCookie,
  clientIp,
  currentOwner,
  fail,
  hashPassword,
  issueSession,
  loadOwner,
  overLimit,
  readBody,
  recordHit,
  rpc,
  sameOrigin,
  sb,
  send,
  sessionCookie,
  verifyPassword,
} = require('./_lib');

const LOGIN_MAX_FAILS = 5;
const LOGIN_WINDOW_MS = 15 * 60 * 1000;
const MIN_PASSWORD = 10;
const LIST_COLUMNS = 'device_id,hostname,note,source,status,created_at,decided_at,decided_by,consumed_at';
const DUMMY_HASH = `scrypt$16384$8$1$${Buffer.alloc(16).toString('base64')}$${Buffer.alloc(32).toString('base64')}`;

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function deviceId(body) {
  const id = String(body.device_id || '').trim().toLowerCase();
  if (!DEVICE_ID_RE.test(id)) throw new HttpError(400, 'Некорректный Device ID');
  return id;
}

async function login(req, res, body) {
  const key = `login:${clientIp(req)}`;
  if (overLimit(key, LOGIN_MAX_FAILS, LOGIN_WINDOW_MS)) {
    throw new HttpError(429, 'Слишком много неудачных попыток входа. Подождите 15 минут.');
  }
  const owner = await loadOwner();
  const user = String(body.user || '').trim();
  const password = String(body.password || '');
  const userOk = user.toLowerCase() === owner.user.toLowerCase();
  const passOk = await verifyPassword(password, userOk ? owner.hash : DUMMY_HASH);
  if (!userOk || !passOk) {
    recordHit(key);
    await sleep(400);
    throw new HttpError(401, 'Неверный логин или пароль');
  }
  res.setHeader('Set-Cookie', sessionCookie(issueSession(owner.user, owner.hash)));
  return send(res, 200, { ok: true, user: owner.user });
}

async function list(res) {
  const [rows, settings] = await Promise.all([
    sb(`dev_unlocks?select=${LIST_COLUMNS}&order=created_at.desc&limit=500`),
    sb('dev_settings?key=eq.requests_open&select=value'),
  ]);
  const counts = { pending: 0, approved: 0, consumed: 0, denied: 0 };
  for (const row of rows) counts[row.status] = (counts[row.status] || 0) + 1;
  const open = !(settings[0] && settings[0].value === false);
  return send(res, 200, { ok: true, requests: rows, counts, requests_open: open });
}

async function decide(res, owner, body) {
  const id = deviceId(body);
  const name = body.decision === 'approve' ? 'approve_dev_unlock' : body.decision === 'deny' ? 'deny_dev_unlock' : '';
  if (!name) throw new HttpError(400, 'Неизвестное решение');
  const row = await rpc(name, { p_device_id: id, p_by: owner.user });
  return send(res, 200, { ok: true, status: row.status });
}

async function remove(res, body) {
  const id = deviceId(body);
  await sb(`dev_unlocks?device_id=eq.${id}`, { method: 'DELETE', headers: { Prefer: 'return=minimal' } });
  return send(res, 200, { ok: true });
}

async function setOpen(res, body) {
  if (typeof body.open !== 'boolean') throw new HttpError(400, 'Нужно значение open: true или false');
  await sb('dev_settings?on_conflict=key', {
    method: 'POST',
    headers: { Prefer: 'resolution=merge-duplicates,return=minimal' },
    body: { key: 'requests_open', value: body.open },
  });
  return send(res, 200, { ok: true, requests_open: body.open });
}

async function changePassword(res, owner, body) {
  const current = String(body.current || '');
  const next = String(body.next || '');
  if (next.length < MIN_PASSWORD) {
    throw new HttpError(400, `Новый пароль должен быть не короче ${MIN_PASSWORD} символов`);
  }
  if (next === current) throw new HttpError(400, 'Новый пароль совпадает с текущим');
  if (!(await verifyPassword(current, owner.hash))) {
    await sleep(400);
    throw new HttpError(401, 'Текущий пароль указан неверно');
  }
  const hash = await hashPassword(next);
  await sb('dev_settings?on_conflict=key', {
    method: 'POST',
    headers: { Prefer: 'resolution=merge-duplicates,return=minimal' },
    body: { key: 'owner', value: { user: owner.user, hash } },
  });
  res.setHeader('Set-Cookie', sessionCookie(issueSession(owner.user, hash)));
  return send(res, 200, { ok: true });
}

module.exports = async function handler(req, res) {
  try {
    if (req.method !== 'POST') {
      res.setHeader('Allow', 'POST');
      throw new HttpError(405, 'Метод не поддерживается');
    }
    if (!sameOrigin(req) || req.headers['x-deix-csrf'] !== '1') {
      throw new HttpError(403, 'Запрос отклонён');
    }
    const body = readBody(req);

    if (body.action === 'login') return await login(req, res, body);
    if (body.action === 'logout') {
      res.setHeader('Set-Cookie', clearCookie());
      return send(res, 200, { ok: true });
    }

    const owner = await currentOwner(req);
    if (body.action === 'me') {
      return send(res, 200, { ok: true, owner: Boolean(owner), user: owner ? owner.user : null });
    }
    if (!owner) throw new HttpError(401, 'Требуется вход Owner');

    switch (body.action) {
      case 'list':
        return await list(res);
      case 'decide':
        return await decide(res, owner, body);
      case 'remove':
        return await remove(res, body);
      case 'set_open':
        return await setOpen(res, body);
      case 'change_password':
        return await changePassword(res, owner, body);
      default:
        throw new HttpError(400, 'Неизвестное действие');
    }
  } catch (err) {
    return fail(res, err);
  }
};
