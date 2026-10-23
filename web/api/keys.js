'use strict';

const { DEVICE_ID_RE, HttpError, fail, guardPost, overLimit, readBody, recordHit, requireUser, requireVerified, sb, send } = require('./_lib');
const { issueKey, keysConfigured } = require('./_keys');

const HOURLY_LIMIT = 5;

module.exports = async function handler(req, res) {
  try {
    guardPost(req, res);
    const user = await requireUser(req, res);
    const body = readBody(req);

    if (body.action === 'issue') {
      requireVerified(user);
      const id = String(body.device_id || '').trim().toLowerCase();
      if (!DEVICE_ID_RE.test(id)) {
        throw new HttpError(400, 'Device ID — это 32 символа 0-9 и a-f. Его печатает команда «dev id» в DeiX.');
      }
      if (!keysConfigured()) throw new HttpError(503, 'Выдача ключей не настроена на сервере');
      const settings = await sb('dev_settings?key=eq.requests_open&select=value');
      if (settings[0] && settings[0].value === false) {
        throw new HttpError(400, 'Владелец проекта сейчас не выдаёт ключи.');
      }
      const since = new Date(Date.now() - 60 * 60 * 1000).toISOString();
      const recent = await sb(`dev_keys?user_id=eq.${user.id}&created_at=gte.${encodeURIComponent(since)}&select=id`);
      if (recent.length >= HOURLY_LIMIT || overLimit(`key:${user.id}`, HOURLY_LIMIT, 60 * 60 * 1000)) {
        throw new HttpError(429, `Не больше ${HOURLY_LIMIT} ключей в час. Попробуйте позже.`);
      }
      recordHit(`key:${user.id}`);
      const issued = issueKey(id);
      await sb('dev_keys', {
        method: 'POST',
        headers: { Prefer: 'return=minimal' },
        body: { user_id: user.id },
      });
      return send(res, 200, { ok: true, key: issued.key, device_id: id });
    }

    throw new HttpError(400, 'Неизвестное действие');
  } catch (err) {
    return fail(res, err);
  }
};
