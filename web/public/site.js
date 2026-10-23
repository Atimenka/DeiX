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
      headers: { 'Content-Type': 'application/json', 'x-deix-csrf': '1', 'x-deix-lang': LANG, ...(extraHeaders || {}) },
      body: JSON.stringify(body),
    });
  } catch {
    throw new Error(tr('Нет связи с сервером'));
  }
  let data = null;
  try {
    data = await resp.json();
  } catch {
    data = null;
  }
  if (!resp.ok || !data || data.ok === false) {
    const err = new Error((data && data.error) || tr('Ошибка сервера ({status})', { status: resp.status }));
    err.status = resp.status;
    err.code = data && data.code;
    throw err;
  }
  return data;
}

async function authApi(body) {
  if (body.action === 'logout' && typeof Vault !== 'undefined' && Vault) await Vault.lock().catch(() => {});
  return postJson('/api/auth', body);
}

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
  return /^\/[A-Za-z0-9_\-/]*$/.test(next) && !next.startsWith('//') ? lp(next) : lp('/account');
}

let mePromise = null;
function fetchMe() {
  if (!mePromise) {
    mePromise = authApi({ action: 'me' })
      .then((data) => {
        if (!data.user && typeof Vault !== 'undefined' && Vault) Vault.lock().catch(() => {});
        return data.user;
      })
      .catch(() => null);
  }
  return mePromise;
}

function fmtSize(bytes) {
  if (bytes >= 1048576) return tr('{n} МиБ', { n: (bytes / 1048576).toFixed(1) });
  if (bytes >= 1024) return tr('{n} КиБ', { n: Math.round(bytes / 1024) });
  return tr('{n} Б', { n: bytes });
}

const SVG_NS = 'http://www.w3.org/2000/svg';

function icon(name, extra) {
  const svg = document.createElementNS(SVG_NS, 'svg');
  svg.setAttribute('class', `icon${extra ? ` ${extra}` : ''}`);
  svg.setAttribute('aria-hidden', 'true');
  const use = document.createElementNS(SVG_NS, 'use');
  use.setAttribute('href', `/icons.svg#${name}`);
  svg.append(use);
  return svg;
}

const MODES = [
  ['beauty', '✦', tr('Красота')],
  ['femboy', '♡', tr('Фембой')],
  ['plain', '○', tr('Обычный')],
];

function currentMode() {
  return document.documentElement.getAttribute('data-mode') || 'beauty';
}

function setMode(mode) {
  document.documentElement.setAttribute('data-mode', mode);
  try {
    localStorage.setItem('deix-mode', mode);
  } catch {
    return;
  }
  location.reload();
}

function setTheme(theme) {
  document.documentElement.setAttribute('data-theme', theme);
  try {
    localStorage.setItem('deix-theme', theme);
  } catch {
    return;
  }
}

const NAV_LINKS = [
  ['home', '/', tr('Главная')],
  ['features', '/#features', tr('Возможности')],
  ['dev', '/#dev', tr('Dev-режим')],
  ['download', '/download', tr('Скачать')],
  ['request', '/request', tr('Заявки')],
];

