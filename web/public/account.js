'use strict';

const TABS = ['overview', 'devices', 'key', 'security'];
let me = null;
let mailEnabled = false;

function showTab(name) {
  const tab = TABS.includes(name) ? name : 'overview';
  for (const pane of document.querySelectorAll('[data-pane]')) pane.hidden = pane.dataset.pane !== tab;
  for (const button of document.querySelectorAll('#menu [data-tab]')) button.classList.toggle('active', button.dataset.tab === tab);
  if (location.hash.slice(1) !== tab) history.replaceState(null, '', `#${tab}`);
}

function renderProfile() {
  document.getElementById('avatar').textContent = me.username.slice(0, 1).toUpperCase();
  document.getElementById('acc-name').textContent = me.username;
  document.getElementById('acc-email').textContent = me.email;
  document.getElementById('ov-name').textContent = me.username;
  document.getElementById('ov-email').textContent = me.email;
  document.getElementById('ov-role').textContent = me.role === 'owner' ? 'Owner' : tr('Пользователь');
  document.getElementById('admin-link').hidden = me.role !== 'owner';
  const badges = document.getElementById('acc-badges');
  badges.replaceChildren(
    el('span', { class: `badge ${me.role === 'owner' ? 'owner' : ''}`, text: me.role === 'owner' ? 'Owner' : tr('Пользователь') }),
    el('span', { class: `badge ${me.mfa ? 'on' : 'warn'}`, text: me.mfa ? tr('2FA включена') : tr('2FA выключена') }),
  );
  renderMail();
  renderMfa();
}

function renderMail() {
  const badge = document.getElementById('mail-badge');
  const text = document.getElementById('mail-text');
  const resend = document.getElementById('mail-resend');
  badge.className = `badge ${me.email_verified ? 'on' : 'warn'}`;
  badge.textContent = me.email_verified ? tr('Подтверждена') : tr('Не подтверждена');
  resend.hidden = me.email_verified || !mailEnabled;
  if (me.email_verified) text.textContent = tr('Адрес {email} подтверждён.', { email: me.email });
  else if (mailEnabled) text.textContent = tr('Мы отправили письмо со ссылкой на {email}. Без подтверждения недоступны ключи и заявки.', { email: me.email });
  else text.textContent = tr('Отправка писем на сервере не настроена, поэтому подтверждение пока не требуется.');
}

(async () => {
  me = await fetchMe();
  if (!me) {
    location.replace(lp('/login?next=/account'));
    return;
  }
  try {
    mailEnabled = (await authApi({ action: 'config' })).email_verification === true;
  } catch {
    mailEnabled = false;
  }
  renderProfile();
  document.getElementById('account-view').hidden = false;
  showTab(location.hash.slice(1));
  loadDevices();
  loadKeyHistory();
})();

for (const button of document.querySelectorAll('#menu [data-tab]')) {
  button.addEventListener('click', () => showTab(button.dataset.tab));
}
window.addEventListener('hashchange', () => showTab(location.hash.slice(1)));

document.getElementById('mail-resend').addEventListener('click', (event) => resendVerification(event.currentTarget));

async function loadDevices() {
  const box = document.getElementById('devices');
  try {
    const data = await postJson('/api/devices', { action: 'list' });
    box.replaceChildren();
    document.getElementById('key-devices').replaceChildren(
      ...data.devices.map((device) => el('option', { value: device.device_id })),
    );
    if (!data.devices.length) {
      box.append(el('p', { class: 'hint', text: tr('Привязанных устройств пока нет.') }));
      return;
    }
    for (const device of data.devices) {
      box.append(
        el('div', { class: 'mine-item device-row' }, [
          el('div', null, [
            el('span', { class: 'mono', text: device.device_id }),
            el('div', { class: 'hint', text: tr('Привязано {date}', { date: new Date(device.linked_at).toLocaleString(LOCALE) }) }),
          ]),
          el('button', { class: 'btn small', type: 'button', text: tr('Отвязать'), onclick: () => unlinkDevice(device.device_id) }),
        ]),
      );
    }
  } catch (err) {
    box.replaceChildren(el('p', { class: 'hint', text: err.message }));
  }
}

