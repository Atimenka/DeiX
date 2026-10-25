'use strict';

const { DEVICE_ID_RE, HttpError, clientIp, overLimit, readBody, recordHit, rpc, send } = require('./_lib');

/**
 * Шлюз для ОС: DeiX ходит сюда, а не напрямую на supabase.co, который
 * режут провайдеры (DNS/SNI/IP). Разрешены четыре функции; остальное
 * отклоняется. Ответы повторяют формат PostgREST, который разбирает ОС:
 * при успехе — jsonb функции, при отказе — {"message": ...} и код 4xx/5xx.
 */
const FUNCTIONS = {
  begin_device_link: (id) => ({ p_device_id: id }),
  device_link_status: (id) => ({ p_device_id: id }),
  claim_dev_unlock: (id) => ({ p_device_id: id }),
  request_dev_unlock: (id, body) => ({
    p_device_id: id,
    p_hostname: typeof body.p_hostname === 'string' ? body.p_hostname.slice(0, 64) : null,
    p_source: 'os',
  }),
};

function reject(res, status, message) {
  return send(res, status, { message });
}

module.exports = async function handler(req, res) {
  try {
    if (req.method !== 'POST') {
      res.setHeader('Allow', 'POST');
      return reject(res, 405, 'POST only');
    }
    const url = new URL(req.url, 'http://x');
    const name = url.searchParams.get('fn') || '';
    const build = Object.prototype.hasOwnProperty.call(FUNCTIONS, name) ? FUNCTIONS[name] : null;
    if (!build) return reject(res, 404, 'unknown function');

    const body = readBody(req);
    const id = String(body.p_device_id || '').trim().toLowerCase();
    if (!DEVICE_ID_RE.test(id)) return reject(res, 400, 'device id must be 32 lowercase hex characters');

    const ipKey = `os-ip:${clientIp(req)}`;
    const devKey = `os-dev:${id}`;
    if (overLimit(ipKey, 120, 60 * 60 * 1000) || overLimit(devKey, 60, 60 * 60 * 1000)) {
      return reject(res, 429, 'too many requests, try again later');
    }
    recordHit(ipKey);
    recordHit(devKey);

    const out = await rpc(name, build(id, body));
    return send(res, 200, out);
  } catch (err) {
    if (err instanceof HttpError) return reject(res, err.status, err.message);
    console.error('os gateway error:', err && err.message);
    return reject(res, 500, 'internal error');
  }
};
