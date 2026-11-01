'use strict';

const crypto = require('node:crypto');
const { checkEmailDomain } = require('./_emailcheck');
const { mailConfigured, sendMail } = require('./_mail');
const { MAIL, requestLang } = require('./_i18n');
const {
  EMAIL_RE,
  HttpError,
  MIN_PASSWORD,
  TURNSTILE_SITE_KEY,
  USERNAME_RE,
  auth,
  captchaEnabled,
  clearSession,
  cookie,
  cookieValue,
  clientIp,
  currentUser,
  fail,
  guardPost,
  overLimit,
  readBody,
  recordHit,
  requireUser,
  rpc,
  sb,
  send,
  setSession,
  verifiedTotp,
  verifyCaptcha,
} = require('./_lib');

const REGISTER_MAX = 5;
const REGISTER_WINDOW_MS = 60 * 60 * 1000;
const LOGIN_MAX_FAILS = 8;
const LOGIN_WINDOW_MS = 15 * 60 * 1000;

const ME_CACHE_TTL_MS = 20 * 1000;
const ME_CACHE_MAX = 200;
const meCache = new Map();

const MFA_COOKIE = 'deix_mfa';
const MFA_COOKIE_TTL_S = 5 * 60;
const MFA_MAX_FAILS = 5;
const MFA_WINDOW_MS = 10 * 60 * 1000;
const VERIFY_TTL_MS = 24 * 60 * 60 * 1000;
const VERIFY_SEND_MAX = 3;
const VERIFY_SEND_WINDOW_MS = 60 * 60 * 1000;
const BACKUP_ALPHABET = 'ABCDEFGHJKLMNPQRSTUVWXYZ23456789';
const BACKUP_COUNT = 8;
const BACKUP_LENGTH = 10;

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const sha256 = (value) => crypto.createHash('sha256').update(value).digest('hex');

function publicUser(user) {
  return {
    id: user.id,
    username: user.username,
    email: user.email,
    role: user.role,
    email_verified: user.emailVerified === true,
    mfa: Boolean(verifiedTotp(user)),
    aal: user.aal,
  };
}

// Базовый URL сайта для ссылок в письмах. Host берём из запроса: на Vercel
// его ставит край после terminate TLS, подделать его клиент не может;
// в production дополнительно переопределён переменной SITE_URL.
function siteUrl(req) {
  const fixed = (process.env.SITE_URL || '').replace(/\/+$/, '');
  if (fixed) return fixed;
  return `https://${String(req.headers['x-forwarded-host'] || req.headers.host || '').split(',')[0].trim()}`;
}

function validatePassword(password) {
  if (password.length < MIN_PASSWORD) {
    throw new HttpError(400, `Пароль должен быть не короче ${MIN_PASSWORD} символов`);
  }
  if (password.length > 72) throw new HttpError(400, 'Пароль не должен быть длиннее 72 символов');
}

async function signIn(email, password) {
  const r = await auth('token?grant_type=password', { body: { email, password } });
  if (r.status !== 200 || !r.data || !r.data.access_token) return null;
  return r.data;
}

