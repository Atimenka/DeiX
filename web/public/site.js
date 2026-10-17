'use strict';

function el(tag, attrs, children) {
  const node = document.createElement(tag);
  if (attrs) {
    for (const [key, value] of Object.entries(attrs)) {
      if (key === 'class') node.className = value;
      else if (key === 'text') node.textContent = value;
      else if (key.startsWith('on')) node.addEventListener(key.slice(2), value);
      else node.setAttribute(key, value);
    }
  }
  for (const child of children || []) node.append(child);
  return node;
}

async function postJson(url, body, extraHeaders) {
  let resp;
  try {
    resp = await fetch(url, {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'Content-Type': 'application/json', 'x-deix-csrf': '1', ...(extraHeaders || {}) },
      body: JSON.stringify(body),
    });
  } catch {
    throw new Error('Нет связи с сервером');
  }
  let data = null;
  try {
    data = await resp.json();
  } catch {
    data = null;
  }
  if (!resp.ok || !data || data.ok === false) {
    const err = new Error((data && data.error) || `Ошибка сервера (${resp.status})`);
    err.status = resp.status;
    throw err;
  }
  return data;
}

const authApi = (body) => postJson('/api/auth', body);

function toast(text, kind) {
  let box = document.getElementById('toasts');
  if (!box) {
    box = el('div', { id: 'toasts' });
    document.body.append(box);
  }
  const node = el('div', { class: `toast ${kind || ''}`, text });
  box.append(node);
  setTimeout(() => node.remove(), 4500);
}

function showResult(node, kind, text) {
  node.hidden = false;
  node.className = `result ${kind}`;
  node.textContent = text;
}

/** Безопасный адрес возврата после входа: только путь на этом же сайте. */
function nextPath() {
  const next = new URLSearchParams(location.search).get('next') || '';
  return /^\/[A-Za-z0-9_\-/]*$/.test(next) && !next.startsWith('//') ? next : '/account';
}

let mePromise = null;
function fetchMe() {
  if (!mePromise) {
    mePromise = authApi({ action: 'me' })
      .then((data) => data.user)
      .catch(() => null);
  }
  return mePromise;
}

async function renderNav() {
  const slot = document.getElementById('nav-auth');
  if (!slot) return;
  const user = await fetchMe();
  slot.replaceChildren();
  if (user) {
    if (user.role === 'owner') slot.append(el('a', { href: '/admin', text: 'Панель' }));
    slot.append(el('a', { class: 'btn small', href: '/account', text: user.username }));
  } else {
    slot.append(el('a', { href: '/login', text: 'Войти' }), el('a', { class: 'btn primary small', href: '/register', text: 'Регистрация' }));
  }
}

/**
 * Капча Cloudflare Turnstile. Если владелец ещё не включил её на сервере,
 * виджета нет и форма прямо говорит об этом.
 */
async function mountCaptcha(container) {
  let config = null;
  try {
    config = await authApi({ action: 'config' });
  } catch {
    config = null;
  }
  if (!config || !config.captcha) {
    container.append(el('p', { class: 'hint', text: 'Защита от ботов на сервере пока не включена.' }));
    return { token: () => '', reset: () => {} };
  }
  await new Promise((resolve, reject) => {
    if (window.turnstile) return resolve();
    const script = document.createElement('script');
    script.src = 'https://challenges.cloudflare.com/turnstile/v0/api.js?render=explicit';
    script.async = true;
    script.onload = resolve;
    script.onerror = () => reject(new Error('Не удалось загрузить капчу'));
    document.head.append(script);
  }).catch((err) => {
    container.append(el('p', { class: 'hint', text: err.message }));
  });
  let value = '';
  let widget = null;
  if (window.turnstile) {
    widget = window.turnstile.render(container, {
      sitekey: config.captcha_site_key,
      theme: 'dark',
      callback: (token) => {
        value = token;
      },
      'expired-callback': () => {
        value = '';
      },
    });
  }
  return {
    token: () => value,
    reset: () => {
      value = '';
      if (widget !== null && window.turnstile) window.turnstile.reset(widget);
    },
  };
}

for (const node of document.querySelectorAll('[data-year]')) {
  node.textContent = String(new Date().getFullYear());
}
renderNav();
