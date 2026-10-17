'use strict';

const crypto = require('crypto');

const SUPABASE_URL = process.env.SUPABASE_URL || 'https://xtsrvmwfnsxunvwuaplz.supabase.co';
const SERVICE_KEY = process.env.SUPABASE_SERVICE_ROLE_KEY || '';
const SESSION_SECRET = process.env.SESSION_SECRET || '';

const COOKIE = 'deix_owner';
const SESSION_TTL_S = 12 * 60 * 60;
const DEVICE_ID_RE = /^[0-9a-f]{32}$/;

class HttpError extends Error {
  constructor(status, message) {
    super(message);
    this.status = status;
  }
}

function send(res, status, payload) {
  res.statusCode = status;
  res.setHeader('Content-Type', 'application/json; charset=utf-8');
  res.setHeader('Cache-Control', 'no-store');
  res.end(JSON.stringify(payload));
}

function fail(res, err) {
  if (err instanceof HttpError) {
    return send(res, err.status, { ok: false, error: err.message });
  }
  console.error('unhandled error:', err && err.message);
  return send(res, 500, { ok: false, error: 'Внутренняя ошибка сервера' });
}

function requireConfigured() {
  if (!SERVICE_KEY || !SESSION_SECRET) {
    throw new HttpError(503, 'Сервер не настроен: не заданы переменные окружения');
  }
}

async function sb(path, { method = 'GET', body, headers = {} } = {}) {
  requireConfigured();
  let resp;
  try {
    resp = await fetch(`${SUPABASE_URL}/rest/v1/${path}`, {
      method,
      headers: {
        apikey: SERVICE_KEY,
        Authorization: `Bearer ${SERVICE_KEY}`,
        'Content-Type': 'application/json',
        ...headers,
      },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
  } catch (e) {
    console.error('supabase unreachable:', e && e.message);
    throw new HttpError(502, 'База данных недоступна');
  }
  const text = await resp.text();
  let data = null;
  if (text) {
    try {
      data = JSON.parse(text);
    } catch {
      data = null;
    }
  }
  if (!resp.ok) {
    if (resp.status === 400 && data && data.code === 'P0001' && data.message) {
      throw new HttpError(400, data.message);
    }
    console.error('supabase error:', resp.status, data && data.message);
    throw new HttpError(502, 'Ошибка базы данных');
  }
  return data;
}

function rpc(name, args) {
  return sb(`rpc/${name}`, { method: 'POST', body: args });
}

function readBody(req) {
  const raw = req.body;
  if (raw && typeof raw === 'object') return raw;
  if (typeof raw === 'string' && raw) {
    try {
      const parsed = JSON.parse(raw);
      if (parsed && typeof parsed === 'object') return parsed;
    } catch {
      throw new HttpError(400, 'Некорректный JSON');
    }
  }
  return {};
}

function clientIp(req) {
  const fwd = req.headers['x-forwarded-for'];
  if (typeof fwd === 'string' && fwd) return fwd.split(',')[0].trim();
  return (req.socket && req.socket.remoteAddress) || 'unknown';
}

const hits = new Map();

function overLimit(key, max, windowMs) {
  const now = Date.now();
  const recent = (hits.get(key) || []).filter((t) => now - t < windowMs);
  hits.set(key, recent);
  if (hits.size > 5000) hits.clear();
  return recent.length >= max;
}

function recordHit(key) {
  const list = hits.get(key) || [];
  list.push(Date.now());
  hits.set(key, list);
}

function sameOrigin(req) {
  const origin = req.headers.origin;
  if (!origin) return true;
  try {
    return new URL(origin).host === req.headers.host;
  } catch {
    return false;
  }
}

function scrypt(password, salt, keylen, opts) {
  return new Promise((resolve, reject) => {
    crypto.scrypt(password, salt, keylen, opts, (err, key) => (err ? reject(err) : resolve(key)));
  });
}

const SCRYPT = { N: 16384, r: 8, p: 1 };

async function hashPassword(password) {
  const salt = crypto.randomBytes(16);
  const key = await scrypt(password, salt, 32, SCRYPT);
  return `scrypt$${SCRYPT.N}$${SCRYPT.r}$${SCRYPT.p}$${salt.toString('base64')}$${key.toString('base64')}`;
}

async function verifyPassword(password, stored) {
  const parts = String(stored || '').split('$');
  if (parts.length !== 6 || parts[0] !== 'scrypt') return false;
  const [, n, r, p, saltB64, keyB64] = parts;
  const expected = Buffer.from(keyB64, 'base64');
  const key = await scrypt(password, Buffer.from(saltB64, 'base64'), expected.length, {
    N: Number(n),
    r: Number(r),
    p: Number(p),
  });
  return key.length === expected.length && crypto.timingSafeEqual(key, expected);
}

function b64url(buf) {
  return Buffer.from(buf).toString('base64url');
}

function hmac(data) {
  return crypto.createHmac('sha256', SESSION_SECRET).update(data).digest();
}

function fingerprint(hash) {
  return crypto.createHash('sha256').update(String(hash)).digest('hex').slice(0, 16);
}

function issueSession(user, hash) {
  const payload = b64url(
    JSON.stringify({ u: user, v: fingerprint(hash), exp: Math.floor(Date.now() / 1000) + SESSION_TTL_S }),
  );
  return `${payload}.${b64url(hmac(payload))}`;
}

function sessionCookie(token) {
  return `${COOKIE}=${token}; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age=${SESSION_TTL_S}`;
}

function clearCookie() {
  return `${COOKIE}=; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age=0`;
}

function cookieValue(req, name) {
  const header = req.headers.cookie || '';
  for (const part of header.split(';')) {
    const idx = part.indexOf('=');
    if (idx > 0 && part.slice(0, idx).trim() === name) return part.slice(idx + 1).trim();
  }
  return '';
}

async function loadOwner() {
  const rows = await sb('dev_settings?key=eq.owner&select=value');
  const value = Array.isArray(rows) && rows[0] && rows[0].value;
  if (!value || !value.user || !value.hash) {
    throw new HttpError(503, 'Учётная запись Owner не создана');
  }
  return value;
}

async function currentOwner(req) {
  requireConfigured();
  const token = cookieValue(req, COOKIE);
  const dot = token.indexOf('.');
  if (dot < 1) return null;
  const payload = token.slice(0, dot);
  const given = Buffer.from(token.slice(dot + 1), 'base64url');
  const want = hmac(payload);
  if (given.length !== want.length || !crypto.timingSafeEqual(given, want)) return null;
  let data;
  try {
    data = JSON.parse(Buffer.from(payload, 'base64url').toString('utf8'));
  } catch {
    return null;
  }
  if (!data || typeof data.exp !== 'number' || data.exp < Date.now() / 1000) return null;
  const owner = await loadOwner();
  if (data.u !== owner.user || data.v !== fingerprint(owner.hash)) return null;
  return owner;
}

module.exports = {
  DEVICE_ID_RE,
  HttpError,
  clearCookie,
  clientIp,
  currentOwner,
  fail,
  hashPassword,
  issueSession,
  loadOwner,
  overLimit,
  readBody,
  recordHit,
  rpc,
  sameOrigin,
  sb,
  send,
  sessionCookie,
  verifyPassword,
};
