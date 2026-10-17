'use strict';

const {
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
  guardPost,
  overLimit,
  readBody,
  recordHit,
  requireUser,
  rpc,
  sb,
  send,
  setSession,
  verifyCaptcha,
} = require('./_lib');

const REGISTER_MAX = 5;
const REGISTER_WINDOW_MS = 60 * 60 * 1000;
const LOGIN_MAX_FAILS = 8;
const LOGIN_WINDOW_MS = 15 * 60 * 1000;

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function publicUser(user) {
  return { username: user.username, email: user.email, role: user.role };
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
  return send(res, 200, { ok: true, user: { username, email, role: 'user' } });
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
  const profile = await sb(`profiles?id=eq.${session.user.id}&select=username,role`);
  if (!profile[0]) throw new HttpError(403, 'Профиль аккаунта не найден');
  setSession(res, session);
  return send(res, 200, {
    ok: true,
    user: { username: profile[0].username, email: session.user.email, role: profile[0].role },
  });
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
  return send(res, 200, { ok: true });
}

module.exports = async function handler(req, res) {
  try {
    guardPost(req, res);
    const body = readBody(req);
    switch (body.action) {
      case 'config':
        return send(res, 200, {
          ok: true,
          captcha: captchaEnabled(),
          captcha_site_key: captchaEnabled() ? TURNSTILE_SITE_KEY : null,
        });
      case 'me': {
        const user = await currentUser(req, res);
        return send(res, 200, { ok: true, user: user ? publicUser(user) : null });
      }
      case 'register':
        return await register(req, res, body);
      case 'login':
        return await login(req, res, body);
      case 'logout':
        return await logout(req, res);
      case 'change_password':
        return await changePassword(req, res, body);
      default:
        throw new HttpError(400, 'Неизвестное действие');
    }
  } catch (err) {
    return fail(res, err);
  }
};
