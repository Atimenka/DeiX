'use strict';

(async () => {
  const box = document.getElementById('state');
  const actions = document.getElementById('state-actions');
  const finish = (ok, title, text, buttons) => {
    document.getElementById('spinner')?.remove();
    box.classList.toggle('bad', !ok);
    box.querySelector('.big')?.remove();
    box.prepend(el('div', { class: 'big' }, [icon(ok ? 'check' : 'alert')]));
    document.getElementById('state-title').textContent = title;
    document.getElementById('state-text').textContent = text;
    actions.replaceChildren(...buttons);
    actions.hidden = false;
  };
  const token = new URLSearchParams(location.search).get('token') || '';
  try {
    await authApi({ action: 'verify_email', token });
    finish(true, 'Почта подтверждена', 'Теперь вам доступны ключи dev-режима и заявки.', [
      el('a', { class: 'btn primary', href: '/account', text: 'В аккаунт' }),
    ]);
  } catch (err) {
    finish(false, 'Не удалось подтвердить', err.message, [
      el('a', { class: 'btn primary', href: '/account', text: 'В аккаунт' }),
      el('a', { class: 'btn', href: '/', text: 'На главную' }),
    ]);
  }
})();
