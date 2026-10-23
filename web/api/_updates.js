'use strict';

const nodeCrypto = require('node:crypto');
const { HttpError } = require('./_lib');

const MAGIC = 'DEIXMANIFEST1';
const DOMAIN = Buffer.from('DEIX-UPDATE-1\0', 'latin1');
const PART_SIZE = 128 * 1024;
const MAX_PAYLOAD = 6 * 1024 * 1024;
const MAX_MANIFEST = 16000;
const CHANNELS = ['stable', 'beta'];
const VERSION_RE = /^[0-9A-Za-z][0-9A-Za-z._-]{0,31}$/;
const SPKI_PREFIX = Buffer.from('3059301306072a8648ce3d020106082a8648ce3d030107034200', 'hex');

function payloadPath(build) {
  return `updates/${build}/payload.bin`;
}

function publicKey() {
  const hex = String(process.env.UPDATE_PUBKEY || '').trim().toLowerCase();
  if (!/^04[0-9a-f]{128}$/.test(hex)) return null;
  return nodeCrypto.createPublicKey({ key: Buffer.concat([SPKI_PREFIX, Buffer.from(hex, 'hex')]), format: 'der', type: 'spki' });
}

/**
 * Разбирает файл манифеста (строка-сигнатура, подпись, одна строка JSON).
 * Текст манифеста возвращается байт-в-байт: подпись считается по нему.
 */
function parseManifestFile(text) {
  const lines = String(text).split('\n');
  if (lines.length < 3 || lines[0] !== MAGIC) throw new HttpError(400, 'Файл манифеста не распознан');
  const signature = lines[1].trim().toLowerCase();
  const manifest = lines[2];
  if (!/^[0-9a-f]{128}$/.test(signature)) throw new HttpError(400, 'Подпись манифеста: нужно 128 hex-символов');
  if (manifest.length > MAX_MANIFEST || !/^\{[\x20-\x7e]*\}$/.test(manifest)) {
    throw new HttpError(400, 'Манифест должен быть одной строкой ASCII-JSON до 16000 символов');
  }
  let doc;
  try {
    doc = JSON.parse(manifest);
  } catch {
    throw new HttpError(400, 'Манифест не является корректным JSON');
  }
  const valid =
    doc !== null &&
    typeof doc === 'object' &&
    doc.product === 'deix' &&
    typeof doc.version === 'string' &&
    VERSION_RE.test(doc.version) &&
    Number.isSafeInteger(doc.build) &&
    doc.build >= 1 &&
    CHANNELS.includes(doc.channel) &&
    Number.isSafeInteger(doc.payload_size) &&
    doc.payload_size >= 1 &&
    doc.payload_size <= MAX_PAYLOAD &&
    typeof doc.payload_sha256 === 'string' &&
    /^[0-9a-f]{64}$/.test(doc.payload_sha256);
  if (!valid) throw new HttpError(400, 'Манифест содержит некорректные поля');

  const key = publicKey();
  if (key) {
    const ok = nodeCrypto.verify(
      'sha256',
      Buffer.concat([DOMAIN, Buffer.from(manifest, 'latin1')]),
      { key, dsaEncoding: 'ieee-p1363' },
      Buffer.from(signature, 'hex'),
    );
    if (!ok) throw new HttpError(400, 'Подпись манифеста не сходится с ключом обновлений');
  }
  return { manifest, signature, doc };
}

module.exports = { CHANNELS, MAX_PAYLOAD, PART_SIZE, parseManifestFile, payloadPath };