function buildLayout() {
  const page = document.body.dataset.page || '';
  document.body.prepend(
    el('div', { class: 'bg', 'aria-hidden': 'true' }, [el('div', { class: 'blob a' }), el('div', { class: 'blob b' }), el('div', { class: 'blob c' })]),
  );

  const nav = el('header', { class: 'nav' });
  const themeBtn = el('button', { class: 'icon-btn theme-toggle', type: 'button', 'aria-label': tr('Сменить тему'), title: tr('Светлая / тёмная тема') }, [
    icon('moon', 'moon'),
    icon('sun', 'sun'),
  ]);
  themeBtn.addEventListener('click', () => {
    setTheme(document.documentElement.getAttribute('data-theme') === 'light' ? 'dark' : 'light');
  });
  const modeNow = currentMode();
  const modeBtn = el(
    'button',
    { class: 'icon-btn mode-toggle', type: 'button', 'aria-label': tr('Режим сайта'), 'aria-haspopup': 'true', 'aria-expanded': 'false', title: tr('Режим сайта') },
    [document.createTextNode(MODES.find(([key]) => key === modeNow)[1])],
  );
  const modeMenu = el(
    'div',
    { class: 'mode-menu', role: 'menu' },
    MODES.map(([key, glyph, label]) => {
      const item = el('button', { type: 'button', role: 'menuitem', class: key === modeNow ? 'on' : '' }, [el('span', { class: 'g', text: glyph }), document.createTextNode(label)]);
      item.addEventListener('click', () => setMode(key));
      return item;
    }),
  );
  modeMenu.append(el('small', { text: tr('Страница перезагрузится') }));
  const closeModes = () => {
    modeMenu.classList.remove('open');
    modeBtn.setAttribute('aria-expanded', 'false');
  };
  modeBtn.addEventListener('click', (event) => {
    event.stopPropagation();
    const open = modeMenu.classList.toggle('open');
    modeBtn.setAttribute('aria-expanded', String(open));
  });
  document.addEventListener('click', closeModes);
  document.addEventListener('keydown', (event) => {
    if (event.key === 'Escape') closeModes();
  });
  const modeWrap = el('div', { class: 'mode-wrap' }, [modeBtn, modeMenu]);
  const langBtn = el('a', {
    class: 'icon-btn lang-toggle',
    href: otherLangPath(),
    hreflang: LANG === 'en' ? 'ru' : 'en',
    lang: LANG === 'en' ? 'ru' : 'en',
    'aria-label': LANG === 'en' ? 'Русская версия' : 'English version',
    title: LANG === 'en' ? 'Русская версия' : 'English version',
    text: LANG === 'en' ? 'RU' : 'EN',
  });
  langBtn.addEventListener('click', () => {
    langBtn.href = otherLangPath();
    try {
      localStorage.setItem('deix-lang', LANG === 'en' ? 'ru' : 'en');
    } catch {
      return;
    }
  });
  const burger = el('button', { class: 'icon-btn burger', type: 'button', 'aria-label': tr('Меню'), 'aria-expanded': 'false' }, [icon('menu')]);
  burger.addEventListener('click', () => {
    const open = nav.classList.toggle('open');
    burger.setAttribute('aria-expanded', String(open));
    burger.replaceChildren(icon(open ? 'close' : 'menu'));
  });
  const links = el(
    'nav',
    { class: 'links', 'aria-label': tr('Основная навигация') },
    NAV_LINKS.map(([key, href, label]) => el('a', { class: `link${key === page ? ' active' : ''}`, href: lp(href), text: label })),
  );
  links.append(el('a', { class: 'link', href: 'https://github.com/Atimenka/DeiX', rel: 'noopener', text: 'GitHub' }), el('span', { id: 'nav-auth', class: 'nav-auth' }));
  nav.append(
    el('div', { class: 'wrap' }, [
      el('div', { class: 'nav-bar' }, [
        el('a', { class: 'brand', href: lp('/') }, [el('img', { src: '/favicon.svg', alt: '' }), document.createTextNode('DeiX OS')]),
        links,
        el('div', { class: 'nav-tools' }, [modeWrap, langBtn, themeBtn, burger]),
      ]),
    ]),
  );
  document.body.insertBefore(nav, document.body.querySelector('main'));

  const foot = el('footer', { class: 'foot' }, [
    el('div', { class: 'wrap' }, [
      el('span', { text: `© ${new Date().getFullYear()} DeiX OS` }),
      el('nav', null, [
        el('a', { href: lp('/download'), text: tr('Скачать') }),
        el('a', { href: lp('/request'), text: tr('Заявки') }),
        el('a', { href: lp('/register'), text: tr('Регистрация') }),
        el('a', { href: 'https://github.com/Atimenka/DeiX', rel: 'noopener', text: 'GitHub' }),
      ]),
    ]),
  ]);
  document.body.append(foot);
}

function showBanner(parts) {
  const nav = document.querySelector('header.nav');
  if (!nav || document.getElementById('site-banner')) return;
  nav.after(el('div', { id: 'site-banner', class: 'banner', role: 'status' }, [icon('alert'), ...parts]));
}

async function resendVerification(button) {
  button.disabled = true;
  try {
    await authApi({ action: 'resend_verification' });
    toast(tr('Письмо отправлено. Проверьте почту.'), 'ok');
  } catch (err) {
    toast(err.message, 'err');
    button.disabled = false;
  }
}

async function accountBanners(user) {
  if (!user.email_verified) {
    let enabled = false;
    try {
      enabled = (await authApi({ action: 'config' })).email_verification === true;
    } catch {
      enabled = false;
    }
    if (enabled) {
      const button = el('button', { class: 'btn small', type: 'button', text: tr('Отправить письмо ещё раз') });
      button.addEventListener('click', () => resendVerification(button));
      showBanner([el('span', { class: 'grow', text: tr('Почта {email} не подтверждена: без этого недоступны ключи и заявки.', { email: user.email }) }), button]);
      return;
    }
  }
  if (user.role === 'owner' && !user.mfa) {
    showBanner([
      el('span', { class: 'grow', text: tr('Для панели Owner обязательна двухфакторная аутентификация.') }),
      el('a', { class: 'btn small primary', href: lp('/account#security'), text: tr('Включить 2FA') }),
    ]);
  }
}

