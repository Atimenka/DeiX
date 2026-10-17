'use strict';

(async () => {
  const user = await fetchMe();
  if (!user) {
    location.replace('/login?next=/account');
    return;
  }
  document.getElementById('acc-name').textContent = user.username;
  document.getElementById('acc-email').textContent = user.email;
  document.getElementById('acc-role').textContent = user.role === 'owner' ? 'Owner' : 'Пользователь';
  document.getElementById('admin-link').hidden = user.role !== 'owner';
  document.getElementById('account-view').hidden = false;
  loadDevices();
})();

async function loadDevices() {
  const box = document.getElementById('devices');
  try {
    const data = await postJson('/api/devices', { action: 'list' });
    box.replaceChildren();
    if (!data.devices.length) {
      box.append(el('p', { class: 'hint', text: 'Привязанных устройств пока нет.' }));
      return;
    }
    for (const device of data.devices) {
      box.append(
        el('div', { class: 'mine-item device-row' }, [
          el('div', null, [
            el('span', { class: 'mono', text: device.device_id }),
            el('div', { class: 'hint', text: `Привязано ${new Date(device.linked_at).toLocaleString('ru-RU')}` }),
          ]),
          el('button', { class: 'btn small', type: 'button', text: 'Отвязать', onclick: () => unlinkDevice(device.device_id) }),
        ]),
      );
    }
  } catch (err) {
    box.replaceChildren(el('p', { class: 'hint', text: err.message }));
  }
}

async function unlinkDevice(id) {
  if (!confirm('Отвязать устройство? Подать заявку с него можно будет только после новой привязки.')) return;
  try {
    await postJson('/api/devices', { action: 'unlink', device_id: id });
    toast('Устройство отвязано', 'ok');
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
    showResult(result, 'ok', `Устройство ${data.device_id.slice(0, 8)}… привязано. В DeiX можно выполнить «dev request».`);
    loadDevices();
  } catch (err) {
    showResult(result, 'err', err.message);
  }
});

document.getElementById('logout-btn').addEventListener('click', async () => {
  try {
    await authApi({ action: 'logout' });
  } finally {
    location.assign('/');
  }
});

document.getElementById('password-form').addEventListener('submit', async (event) => {
  event.preventDefault();
  const result = document.getElementById('pw-result');
  const next = document.getElementById('pw-next').value;
  if (next !== document.getElementById('pw-next2').value) {
    showResult(result, 'err', 'Новые пароли не совпадают');
    return;
  }
  try {
    await authApi({ action: 'change_password', current: document.getElementById('pw-current').value, next });
    event.target.reset();
    showResult(result, 'ok', 'Пароль изменён');
  } catch (err) {
    showResult(result, 'err', err.message);
  }
});
