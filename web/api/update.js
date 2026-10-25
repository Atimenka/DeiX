'use strict';

const { HttpError, clientIp, fail, floodGuard, overLimit, recordHit, rpc, sb, storageBytes } = require('./_lib');
const { CHANNELS, PART_SIZE } = require('./_updates');
const { requestLang } = require('./_i18n');

const MANIFESTS_PER_HOUR = 120;
const PARTS_PER_HOUR = 600;
const WHOLE_PER_HOUR = 30;
const HOUR_MS = 60 * 60 * 1000;

function limit(req, kind, max) {
  const key = `${kind}:${clientIp(req)}`;
  if (overLimit(key, max, HOUR_MS)) {
    throw new HttpError(429, 'Слишком много запросов с этого адреса. Попробуйте позже.');
  }
  recordHit(key);
}

/** Манифест отдаётся ровно в том виде, в котором подписан: он вставляется в ответ как текст. */
async function manifest(req, res, channel) {
  if (!CHANNELS.includes(channel)) throw new HttpError(400, 'Неизвестный канал обновлений');
  limit(req, 'upd', MANIFESTS_PER_HOUR);
  const rows = await sb(
    `os_updates?channel=eq.${channel}&published=eq.true&ready=eq.true&select=build,manifest,signature,payload_size&order=build.desc&limit=1`,
  );
  res.statusCode = 200;
  res.setHeader('Content-Type', 'application/json; charset=utf-8');
  res.setHeader('Cache-Control', 'public, s-maxage=60, stale-while-revalidate=120');
  if (!rows[0]) return res.end('{"none":true}');
  const row = rows[0];
  const parts = Math.ceil(Number(row.payload_size) / PART_SIZE);
  return res.end(`{"manifest":${row.manifest},"parts":${parts},"part_size":${PART_SIZE},"signature":"${row.signature}"}`);
}

/** Весь payload одним ответом: ОС делает одно рукопожатие вместо многих. */
async function whole(req, res, params) {
  const build = Number(params.get('build'));
  if (!Number.isSafeInteger(build) || build < 1) {
    throw new HttpError(400, 'Некорректный номер сборки');
  }
  limit(req, 'upw', WHOLE_PER_HOUR);
  const rows = await sb(`os_updates?build=eq.${build}&published=eq.true&ready=eq.true&select=payload_path,payload_size`);
  if (!rows[0]) throw new HttpError(404, 'Такой сборки нет или она не опубликована');
  const size = Number(rows[0].payload_size);
  const bytes = await storageBytes(`object/authenticated/os-images/${rows[0].payload_path}`, 0, size - 1);
  if (!bytes || bytes.length !== size) throw new HttpError(502, 'Не удалось прочитать файл из хранилища');
  res.statusCode = 200;
  res.setHeader('Content-Type', 'application/octet-stream');
  res.setHeader('Content-Length', String(bytes.length));
  res.setHeader('Cache-Control', 'no-store');
  return res.end(bytes);
}

async function part(req, res, params) {
  const build = Number(params.get('build'));
  const index = Number(params.get('part'));
  if (!Number.isSafeInteger(build) || build < 1 || !Number.isSafeInteger(index) || index < 0) {
    throw new HttpError(400, 'Некорректный номер сборки или части');
  }
  limit(req, 'updp', PARTS_PER_HOUR);
  const rows = await sb(`os_updates?build=eq.${build}&published=eq.true&ready=eq.true&select=id,payload_path,payload_size,downloads`);
  if (!rows[0]) throw new HttpError(404, 'Такой сборки нет или она не опубликована');
  const size = Number(rows[0].payload_size);
  const start = index * PART_SIZE;
  if (start >= size) throw new HttpError(404, 'Такой части нет');
  const end = Math.min(start + PART_SIZE, size) - 1;
  const bytes = await storageBytes(`object/authenticated/os-images/${rows[0].payload_path}`, start, end);
  if (!bytes || bytes.length !== end - start + 1) throw new HttpError(502, 'Не удалось прочитать файл из хранилища');
  if (end + 1 === size) {
    try {
      await rpc('bump_update_downloads', { p_id: rows[0].id });
    } catch (err) {
      console.error('update counter failed:', err && err.message);
    }
  }
  res.statusCode = 200;
  res.setHeader('Content-Type', 'application/octet-stream');
  res.setHeader('Content-Length', String(bytes.length));
  return res.end(bytes);
}

module.exports = async function handler(req, res) {
  try {
    res.deixLang = requestLang(req);
    floodGuard(req);
    if (req.method !== 'GET') {
      res.setHeader('Allow', 'GET');
      throw new HttpError(405, 'Метод не поддерживается');
    }
    const params = new URL(req.url, 'http://x').searchParams;
    if (params.get('whole') !== null) return await whole(req, res, params);
    if (params.get('part') !== null) return await part(req, res, params);
    return await manifest(req, res, params.get('channel') || 'stable');
  } catch (err) {
    return fail(res, err);
  }
};