async function register(req, res, body) {
  const ip = clientIp(req);
  if (body.website) throw new HttpError(400, 'Регистрация отклонена');
  if (overLimit(`reg:${ip}`, REGISTER_MAX, REGISTER_WINDOW_MS)) {
    throw new HttpError(429, 'Слишком много регистраций с этого адреса. Попробуйте позже.');
  }
  const username = String(body.username || '').trim();
  const email = String(body.email || '').trim().toLowerCase();
  const password = String(body.password || '');
  const password2 = String(body.password2 || '');

  if (!USERNAME_RE.test(username)) {
    throw new HttpError(400, 'Имя пользователя: 3–24 символа, латинские буквы, цифры и _');
  }
  if (!EMAIL_RE.test(email)) throw new HttpError(400, 'Укажите корректную почту');
  validatePassword(password);
  if (password !== password2) throw new HttpError(400, 'Пароли не совпадают');
  await verifyCaptcha(body.captcha, ip);
  await checkEmailDomain(email);
  recordHit(`reg:${ip}`);

  const taken = await sb(`profiles?username=ilike.${encodeURIComponent(username)}&select=id&limit=1`);
  if (Array.isArray(taken) && taken.length) throw new HttpError(409, 'Это имя пользователя уже занято');

  const created = await auth('admin/users', {
    admin: true,
    body: { email, password, email_confirm: true, user_metadata: { username } },
  });
  if (created.status === 422 || created.status === 409) {
    throw new HttpError(409, 'Аккаунт с такой почтой уже существует');
  }
  if (created.status >= 300 || !created.data || !created.data.id) {
    console.error('admin create user failed:', created.status, created.data && created.data.msg);
    throw new HttpError(502, 'Не удалось создать аккаунт');
  }

  const session = await signIn(email, password);
  if (!session) throw new HttpError(502, 'Аккаунт создан, но войти не удалось. Попробуйте войти вручную.');
  setSession(res, session);
  const verification = mailConfigured() ? await sendVerification(req, created.data.id, email, username) : 'off';
  return send(res, 200, {
    ok: true,
    verification,
    user: { id: created.data.id, username, email, role: 'user', email_verified: false, mfa: false, aal: 'aal1' },
  });
}

async function sendVerification(req, userId, email, username) {
  const token = crypto.randomBytes(32).toString('hex');
  await sb('email_verifications', {
    method: 'POST',
    body: { user_id: userId, token_hash: sha256(token), expires_at: new Date(Date.now() + VERIFY_TTL_MS).toISOString() },
  });
  const lang = requestLang(req);
  const link = `${siteUrl(req)}${lang === 'en' ? '/en' : ''}/verify?token=${token}`;
  const mail = MAIL[lang];
  try {
    await sendMail({
      to: email,
      subject: mail.subject,
      text: mail.text(username, link),
      html: mail.html(username, link),
    });
    return 'sent';
  } catch (e) {
    console.error('verification mail failed:', e && e.message);
    return 'failed';
  }
}

async function resendVerification(req, res) {
  const user = await requireUser(req, res);
  if (user.emailVerified) throw new HttpError(400, 'Почта уже подтверждена');
  if (!mailConfigured()) throw new HttpError(503, 'Отправка писем на сервере не настроена');
  const key = `vsend:${user.id}`;
  if (overLimit(key, VERIFY_SEND_MAX, VERIFY_SEND_WINDOW_MS)) {
    throw new HttpError(429, 'Письмо уже отправлялось несколько раз. Попробуйте через час.');
  }
  recordHit(key);
  const result = await sendVerification(req, user.id, user.email, user.username);
  if (result !== 'sent') throw new HttpError(502, 'Не удалось отправить письмо. Попробуйте позже.');
  return send(res, 200, { ok: true });
}

async function verifyEmail(req, res, body) {
  const ip = clientIp(req);
  if (overLimit(`vmail:${ip}`, 20, 60 * 60 * 1000)) throw new HttpError(429, 'Слишком много попыток');
  recordHit(`vmail:${ip}`);
  const token = String(body.token || '');
  if (!/^[0-9a-f]{64}$/.test(token)) throw new HttpError(400, 'Ссылка подтверждения повреждена');
  const hash = sha256(token);
  const now = encodeURIComponent(new Date().toISOString());
  const rows = await sb(`email_verifications?token_hash=eq.${hash}&used_at=is.null&expires_at=gt.${now}&select=id,user_id&limit=1`);
  if (!rows[0]) throw new HttpError(400, 'Ссылка недействительна или устарела. Запросите новое письмо в аккаунте.');
  await sb(`email_verifications?id=eq.${rows[0].id}`, { method: 'PATCH', body: { used_at: new Date().toISOString() } });
  await sb(`profiles?id=eq.${rows[0].user_id}`, { method: 'PATCH', body: { email_verified: true } });
  return send(res, 200, { ok: true });
}

