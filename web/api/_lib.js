'use strict';

const SUPABASE_URL = process.env.SUPABASE_URL || 'https://xtsrvmwfnsxunvwuaplz.supabase.co';
const SERVICE_KEY = process.env.SUPABASE_SERVICE_ROLE_KEY || '';
const ANON_KEY = process.env.SUPABASE_ANON_KEY || '';
const TURNSTILE_SECRET = process.env.TURNSTILE_SECRET || '';
const TURNSTILE_SITE_KEY = process.env.TURNSTILE_SITE_KEY || '';

const { mailConfigured } = require('./_mail');
const { requestLang, translatePayload } = require('./_i18n');

const ACCESS_COOKIE = 'deix_at';
const REFRESH_COOKIE = 'deix_rt';
const COOKIE_TTL_S = 30 * 24 * 60 * 60;
const DEVICE_ID_RE = /^[0-9a-f]{32}$/;
const USERNAME_RE = /^[A-Za-z0-9_]{3,24}$/;
const EMAIL_RE = /^[^\s@]{1,64}@[^\s@]{1,255}\.[^\s@]{2,}$/;
const MIN_PASSWORD = 8;

class HttpError extends Error {
  constructor(status, message, code) {
    super(message);
    this.status = status;
    this.code = code;
  }
}

function send(res, status, payload) {
  res.statusCode = status;
  res.setHeader('Content-Type', 'application/json; charset=utf-8');
  if (!res.getHeader('Cache-Control')) res.setHeader('Cache-Control', 'no-store');
  res.end(JSON.stringify(translatePayload(payload, res.deixLang)));
}

function fail(res, err) {
  if (err instanceof HttpError) {
    return send(res, err.status, err.code ? { ok: false, error: err.message, code: err.code } : { ok: false, error: err.message });
  }
  console.error('unhandled error:', err && err.message);
  return send(res, 500, { ok: false, error: 'Внутренняя ошибка сервера' });
}

function requireConfigured() {
  if (!SERVICE_KEY || !ANON_KEY) {
    throw new HttpError(503, 'Сервер не настроен: не заданы переменные окружения');
  }
}

