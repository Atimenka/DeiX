'use strict';

const { DEVICE_ID_RE, HttpError, fail, guardPost, readBody, requireOwner, rpc, sb, send } = require('./_lib');

const LIST_COLUMNS =
  'device_id,hostname,note,source,status,created_at,decided_at,decided_by,consumed_at,profiles(username)';

function deviceId(body) {
  const id = String(body.device_id || '').trim().toLowerCase();
  if (!DEVICE_ID_RE.test(id)) throw new HttpError(400, 'Некорректный Device ID');
  return id;
}

async function list(res) {
  const [rows, settings, users] = await Promise.all([
    sb(`dev_unlocks?select=${LIST_COLUMNS}&order=created_at.desc&limit=500`),
    sb('dev_settings?key=eq.requests_open&select=value'),
    sb('profiles?select=id&limit=1000'),
  ]);
  const counts = { pending: 0, approved: 0, consumed: 0, denied: 0 };
  for (const row of rows) counts[row.status] = (counts[row.status] || 0) + 1;
  const requests = rows.map(({ profiles, ...row }) => ({ ...row, requester: profiles ? profiles.username : null }));
  const open = !(settings[0] && settings[0].value === false);
  return send(res, 200, { ok: true, requests, counts, requests_open: open, users_total: users.length });
}

async function decide(res, owner, body) {
  const id = deviceId(body);
  const name = body.decision === 'approve' ? 'approve_dev_unlock' : body.decision === 'deny' ? 'deny_dev_unlock' : '';
  if (!name) throw new HttpError(400, 'Неизвестное решение');
  const row = await rpc(name, { p_device_id: id, p_by: owner.username });
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

module.exports = async function handler(req, res) {
  try {
    guardPost(req, res);
    const owner = await requireOwner(req, res);
    const body = readBody(req);
    switch (body.action) {
      case 'list':
        return await list(res);
      case 'decide':
        return await decide(res, owner, body);
      case 'remove':
        return await remove(res, body);
      case 'set_open':
        return await setOpen(res, body);
      default:
        throw new HttpError(400, 'Неизвестное действие');
    }
  } catch (err) {
    return fail(res, err);
  }
};