async function login(req, res, body) {
  const ip = clientIp(req);
  const key = `login:${ip}`;
  if (overLimit(key, LOGIN_MAX_FAILS, LOGIN_WINDOW_MS)) {
    throw new HttpError(429, 'Слишком много неудачных попыток входа. Подождите 15 минут.');
  }
  const identifier = String(body.identifier || '').trim();
  const password = String(body.password || '');
  if (!identifier || !password) throw new HttpError(400, 'Введите логин и пароль');
  await verifyCaptcha(body.captcha, ip);

  let email = identifier.toLowerCase();
  if (!identifier.includes('@')) {
    email = (await rpc('email_for_username', { p_username: identifier })) || '';
  }
  const session = email ? await signIn(email, password) : null;
  if (!session) {
    recordHit(key);
    await sleep(400);
    throw new HttpError(401, 'Неверный логин или пароль');
  }
  const profile = await sb(`profiles?id=eq.${session.user.id}&select=username,role,email_verified`);
  if (!profile[0]) throw new HttpError(403, 'Профиль аккаунта не найден');
  const full = await auth('user', { method: 'GET', token: session.access_token });
  if (verifiedTotp(full.status === 200 && full.data ? full.data : session.user)) {
    const pending = Buffer.from(JSON.stringify({ a: session.access_token, r: session.refresh_token })).toString('base64url');
    res.setHeader('Set-Cookie', [cookie(MFA_COOKIE, pending, MFA_COOKIE_TTL_S, '/api')]);
    return send(res, 200, { ok: true, mfa: true });
  }
  setSession(res, session);
  return send(res, 200, {
    ok: true,
    user: {
      id: session.user.id,
      username: profile[0].username,
      email: session.user.email,
      role: profile[0].role,
      email_verified: profile[0].email_verified === true,
      mfa: false,
      aal: 'aal1',
    },
  });
}

function readPending(req) {
  const raw = cookieValue(req, MFA_COOKIE);
  if (!raw) return null;
  try {
    const data = JSON.parse(Buffer.from(raw, 'base64url').toString('utf8'));
    return data && data.a && data.r ? { access: data.a, refresh: data.r } : null;
  } catch {
    return null;
  }
}

function normalizeBackup(code) {
  const value = String(code || '').toUpperCase().replace(/[\s-]/g, '');
  return new RegExp(`^[${BACKUP_ALPHABET}]{${BACKUP_LENGTH}}$`).test(value) ? value : null;
}

function normalizeTotp(code) {
  const value = String(code || '').replace(/\s/g, '');
  return /^\d{6}$/.test(value) ? value : null;
}

/** Проверка TOTP-кода через GoTrue; возвращает aal2-сессию или null при неверном коде. */
async function totpVerify(factorId, token, code) {
  const challenge = await auth(`factors/${factorId}/challenge`, { token, body: {} });
  if (challenge.status !== 200 || !challenge.data || !challenge.data.id) {
    console.error('mfa challenge failed:', challenge.status);
    throw new HttpError(502, 'Не удалось проверить код. Попробуйте ещё раз.');
  }
  const verified = await auth(`factors/${factorId}/verify`, { token, body: { challenge_id: challenge.data.id, code } });
  if (verified.status === 200 && verified.data && verified.data.access_token) return verified.data;
  return null;
}

async function storeBackupCodes(userId) {
  const codes = [];
  for (let i = 0; i < BACKUP_COUNT; i += 1) {
    let code = '';
    for (let j = 0; j < BACKUP_LENGTH; j += 1) code += BACKUP_ALPHABET[crypto.randomInt(BACKUP_ALPHABET.length)];
    codes.push(code);
  }
  await sb(`mfa_backup_codes?user_id=eq.${userId}`, { method: 'DELETE' });
  await sb('mfa_backup_codes', {
    method: 'POST',
    body: codes.map((code) => ({ user_id: userId, code_hash: sha256(code) })),
  });
  return codes.map((code) => `${code.slice(0, 5)}-${code.slice(5)}`);
}

