'use strict';

const { DEVICE_ID_RE, HttpError, fail, guardPost, readBody, requireOwner, rpc, sb, send, storage, storageUrl } = require('./_lib');
const { BUCKET, EXT_RE, MAX_SIZE, UUID_RE, VERSION_RE, objectPath } = require('./_releases');

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

const RELEASE_COLUMNS =
  'id,version,title,notes,file_name,size_bytes,sha256,ready,published,downloads,created_by,created_at,published_at';

function releaseId(body) {
  const id = String(body.id || '').trim().toLowerCase();
  if (!UUID_RE.test(id)) throw new HttpError(400, 'Некорректный идентификатор версии');
  return id;
}

async function releaseRow(id) {
  const rows = await sb(`os_releases?id=eq.${id}&select=id,version,file_path,size_bytes,ready`);
  if (!rows[0]) throw new HttpError(404, 'Версия не найдена');
  return rows[0];
}

async function releasesList(res) {
  const releases = await sb(`os_releases?select=${RELEASE_COLUMNS}&order=created_at.desc&limit=200`);
  return send(res, 200, { ok: true, releases });
}

async function releasePrepare(res, owner, body) {
  const version = String(body.version || '').trim();
  if (!VERSION_RE.test(version)) {
    throw new HttpError(400, 'Версия: до 32 символов, латинские буквы, цифры, точка, дефис и _');
  }
  const title = String(body.title || '').trim();
  const notes = String(body.notes || '').trim();
  if (title.length > 120) throw new HttpError(400, 'Название не длиннее 120 символов');
  if (notes.length > 4000) throw new HttpError(400, 'Описание не длиннее 4000 символов');
  const size = Number(body.size);
  if (!Number.isInteger(size) || size < 1) throw new HttpError(400, 'Файл пустой');
  if (size > MAX_SIZE) throw new HttpError(400, 'Файл больше 50 МиБ: это предел хранилища');
  const sha256 = String(body.sha256 || '').trim().toLowerCase();
  if (!/^[0-9a-f]{64}$/.test(sha256)) throw new HttpError(400, 'Некорректная контрольная сумма SHA-256');
  const ext = EXT_RE.exec(String(body.filename || ''));
  if (!ext) throw new HttpError(400, 'Допустимые файлы: .img, .img.gz, .img.xz, .zip, .qcow2');

  const fileName = `deix-${version}.${ext[1].toLowerCase()}`;
  const path = objectPath(version, fileName);
  const fields = { title, notes, file_path: path, file_name: fileName, size_bytes: size, sha256, ready: false, published: false };

  const existing = await sb(`os_releases?version=eq.${version}&select=id,ready`);
  let id;
  if (existing[0]) {
    if (existing[0].ready) throw new HttpError(409, 'Такая версия уже добавлена');
    id = existing[0].id;
    await sb(`os_releases?id=eq.${id}`, { method: 'PATCH', headers: { Prefer: 'return=minimal' }, body: fields });
  } else {
    const rows = await sb('os_releases', {
      method: 'POST',
      headers: { Prefer: 'return=representation' },
      body: { version, created_by: owner.username, ...fields },
    });
    id = rows[0].id;
  }

  const signed = await storage(`object/upload/sign/${BUCKET}/${path}`, { method: 'POST', headers: { 'x-upsert': 'true' } });
  if (signed.status !== 200 || !signed.data || typeof signed.data.url !== 'string') {
    throw new HttpError(502, 'Не удалось подготовить загрузку в хранилище');
  }
  return send(res, 200, { ok: true, id, upload_url: storageUrl(signed.data.url) });
}

async function releaseFinalize(res, body) {
  const row = await releaseRow(releaseId(body));
  const info = await storage(`object/info/${BUCKET}/${row.file_path}`);
  if (info.status === 404 || info.status === 400) {
    throw new HttpError(400, 'Файла нет в хранилище: загрузка не завершилась');
  }
  if (info.status !== 200 || !info.data) throw new HttpError(502, 'Не удалось проверить файл в хранилище');
  const stored = Number(info.data.size !== undefined ? info.data.size : info.data.metadata && info.data.metadata.size);
  if (stored !== Number(row.size_bytes)) {
    await storage(`object/${BUCKET}`, { method: 'DELETE', body: { prefixes: [row.file_path] } });
    throw new HttpError(400, 'Размер загруженного файла не совпал с заявленным, файл удалён');
  }
  const publish = body.publish === true;
  await sb(`os_releases?id=eq.${row.id}`, {
    method: 'PATCH',
    headers: { Prefer: 'return=minimal' },
    body: { ready: true, published: publish, published_at: publish ? new Date().toISOString() : null },
  });
  return send(res, 200, { ok: true, published: publish });
}

async function releasePublish(res, body) {
  if (typeof body.published !== 'boolean') throw new HttpError(400, 'Нужно значение published: true или false');
  const row = await releaseRow(releaseId(body));
  if (!row.ready) throw new HttpError(400, 'Файл версии ещё не загружен');
  await sb(`os_releases?id=eq.${row.id}`, {
    method: 'PATCH',
    headers: { Prefer: 'return=minimal' },
    body: { published: body.published, published_at: body.published ? new Date().toISOString() : null },
  });
  return send(res, 200, { ok: true, published: body.published });
}

async function releaseRemove(res, body) {
  const row = await releaseRow(releaseId(body));
  const removed = await storage(`object/${BUCKET}`, { method: 'DELETE', body: { prefixes: [row.file_path] } });
  if (removed.status >= 500) throw new HttpError(502, 'Не удалось удалить файл из хранилища');
  await sb(`os_releases?id=eq.${row.id}`, { method: 'DELETE', headers: { Prefer: 'return=minimal' } });
  return send(res, 200, { ok: true });
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
      case 'releases':
        return await releasesList(res);
      case 'release_prepare':
        return await releasePrepare(res, owner, body);
      case 'release_finalize':
        return await releaseFinalize(res, body);
      case 'release_publish':
        return await releasePublish(res, body);
      case 'release_remove':
        return await releaseRemove(res, body);
      default:
        throw new HttpError(400, 'Неизвестное действие');
    }
  } catch (err) {
    return fail(res, err);
  }
};
