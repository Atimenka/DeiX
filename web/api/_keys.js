'use strict';

const crypto = require('node:crypto');

const ALPHABET = 'ABCDEFGHJKLMNPQRSTUVWXYZ23456789';
const DOMAIN = Buffer.from('DEIX-DEV-UNLOCK-1\0', 'latin1');
const NONCE_LEN = 8;
const SIGNATURE_LEN = 64;

function signedMessage(deviceId, nonce) {
  return Buffer.concat([DOMAIN, Buffer.from(deviceId, 'latin1'), nonce]);
}

function encode32(bytes) {
  let bits = 0;
  let acc = 0;
  let out = '';
  for (const byte of bytes) {
    acc = (acc << 8) | byte;
    bits += 8;
    while (bits >= 5) {
      out += ALPHABET[(acc >> (bits - 5)) & 31];
      bits -= 5;
    }
    acc &= (1 << bits) - 1;
  }
  if (bits > 0) out += ALPHABET[(acc << (5 - bits)) & 31];
  return out;
}

function group(text) {
  return text.match(/.{1,4}/g).join('-');
}

function privateKey() {
  const raw = process.env.DEV_KEY_PRIVATE || '';
  if (!raw) return null;
  return crypto.createPrivateKey({ key: Buffer.from(raw, 'base64'), format: 'der', type: 'pkcs8' });
}

function keysConfigured() {
  try {
    return privateKey() !== null;
  } catch {
    return false;
  }
}

/** Выдаёт ключ для device_id: nonce (8 байт) + подпись r||s (64 байта), base32 по 4 символа в группе. */
function issueKey(deviceId) {
  const key = privateKey();
  if (!key) throw new Error('DEV_KEY_PRIVATE is not set');
  const nonce = crypto.randomBytes(NONCE_LEN);
  const signature = crypto.sign('sha256', signedMessage(deviceId, nonce), { key, dsaEncoding: 'ieee-p1363' });
  if (signature.length !== SIGNATURE_LEN) throw new Error('unexpected signature length');
  return { key: group(encode32(Buffer.concat([nonce, signature]))), nonce: nonce.toString('hex') };
}

module.exports = { issueKey, keysConfigured, signedMessage, encode32 };