async function takeBackupCode(userId, code) {
  const rows = await sb(`mfa_backup_codes?user_id=eq.${userId}&code_hash=eq.${sha256(code)}&used_at=is.null&select=id&limit=1`);
  if (!rows[0]) return false;
  await sb(`mfa_backup_codes?id=eq.${rows[0].id}`, { method: 'PATCH', body: { used_at: new Date().toISOString() } });
  return true;
}

async function dropTotp(user) {
  for (const factor of user.factors.filter((f) => f.factor_type === 'totp')) {
    const r = await auth(`admin/users/${user.id}/factors/${factor.id}`, { method: 'DELETE', admin: true });
    if (r.status >= 300) {
      console.error('factor delete failed:', r.status);
      throw new HttpError(502, 'Не удалось отключить двухфакторную аутентификацию');
    }
  }
  await sb(`mfa_backup_codes?user_id=eq.${user.id}`, { method: 'DELETE' });
}

async function mfaVerify(req, res, body) {
  const pending = readPending(req);
  const expired = new HttpError(401, 'Время ввода кода истекло. Войдите заново.', 'mfa_expired');
  if (!pending) throw expired;
  const me = await auth('user', { method: 'GET', token: pending.access });
  if (me.status !== 200 || !me.data || !me.data.id) throw expired;
  const user = me.data;
  const factor = verifiedTotp(user);
  if (!factor) throw expired;

  const key = `mfa:${user.id}`;
  const ipKey = `mfa-ip:${clientIp(req)}`;
  if (overLimit(key, MFA_MAX_FAILS, MFA_WINDOW_MS) || overLimit(ipKey, 20, MFA_WINDOW_MS)) {
    throw new HttpError(429, 'Слишком много неверных кодов. Подождите 10 минут.');
  }
  const profile = (await sb(`profiles?id=eq.${user.id}&select=username,role,email_verified`))[0];
  if (!profile) throw new HttpError(403, 'Профиль аккаунта не найден');

  const clearPending = cookie(MFA_COOKIE, '', 0, '/api');
  const totp = normalizeTotp(body.code);
  const backup = totp ? null : normalizeBackup(body.code);
  if (!totp && !backup) throw new HttpError(400, 'Введите 6 цифр из приложения или резервный код');

  let session = null;
  let reset = false;
  if (totp) {
    session = await totpVerify(factor.id, pending.access, totp);
  } else if (await takeBackupCode(user.id, backup)) {
    await dropTotp({ id: user.id, factors: user.factors });
    session = { access_token: pending.access, refresh_token: pending.refresh };
    reset = true;
  }
  if (!session) {
    recordHit(key);
    recordHit(ipKey);
    await sleep(400);
    throw new HttpError(401, 'Неверный код');
  }
  setSession(res, session, [clearPending]);
  return send(res, 200, {
    ok: true,
    mfa_reset: reset,
    user: {
      id: user.id,
      username: profile.username,
      email: user.email,
      role: profile.role,
      email_verified: profile.email_verified === true,
      mfa: !reset,
      aal: reset ? 'aal1' : 'aal2',
    },
  });
}

async function mfaEnrollStart(req, res) {
  const user = await requireUser(req, res);
  if (verifiedTotp(user)) throw new HttpError(400, 'Двухфакторная аутентификация уже включена');
  for (const factor of user.factors.filter((f) => f.factor_type === 'totp')) {
    await auth(`admin/users/${user.id}/factors/${factor.id}`, { method: 'DELETE', admin: true });
  }
  const r = await auth('factors', {
    token: user.token,
    body: { factor_type: 'totp', friendly_name: `DeiX ${Date.now().toString(36)}`, issuer: 'DeiX OS' },
  });
  if (r.status !== 200 || !r.data || !r.data.totp) {
    console.error('mfa enroll failed:', r.status, r.data && r.data.msg);
    throw new HttpError(502, 'Не удалось начать подключение 2FA');
  }
  const secret = String(r.data.totp.secret || '');
  const uri = String(r.data.totp.uri || '') ||
    `otpauth://totp/${encodeURIComponent(`DeiX OS:${user.email}`)}?secret=${secret}&issuer=${encodeURIComponent('DeiX OS')}`;
  return send(res, 200, { ok: true, factor_id: r.data.id, secret, uri });
}

