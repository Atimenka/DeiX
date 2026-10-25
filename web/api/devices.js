'use strict';

const { DEVICE_ID_RE, HttpError, fail, guardPost, overLimit, readBody, recordHit, requireUser, rpc, sb, send } = require('./_lib');

const LINK_ERRORS = {
  'invalid or expired code': 'Код неверный или просрочен. Выполните «dev link» в DeiX и введите новый код.',
  'you can link at most 5 devices': 'К аккаунту можно привязать не больше 5 устройств.',
};

function translated(err, map) {
  if (err instanceof HttpError && err.status === 400 && map[err.message]) {
    return new HttpError(400, map[err.message]);
  }
  return err;
}

module.exports = async function handler(req, res) {
  try {
    guardPost(req, res);
    const user = await requireUser(req, res);
    const body = readBody(req);

    if (body.action === 'list') {
      const devices = await sb(`device_links?user_id=eq.${user.id}&select=device_id,linked_at&order=linked_at.desc`);
      return send(res, 200, { ok: true, devices });
    }

    if (body.action === 'link') {
      const key = `link:${user.id}`;
      if (overLimit(key, 10, 15 * 60 * 1000)) {
        throw new HttpError(429, 'Слишком много неверных кодов. Попробуйте через 15 минут.');
      }
      const code = String(body.code || '').trim();
      if (!/^[A-Za-z0-9]{4}-?[A-Za-z0-9]{4}$/.test(code)) {
        throw new HttpError(400, 'Код состоит из 8 символов вида XXXX-XXXX.');
      }
      try {
        const out = await rpc('link_device', { p_user_id: user.id, p_code: code });
        return send(res, 200, { ok: true, device_id: out.device_id });
      } catch (err) {
        recordHit(key);
        throw translated(err, LINK_ERRORS);
      }
    }

    if (body.action === 'unlink') {
      const id = String(body.device_id || '').trim().toLowerCase();
      if (!DEVICE_ID_RE.test(id)) throw new HttpError(400, 'Некорректный Device ID');
      const removed = await rpc('unlink_device', { p_user_id: user.id, p_device_id: id });
      if (!removed) throw new HttpError(404, 'Такого устройства в вашем аккаунте нет');
      return send(res, 200, { ok: true });
    }

    throw new HttpError(400, 'Неизвестное действие');
  } catch (err) {
    return fail(res, err);
  }
};
