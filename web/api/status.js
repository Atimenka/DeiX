'use strict';

const { HttpError, send, sb } = require('./_lib');

/**
 * GET /api/status — общественная информация о сайте из site_meta:
 * текущая версия и флаг обслуживания. Публичный запрос, без персональных данных.
 * Кэшируется на 60 секунд, чтобы переключать флаг обслуживания без
 * переdeployа и не перегружать Supabase на каждом запросе страницы.
 */

const CACHE_SECONDS = 60;

module.exports = async function handler(req, res) {
  try {
    res.setHeader('Cache-Control', `public, max-age=${CACHE_SECONDS}`);
    if (req.method !== 'GET') {
      throw new HttpError(405, 'Метод не поддерживается');
    }
    const rows = await sb('site_meta?select=key,value');
    const meta = {};
    if (Array.isArray(rows)) {
      for (const row of rows) {
        if (row && row.key != null) meta[row.key] = row.value;
      }
    }
    const maintenance = String(meta.maintenance || 'false') === 'true';
    const version = String(meta.site_version || '');
    send(res, 200, {
      status: 'ok',
      version,
      maintenance,
      cached: CACHE_SECONDS,
    });
  } catch (err) {
    // Нельзя показать сайт «вне обслуживания» из-за сбоя БД — отдаём
    // показатель с maintenance:false и версию ненайденной.
    if (err instanceof HttpError) {
      send(res, err.statusCode, { status: 'error', message: err.message });
      return;
    }
    send(res, 200, { status: 'ok', version: '', maintenance: false, cached: CACHE_SECONDS, degraded: true });
  }
};