async function renderNav() {
  const slot = document.getElementById('nav-auth');
  if (!slot) return;
  const user = await fetchMe();
  slot.replaceChildren();
  if (user) {
    if (user.role === 'owner') slot.append(el('a', { class: 'link', href: lp('/admin'), text: tr('Панель') }));
    slot.append(el('a', { class: 'btn small', href: lp('/account') }, [icon('user'), document.createTextNode(user.username)]));
    accountBanners(user);
  } else {
    slot.append(el('a', { class: 'link', href: lp('/login'), text: tr('Войти') }), el('a', { class: 'btn primary small', href: lp('/register'), text: tr('Регистрация') }));
  }
}

function enhancePasswords() {
  for (const input of document.querySelectorAll('input[type="password"]')) {
    const wrap = el('span', { class: 'pw' });
    const toggle = el('button', { type: 'button', 'aria-label': tr('Показать пароль'), tabindex: '-1' }, [icon('eye')]);
    input.replaceWith(wrap);
    wrap.append(input, toggle);
    toggle.addEventListener('click', () => {
      const show = input.type === 'password';
      input.type = show ? 'text' : 'password';
      toggle.replaceChildren(icon(show ? 'eyeoff' : 'eye'));
      toggle.setAttribute('aria-label', show ? tr('Скрыть пароль') : tr('Показать пароль'));
    });
  }
}

function enhanceMotion() {
  const items = document.querySelectorAll('.reveal');
  if ('IntersectionObserver' in window) {
    const seen = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          if (entry.isIntersecting) {
            entry.target.classList.add('in');
            seen.unobserve(entry.target);
          }
        }
      },
      { threshold: 0.12, rootMargin: '0px 0px -40px 0px' },
    );
    items.forEach((item) => seen.observe(item));
  } else {
    items.forEach((item) => item.classList.add('in'));
  }
  document.addEventListener('pointermove', (event) => {
    const card = event.target instanceof Element ? event.target.closest('.card') : null;
    if (!card) return;
    const box = card.getBoundingClientRect();
    card.style.setProperty('--mx', `${event.clientX - box.left}px`);
    card.style.setProperty('--my', `${event.clientY - box.top}px`);
  });
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
    container.append(el('p', { class: 'hint', text: tr('Защита от ботов на сервере пока не включена.') }));
    return { token: () => '', reset: () => {} };
  }
  await new Promise((resolve, reject) => {
    if (window.turnstile) return resolve();
    const script = document.createElement('script');
    script.src = 'https://challenges.cloudflare.com/turnstile/v0/api.js?render=explicit';
    script.async = true;
    script.onload = resolve;
    script.onerror = () => reject(new Error(tr('Не удалось загрузить капчу')));
    document.head.append(script);
  }).catch((err) => {
    container.append(el('p', { class: 'hint', text: err.message }));
  });
  let value = '';
  let widget = null;
  const note = el('p', { class: 'hint captcha-error', hidden: true });
  container.after(note);
  const say = (text) => {
    note.textContent = text;
    note.hidden = !text;
  };
  const host = location.hostname;
  const hostHint = tr('Если виджет пишет «Unable to connect to website», добавьте {host} в Cloudflare → Turnstile → ваш виджет → Hostname Management.', { host });
  let waiting = null;
  if (window.turnstile) {
    widget = window.turnstile.render(container, {
      sitekey: config.captcha_site_key,
      theme: document.documentElement.getAttribute('data-theme') === 'light' ? 'light' : 'dark',
      callback: (token) => {
        value = token;
        clearTimeout(waiting);
        say('');
      },
      'expired-callback': () => {
        value = '';
      },
      'timeout-callback': () => {
        value = '';
      },
      'unsupported-callback': () => {
        say(tr('Браузер не поддерживается капчей Cloudflare. Обновите браузер.'));
      },
      'error-callback': (code) => {
        value = '';
        clearTimeout(waiting);
        say(
          /^1[01]\d{4}$/.test(String(code))
            ? tr('Капча отклонила адрес {host} (код {code}). {hint}', { host, code, hint: hostHint })
            : tr('Капча не загрузилась (код {code}). Проверьте соединение, отключите блокировщики и VPN, затем обновите страницу.', { code: code || tr('неизвестен') }),
        );
        return true;
      },
    });
    waiting = setTimeout(() => {
      if (!value) say(tr('Капча не завершила проверку за 10 секунд. {hint}', { hint: hostHint }));
    }, 10000);
  }
  return {
    token: () => value,
    reset: () => {
      value = '';
      say('');
      if (widget !== null && window.turnstile) window.turnstile.reset(widget);
    },
  };
}

buildLayout();
try {
  const flash = sessionStorage.getItem('deix-flash');
  if (flash) {
    sessionStorage.removeItem('deix-flash');
    toast(flash, 'ok');
  }
} catch {
  document.documentElement.classList.add('no-storage');
}
enhancePasswords();
enhanceMotion();
renderNav();
