'use strict';

const { HttpError, clientIp, fail, overLimit, recordHit, rpc, sb, send, storage, storageUrl } = require('./_lib');
const { BUCKET, VERSION_RE } = require('./_releases');
const { requestLang } = require('./_i18n');

const DOWNLOADS_PER_HOUR = 30;
const LINK_TTL_S = 300;
const PUBLIC_COLUMNS = 'version,title,notes,file_name,size_bytes,sha256,downloads,published_at';

async function list(res) {
  const releases = await sb(
    `os_releases?ready=eq.true&published=eq.true&select=${PUBLIC_COLUMNS}&order=published_at.desc&limit=100`,
  );
  return send(res, 200, { ok: true, releases });
}

async function download(req, res, version) {
  if (!VERSION_RE.test(version)) throw new HttpError(400, 'Некорректная версия');
  const key = `dl:${clientIp(req)}`;
  if (overLimit(key, DOWNLOADS_PER_HOUR, 60 * 60 * 1000)) {
    throw new HttpError(429, 'Слишком много загрузок с этого адреса. Попробуйте позже.');
  }
  const rows = await sb(`os_releases?version=eq.${version}&ready=eq.true&published=eq.true&select=id,file_path,file_name`);
  if (!rows[0]) throw new HttpError(404, 'Такой версии нет или она не опубликована');
  const signed = await storage(`object/sign/${BUCKET}/${rows[0].file_path}`, { method: 'POST', body: { expiresIn: LINK_TTL_S } });
  if (signed.status !== 200 || !signed.data || typeof signed.data.signedURL !== 'string') {
    throw new HttpError(502, 'Не удалось получить ссылку на файл');
  }
  recordHit(key);
  try {
    await rpc('bump_release_downloads', { p_id: rows[0].id });
  } catch (err) {
    console.error('download counter failed:', err && err.message);
  }
  res.statusCode = 302;
  res.setHeader('Location', `${storageUrl(signed.data.signedURL)}&download=${encodeURIComponent(rows[0].file_name)}`);
  res.setHeader('Cache-Control', 'no-store');
  res.end();
}

module.exports = async function handler(req, res) {
  try {
    res.deixLang = requestLang(req);
    if (req.method !== 'GET') {
      res.setHeader('Allow', 'GET');
      throw new HttpError(405, 'Метод не поддерживается');
    }
    const version = new URL(req.url, 'http://x').searchParams.get('download');
    if (version !== null) return await download(req, res, version);
    return await list(res);
  } catch (err) {
    return fail(res, err);
  }
};