async function unlinkDevice(id) {
  if (!confirm(tr('Отвязать устройство? Подать заявку с него можно будет только после новой привязки.'))) return;
  try {
    await postJson('/api/devices', { action: 'unlink', device_id: id });
    toast(tr('Устройство отвязано'), 'ok');
    loadDevices();
  } catch (err) {
    toast(err.message, 'err');
  }
}

document.getElementById('link-form').addEventListener('submit', async (event) => {
  event.preventDefault();
  const result = document.getElementById('link-result');
  try {
    const data = await postJson('/api/devices', { action: 'link', code: document.getElementById('link-code').value });
    event.target.reset();
    showResult(result, 'ok', tr('Устройство {id}… привязано. В DeiX можно выполнить «dev request».', { id: data.device_id.slice(0, 8) }));
    loadDevices();
  } catch (err) {
    showResult(result, 'err', err.message);
  }
});

document.getElementById('logout-btn').addEventListener('click', async () => {
  try {
    await authApi({ action: 'logout' });
  } finally {
    location.assign(lp('/'));
  }
});

document.getElementById('password-form').addEventListener('submit', async (event) => {
  event.preventDefault();
  const result = document.getElementById('pw-result');
  const next = document.getElementById('pw-next').value;
  if (next !== document.getElementById('pw-next2').value) {
    showResult(result, 'err', tr('Новые пароли не совпадают'));
    return;
  }
  try {
    await authApi({ action: 'change_password', current: document.getElementById('pw-current').value, next });
    event.target.reset();
    showResult(result, 'ok', tr('Пароль изменён'));
  } catch (err) {
    showResult(result, 'err', err.message);
  }
});

async function loadKeyHistory() {
  const box = document.getElementById('key-history');
  try {
    const data = await postJson('/api/keys', { action: 'list' });
    box.replaceChildren();
    if (!data.keys.length) return;
    box.append(el('p', { class: 'hint', text: tr('Недавно выданные ключи:') }));
    for (const item of data.keys) {
      box.append(
        el('div', { class: 'mine-item' }, [
          el('span', { class: 'mono', text: item.device_id }),
          el('div', { class: 'hint', text: new Date(item.created_at).toLocaleString(LOCALE) }),
        ]),
      );
    }
  } catch (err) {
    box.replaceChildren(el('p', { class: 'hint', text: err.message }));
  }
}

document.getElementById('key-form').addEventListener('submit', async (event) => {
  event.preventDefault();
  const result = document.getElementById('key-result');
  const out = document.getElementById('key-out');
  const copy = document.getElementById('key-copy');
  try {
    const data = await postJson('/api/keys', { action: 'issue', device_id: document.getElementById('key-device').value });
    out.textContent = data.key;
    out.hidden = false;
    copy.hidden = false;
    showResult(result, 'ok', tr('Ключ для устройства {id}… готов. Он работает только на нём и только один раз.', { id: data.device_id.slice(0, 8) }));
    loadKeyHistory();
  } catch (err) {
    out.hidden = true;
    copy.hidden = true;
    showResult(result, 'err', err.message);
  }
});

document.getElementById('key-copy').addEventListener('click', async () => {
  try {
    await navigator.clipboard.writeText(document.getElementById('key-out').textContent);
    toast(tr('Ключ скопирован'), 'ok');
  } catch {
    toast(tr('Не удалось скопировать: выделите ключ вручную'), 'err');
  }
});

const mfaResult = document.getElementById('mfa-result');
const codesBox = document.getElementById('mfa-codes');
let setupFactor = null;
let pendingAction = null;
let shownCodes = [];

function renderMfa() {
  const on = me.mfa;
  const badge = document.getElementById('mfa-badge');
  badge.className = `badge ${on ? 'on' : 'off'}`;
  badge.textContent = on ? tr('Включена') : tr('Выключена');
  document.getElementById('mfa-text').textContent = on
    ? tr('При входе нужен код из приложения. Резервные коды помогут, если телефон потерян.')
    : me.role === 'owner'
      ? tr('Для панели Owner двухфакторная аутентификация обязательна: без неё доступ к заявкам закрыт.')
      : tr('Дополнительная защита аккаунта: после пароля потребуется код из приложения-аутентификатора.');
  document.getElementById('mfa-start').hidden = on || !document.getElementById('mfa-setup').hidden;
  document.getElementById('mfa-manage').hidden = !on || !codesBox.hidden;
  if (!on) document.getElementById('mfa-confirm').hidden = true;
}