async function mfaEnrollFinish(req, res, body) {
  const user = await requireUser(req, res);
  const factorId = String(body.factor_id || '');
  const factor = user.factors.find((f) => f.id === factorId && f.factor_type === 'totp' && f.status !== 'verified');
  if (!factor) throw new HttpError(400, 'Подключение не начато или уже завершено. Начните заново.');
  const code = normalizeTotp(body.code);
  if (!code) throw new HttpError(400, 'Введите 6 цифр из приложения');
  const key = `mfa:${user.id}`;
  if (overLimit(key, MFA_MAX_FAILS, MFA_WINDOW_MS)) throw new HttpError(429, 'Слишком много неверных кодов. Подождите 10 минут.');
  const session = await totpVerify(factor.id, user.token, code);
  if (!session) {
    recordHit(key);
    await sleep(400);
    throw new HttpError(400, 'Неверный код. Проверьте время на телефоне и попробуйте ещё раз.');
  }
  setSession(res, session);
  const backup = await storeBackupCodes(user.id);
  return send(res, 200, { ok: true, backup_codes: backup });
}

async function requireTotpCode(user, body) {
  const factor = verifiedTotp(user);
  if (!factor) throw new HttpError(400, 'Двухфакторная аутентификация не включена');
  const key = `mfa:${user.id}`;
  if (overLimit(key, MFA_MAX_FAILS, MFA_WINDOW_MS)) throw new HttpError(429, 'Слишком много неверных кодов. Подождите 10 минут.');
  const code = normalizeTotp(body.code);
  if (!code) throw new HttpError(400, 'Введите 6 цифр из приложения');
  const session = await totpVerify(factor.id, user.token, code);
  if (!session) {
    recordHit(key);
    await sleep(400);
    throw new HttpError(401, 'Неверный код');
  }
  return session;
}

async function mfaDisable(req, res, body) {
  const user = await requireUser(req, res);
  await requireTotpCode(user, body);
  await dropTotp(user);
  return send(res, 200, { ok: true });
}

async function mfaBackupRegenerate(req, res, body) {
  const user = await requireUser(req, res);
  await requireTotpCode(user, body);
  return send(res, 200, { ok: true, backup_codes: await storeBackupCodes(user.id) });
}

async function mfaStatus(req, res) {
  const user = await requireUser(req, res);
  const rows = verifiedTotp(user)
    ? await sb(`mfa_backup_codes?user_id=eq.${user.id}&used_at=is.null&select=id`)
    : [];
  return send(res, 200, { ok: true, enabled: Boolean(verifiedTotp(user)), backup_left: rows.length });
}

async function logout(req, res) {
  const user = await currentUser(req, res);
  if (user) await auth('logout', { token: user.token });
  clearSession(res);
  return send(res, 200, { ok: true });
}