async function parseResponse(resp) {
  const text = await resp.text();
  if (!text) return null;
  try {
    return JSON.parse(text);
  } catch {
    return null;
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
  const data = await parseResponse(resp);
  if (!resp.ok) {
    if (resp.status === 400 && data && data.code === 'P0001' && data.message) {
      throw new HttpError(400, data.message);
    }
    console.error('supabase error:', resp.status, data && data.message);
    throw new HttpError(502, 'Ошибка базы данных');
  }
  return data;
}

/** Запрос к Supabase Storage. Возвращает { status, data } без выброса на 4xx. */
async function storage(path, { method = 'GET', body, headers = {} } = {}) {
  requireConfigured();
  try {
    const resp = await fetch(`${SUPABASE_URL}/storage/v1/${path}`, {
      method,
      headers: {
        apikey: SERVICE_KEY,
        Authorization: `Bearer ${SERVICE_KEY}`,
        'Content-Type': 'application/json',
        ...headers,
      },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    return { status: resp.status, data: await parseResponse(resp) };
  } catch (e) {
    console.error('supabase storage unreachable:', e && e.message);
    throw new HttpError(502, 'Хранилище файлов недоступно');
  }
}

/** Читает диапазон байт объекта приватного бакета. */
async function storageBytes(path, start, end) {
  requireConfigured();
  let resp;
  try {
    resp = await fetch(`${SUPABASE_URL}/storage/v1/${path}`, {
      headers: { apikey: SERVICE_KEY, Authorization: `Bearer ${SERVICE_KEY}`, Range: `bytes=${start}-${end}` },
    });
  } catch (e) {
    console.error('supabase storage unreachable:', e && e.message);
    throw new HttpError(502, 'Хранилище файлов недоступно');
  }
  if (resp.status !== 200 && resp.status !== 206) return null;
  const bytes = Buffer.from(await resp.arrayBuffer());
  return resp.status === 206 ? bytes : bytes.subarray(start, end + 1);
}

/** Абсолютный адрес по относительному пути, который вернул Storage (`/object/...`). */
function storageUrl(relative) {
  return `${SUPABASE_URL}/storage/v1${relative}`;
}

function rpc(name, args) {
  return sb(`rpc/${name}`, { method: 'POST', body: args });
}

/** Запрос к Supabase Auth. Возвращает { status, data } без выброса на 4xx. */
async function auth(path, { method = 'POST', body, token, admin = false } = {}) {
  requireConfigured();
  const key = admin ? SERVICE_KEY : ANON_KEY;
  try {
    const resp = await fetch(`${SUPABASE_URL}/auth/v1/${path}`, {
      method,
      headers: {
        apikey: key,
        Authorization: `Bearer ${token || key}`,
        'Content-Type': 'application/json',
      },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    return { status: resp.status, data: await parseResponse(resp) };
  } catch (e) {
    console.error('supabase auth unreachable:', e && e.message);
    throw new HttpError(502, 'Сервер аккаунтов недоступен');
  }
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

/**
 * Грубый потолок запросов с одного адреса. Счётчик живёт в памяти одного
 * экземпляра функции, поэтому это защита от одиночного шумного клиента, а не
 * от распределённого потока: от него — фильтры Vercel (Attack Challenge Mode).
 */
function floodGuard(req) {
  const max = Number(process.env.FLOOD_MAX_PER_MINUTE) || 300;
  const key = `flood:${clientIp(req)}`;
  if (overLimit(key, max, 60 * 1000)) {
    throw new HttpError(429, 'Слишком много запросов с этого адреса. Попробуйте позже.');
  }
  recordHit(key);
}

/** Общая защита всех POST-эндпоинтов: метод, origin и заголовок против CSRF. */
function guardPost(req, res) {
  res.deixLang = requestLang(req);
  floodGuard(req);
  if (req.method !== 'POST') {
    res.setHeader('Allow', 'POST');
    throw new HttpError(405, 'Метод не поддерживается');
  }
  if (!sameOrigin(req) || req.headers['x-deix-csrf'] !== '1') {
    throw new HttpError(403, 'Запрос отклонён');
  }
}

function captchaEnabled() {
  return Boolean(TURNSTILE_SECRET && TURNSTILE_SITE_KEY);
}

/** Проверка токена Cloudflare Turnstile. Без ключей проверка не выполняется. */
async function verifyCaptcha(token, ip) {
  if (!captchaEnabled()) return;
  if (!token || typeof token !== 'string') {
    throw new HttpError(400, 'Подтвердите, что вы не робот');
  }
  let data = null;
  try {
    const resp = await fetch('https://challenges.cloudflare.com/turnstile/v0/siteverify', {
      method: 'POST',
      headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
      body: new URLSearchParams({ secret: TURNSTILE_SECRET, response: token, remoteip: ip }).toString(),
    });
    data = await parseResponse(resp);
  } catch (e) {
    console.error('turnstile unreachable:', e && e.message);
    throw new HttpError(502, 'Не удалось проверить капчу, попробуйте ещё раз');
  }
  if (!data || data.success !== true) {
    throw new HttpError(400, 'Капча не пройдена, попробуйте ещё раз');
  }
}

function cookieValue(req, name) {
  const header = req.headers.cookie || '';
  for (const part of header.split(';')) {
    const idx = part.indexOf('=');
    if (idx > 0 && part.slice(0, idx).trim() === name) return part.slice(idx + 1).trim();
  }
  return '';
}

function cookie(name, value, maxAge, path = '/') {
  return `${name}=${value}; HttpOnly; Secure; SameSite=Strict; Path=${path}; Max-Age=${maxAge}`;
}

function setSession(res, session, extra = []) {
  res.setHeader('Set-Cookie', [
    cookie(ACCESS_COOKIE, session.access_token, COOKIE_TTL_S),
    cookie(REFRESH_COOKIE, session.refresh_token, COOKIE_TTL_S),
    ...extra,
  ]);
}

/** Уровень аутентификации из access-токена (aal1 — пароль, aal2 — пароль и TOTP). */
function tokenAal(token) {
  try {
    const payload = JSON.parse(Buffer.from(String(token).split('.')[1], 'base64url').toString('utf8'));
    return payload.aal === 'aal2' ? 'aal2' : 'aal1';
  } catch {
    return 'aal1';
  }
}

function verifiedTotp(user) {
  const list = (user && Array.isArray(user.factors) ? user.factors : []).filter(
    (f) => f.factor_type === 'totp' && f.status === 'verified',
  );
  return list[0] || null;
}

function clearSession(res) {
  res.setHeader('Set-Cookie', [cookie(ACCESS_COOKIE, '', 0), cookie(REFRESH_COOKIE, '', 0)]);
}

async function loadProfile(userId) {
  const rows = await sb(`profiles?id=eq.${userId}&select=id,username,role,email_verified`);
  return Array.isArray(rows) && rows[0] ? rows[0] : null;
}

/**
 * Текущий пользователь по cookie. Просроченный access-токен обновляется
 * refresh-токеном, новые cookie ставятся в ответ. null — не вошёл.
 */
async function currentUser(req, res) {
  requireConfigured();
  let access = cookieValue(req, ACCESS_COOKIE);
  const refresh = cookieValue(req, REFRESH_COOKIE);
  let user = null;

  if (access) {
    const r = await auth('user', { method: 'GET', token: access });
    if (r.status === 200 && r.data && r.data.id) user = r.data;
  }
  if (!user && refresh) {
    const r = await auth('token?grant_type=refresh_token', { body: { refresh_token: refresh } });
    if (r.status === 200 && r.data && r.data.access_token && r.data.user) {
      access = r.data.access_token;
      user = r.data.user;
      setSession(res, r.data);
    } else {
      clearSession(res);
    }
  }
  if (!user) return null;

  const profile = await loadProfile(user.id);
  if (!profile) return null;
  return {
    id: user.id,
    email: user.email,
    username: profile.username,
    role: profile.role,
    emailVerified: profile.email_verified === true,
    factors: Array.isArray(user.factors) ? user.factors : [],
    aal: tokenAal(access),
    token: access,
  };
}

async function requireUser(req, res) {
  const user = await currentUser(req, res);
  if (!user) throw new HttpError(401, 'Требуется вход в аккаунт');
  return user;
}

async function requireOwner(req, res) {
  const user = await requireUser(req, res);
  if (user.role !== 'owner') throw new HttpError(403, 'Нужны права Owner');
  if (user.aal !== 'aal2') {
    throw new HttpError(403, 'Для панели Owner нужна двухфакторная аутентификация: включите её в аккаунте и войдите заново.', 'mfa_required');
  }
  return user;
}

/** Без подтверждённой почты ключи и заявки недоступны — но только если сервер умеет слать письма. */
function requireVerified(user) {
  if (mailConfigured() && !user.emailVerified) {
    throw new HttpError(403, 'Подтвердите почту: ссылка отправлена в письме при регистрации.', 'email_unverified');
  }
}

module.exports = {
  ACCESS_COOKIE,
  REFRESH_COOKIE,
  cookie,
  cookieValue,
  requireVerified,
  tokenAal,
  verifiedTotp,
  DEVICE_ID_RE,
  EMAIL_RE,
  HttpError,
  MIN_PASSWORD,
  TURNSTILE_SITE_KEY,
  USERNAME_RE,
  auth,
  captchaEnabled,
  clearSession,
  clientIp,
  currentUser,
  fail,
  floodGuard,
  guardPost,
  overLimit,
  readBody,
  recordHit,
  requireOwner,
  requireUser,
  rpc,
  sb,
  send,
  setSession,
  storage,
  storageBytes,
  storageUrl,
  verifyCaptcha,
};
