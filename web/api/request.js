'use strict';

const {
  DEVICE_ID_RE,
  HttpError,
  fail,
  guardPost,
  overLimit,
  readBody,
  recordHit,
  requireUser,
  rpc,
  sb,
  send,
} = require('./_lib');

const MESSAGES = {
  pending: 'Заявка на рассмотрении у владельца проекта.',
  approved: 'Заявка одобрена. Токен получит ОС при ближайшей проверке: выполните в DeiX «dev request» или «dev on».',
  consumed: 'Одноразовый токен уже выдан устройству. Для нового токена подайте заявку ещё раз.',
  denied: 'Владелец проекта отклонил заявку.',
};

const DB_ERRORS = {
  'this device is not linked to your account': 'Это устройство не привязано к вашему аккаунту. Привяжите его в разделе «Аккаунт».',
  'this account already has 3 pending requests': 'У вас уже 3 заявки на рассмотрении.',
  'the project owner is not accepting dev requests right now': 'Владелец проекта сейчас не принимает заявки.',
};

function cleanDeviceId(value) {
  const id = String(value || '').trim().toLowerCase();
  if (!DEVICE_ID_RE.test(id)) {
    throw new HttpError(400, 'Device ID — это 32 символа 0-9 и a-f. Его печатает команда «dev id» в DeiX.');
  }
  return id;
}

module.exports = async function handler(req, res) {
  try {
    guardPost(req, res);
    const user = await requireUser(req, res);
    const body = readBody(req);

    if (body.action === 'mine') {
      const rows = await sb(
        `dev_unlocks?user_id=eq.${user.id}&select=device_id,hostname,note,status,created_at,decided_at&order=created_at.desc&limit=50`,
      );
      return send(res, 200, {
        ok: true,
        requests: rows.map((row) => ({ ...row, message: MESSAGES[row.status] || '' })),
      });
    }

    if (body.action === 'submit') {
      if (overLimit(`sub:${user.id}`, 10, 60 * 60 * 1000)) {
        throw new HttpError(429, 'Слишком много заявок. Попробуйте позже.');
      }
      recordHit(`sub:${user.id}`);
      const id = cleanDeviceId(body.device_id);
      const name = String(body.name || '').trim().slice(0, 64);
      const note = String(body.note || '').trim().slice(0, 280);
      const out = await rpc('web_request_dev_unlock', {
        p_user_id: user.id,
        p_device_id: id,
        p_name: name || null,
        p_note: note || null,
      }).catch((err) => {
        throw err instanceof HttpError && DB_ERRORS[err.message] ? new HttpError(400, DB_ERRORS[err.message]) : err;
      });
      return send(res, 200, {
        ok: true,
        result: out.result,
        status: out.status,
        message: out.result === 'created' ? 'Заявка отправлена владельцу проекта.' : MESSAGES[out.status] || '',
      });
    }

    throw new HttpError(400, 'Неизвестное действие');
  } catch (err) {
    return fail(res, err);
  }
};