async function changePassword(req, res, body) {
  const user = await requireUser(req, res);
  const current = String(body.current || '');
  const next = String(body.next || '');
  validatePassword(next);
  if (next === current) throw new HttpError(400, 'Новый пароль совпадает с текущим');
  if (!(await rpc('verify_user_password', { p_user_id: user.id, p_password: current }))) {
    await sleep(400);
    throw new HttpError(401, 'Текущий пароль указан неверно');
  }
  const updated = await auth(`admin/users/${user.id}`, { method: 'PUT', admin: true, body: { password: next } });
  if (updated.status >= 300) {
    console.error('password update failed:', updated.status);
    throw new HttpError(502, 'Не удалось сменить пароль');
  }
  const session = await signIn(user.email, next);
  if (session) setSession(res, session);
  else clearSession(res);
  // Украденный refresh-токен не должен переживать смену пароля: гасим все
  // сессии кроме текущей. Ошибку ревкации не прячем — пишем в лог функции.
  if (session) {
    const rev = await auth('logout?scope=others', { token: session.access_token });
    if (rev.status >= 300) console.error('revoke other sessions failed:', rev.status);
  }
  return send(res, 200, { ok: true });
}

module.exports = async function handler(req, res) {
  try {
    guardPost(req, res);
    const body = readBody(req);
    if (body.action !== 'me') meCache.clear();
    switch (body.action) {
      case 'config':
        return send(res, 200, {
          ok: true,
          captcha: captchaEnabled(),
          captcha_site_key: captchaEnabled() ? TURNSTILE_SITE_KEY : null,
          email_verification: mailConfigured(),
        });
      case 'me': {
        const token = cookieValue(req, 'deix_at');
        const cached = token ? meCache.get(token) : null;
        if (cached && cached.until > Date.now()) return send(res, 200, { ok: true, user: cached.user });
        const user = await currentUser(req, res);
        const shown = user ? publicUser(user) : null;
        if (shown && token && user.token === token) {
          if (meCache.size >= ME_CACHE_MAX) meCache.clear();
          meCache.set(token, { user: shown, until: Date.now() + ME_CACHE_TTL_MS });
        }
        return send(res, 200, { ok: true, user: shown });
      }
      case 'register':
        return await register(req, res, body);
      case 'login':
        return await login(req, res, body);
      case 'mfa_verify':
        return await mfaVerify(req, res, body);
      case 'mfa_status':
        return await mfaStatus(req, res);
      case 'mfa_enroll_start':
        return await mfaEnrollStart(req, res);
      case 'mfa_enroll_finish':
        return await mfaEnrollFinish(req, res, body);
      case 'mfa_disable':
        return await mfaDisable(req, res, body);
      case 'mfa_backup_regenerate':
        return await mfaBackupRegenerate(req, res, body);
      case 'resend_verification':
        return await resendVerification(req, res);
      case 'verify_email':
        return await verifyEmail(req, res, body);
      case 'logout':
        return await logout(req, res);
      case 'change_password':
        return await changePassword(req, res, body);
      case 'sync_settings': {
        const user = await currentUser(req, res);
        if (!user) throw new HttpError(401, 'Не авторизован');
        const result = await rpc('get_user_settings', { p_user_id: user.id });
        if (result.result === 'none') {
          return send(res, 200, { ok: true, status: 'none' });
        }
        return send(res, 200, {
          ok: true,
          status: 'ok',
          settings: result.settings,
          updated_at: result.updated_at,
        });
      }
      case 'sync_clipboard': {
        const user = await currentUser(req, res);
        if (!user) throw new HttpError(401, 'Не авторизован');
        if (body.op === 'set') {
          if (!body.content || typeof body.content !== 'string') {
            throw new HttpError(400, 'content required');
          }
          if (body.content.length > 4000) {
            throw new HttpError(400, 'content too long (max 4000 chars)');
          }
          const result = await rpc('save_clipboard', { p_user_id: user.id, p_content: body.content });
          return send(res, 200, { ok: true, status: 'ok', created_at: result.created_at });
        }
        const result = await rpc('get_clipboard', { p_user_id: user.id });
        if (result.result === 'none') {
          return send(res, 200, { ok: true, status: 'none' });
        }
        return send(res, 200, {
          ok: true,
          status: 'ok',
          content: result.content,
          created_at: result.created_at,
        });
      }
      default:
        throw new HttpError(400, 'Неизвестное действие');
    }
  } catch (err) {
    return fail(res, err);
  }
};
