'use strict';

const {
  DEVICE_ID_RE,
  HttpError,
  clientIp,
  fail,
  overLimit,
  readBody,
  recordHit,
  rpc,
  sameOrigin,
  sb,
  send,
} = require('./_lib');

const PUBLIC_STATUS = {
  pending: 'Заявка на рассмотрении у владельца проекта.',
  approved: 'Заявка одобрена. Токен получит ОС при ближайшей проверке: выполните в DeiX «dev request» или «dev on».',
  consumed: 'Одноразовый токен уже выдан устройству. Для нового токена подайте заявку ещё раз.',
  denied: 'Владелец проекта отклонил заявку.',
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
    if (req.method !== 'POST') {
      res.setHeader('Allow', 'POST');
      throw new HttpError(405, 'Метод не поддерживается');
    }
    if (!sameOrigin(req)) throw new HttpError(403, 'Запрос с чужого origin отклонён');
    const body = readBody(req);
    const ip = clientIp(req);

    if (body.action === 'status') {
      if (overLimit(`st:${ip}`, 60, 10 * 60 * 1000)) throw new HttpError(429, 'Слишком много запросов');
      recordHit(`st:${ip}`);
      const id = cleanDeviceId(body.device_id);
      const rows = await sb(`dev_unlocks?device_id=eq.${id}&select=status,created_at,decided_at`);
      if (!Array.isArray(rows) || !rows[0]) {
        return send(res, 200, { ok: true, status: 'none', message: 'Заявки с таким Device ID нет.' });
      }
      const row = rows[0];
      return send(res, 200, {
        ok: true,
        status: row.status,
        created_at: row.created_at,
        decided_at: row.decided_at,
        message: PUBLIC_STATUS[row.status] || '',
      });
    }

    if (body.action === 'submit') {
      if (body.website) throw new HttpError(400, 'Заявка отклонена');
      if (overLimit(`sub:${ip}`, 5, 10 * 60 * 1000)) {
        throw new HttpError(429, 'Слишком много заявок с этого адреса. Попробуйте позже.');
      }
      recordHit(`sub:${ip}`);
      const id = cleanDeviceId(body.device_id);
      const name = String(body.name || '').trim().slice(0, 64);
      const note = String(body.note || '').trim().slice(0, 280);
      const out = await rpc('request_dev_unlock', {
        p_device_id: id,
        p_hostname: name || null,
        p_note: note || null,
        p_source: 'web',
      });
      return send(res, 200, {
        ok: true,
        result: out.result,
        status: out.status,
        message: out.result === 'created' ? 'Заявка отправлена владельцу проекта.' : PUBLIC_STATUS[out.status] || '',
      });
    }

    throw new HttpError(400, 'Неизвестное действие');
  } catch (err) {
    return fail(res, err);
  }
};