async function refreshMe() {
  const data = await authApi({ action: 'me' });
  me = data.user;
  renderProfile();
}

document.getElementById('mfa-start').addEventListener('click', async (event) => {
  const button = event.currentTarget;
  button.disabled = true;
  mfaResult.hidden = true;
  try {
    const data = await authApi({ action: 'mfa_enroll_start' });
    setupFactor = data.factor_id;
    const qrBox = document.getElementById('mfa-qr');
    try {
      qrBox.replaceChildren(qrSvg(data.uri));
    } catch (err) {
      qrBox.replaceChildren(el('p', { class: 'hint', text: tr('QR-код построить не удалось, введите ключ вручную.') }));
    }
    document.getElementById('mfa-secret').textContent = data.secret;
    document.getElementById('mfa-setup').hidden = false;
    button.hidden = true;
    document.getElementById('mfa-code').focus();
  } catch (err) {
    showResult(mfaResult, 'err', err.message);
  } finally {
    button.disabled = false;
  }
});

document.getElementById('mfa-cancel').addEventListener('click', () => {
  setupFactor = null;
  document.getElementById('mfa-setup').hidden = true;
  document.getElementById('mfa-code').value = '';
  renderMfa();
});

function showCodes(codes) {
  shownCodes = codes;
  document.getElementById('mfa-codes-list').replaceChildren(...codes.map((code) => el('span', { text: code })));
  codesBox.hidden = false;
  document.getElementById('mfa-manage').hidden = true;
}

document.getElementById('mfa-finish').addEventListener('submit', async (event) => {
  event.preventDefault();
  try {
    const data = await authApi({ action: 'mfa_enroll_finish', factor_id: setupFactor, code: document.getElementById('mfa-code').value });
    document.getElementById('mfa-setup').hidden = true;
    document.getElementById('mfa-code').value = '';
    showCodes(data.backup_codes);
    await refreshMe();
    toast(tr('Двухфакторная аутентификация включена'), 'ok');
  } catch (err) {
    showResult(mfaResult, 'err', err.message);
  }
});

document.getElementById('codes-copy').addEventListener('click', async () => {
  try {
    await navigator.clipboard.writeText(shownCodes.join('\n'));
    toast(tr('Коды скопированы'), 'ok');
  } catch {
    toast(tr('Не удалось скопировать: выделите коды вручную'), 'err');
  }
});

document.getElementById('codes-save').addEventListener('click', () => {
  const header = tr('DeiX OS — резервные коды 2FA для {name}', { name: me.username });
  const text = `${header}\n${tr('Каждый код работает один раз.')}\n\n${shownCodes.join('\n')}\n`;
  const link = el('a', { href: URL.createObjectURL(new Blob([text], { type: 'text/plain' })), download: 'deix-backup-codes.txt' });
  document.body.append(link);
  link.click();
  link.remove();
});

document.getElementById('codes-done').addEventListener('click', () => {
  shownCodes = [];
  document.getElementById('mfa-codes-list').replaceChildren();
  codesBox.hidden = true;
  renderMfa();
});

function askCode(action) {
  pendingAction = action;
  const form = document.getElementById('mfa-confirm');
  form.hidden = false;
  mfaResult.hidden = true;
  document.getElementById('mfa-confirm-btn').textContent = action === 'mfa_disable' ? tr('Отключить') : tr('Создать коды');
  document.getElementById('mfa-confirm-code').focus();
}

document.getElementById('mfa-regen').addEventListener('click', () => askCode('mfa_backup_regenerate'));
document.getElementById('mfa-off').addEventListener('click', () => askCode('mfa_disable'));
document.getElementById('mfa-confirm-cancel').addEventListener('click', () => {
  document.getElementById('mfa-confirm').hidden = true;
});

document.getElementById('mfa-confirm').addEventListener('submit', async (event) => {
  event.preventDefault();
  const input = document.getElementById('mfa-confirm-code');
  try {
    const data = await authApi({ action: pendingAction, code: input.value });
    input.value = '';
    document.getElementById('mfa-confirm').hidden = true;
    if (pendingAction === 'mfa_disable') {
      await refreshMe();
      toast(tr('Двухфакторная аутентификация отключена'), 'ok');
    } else {
      showCodes(data.backup_codes);
    }
  } catch (err) {
    showResult(mfaResult, 'err', err.message);
  }
});
