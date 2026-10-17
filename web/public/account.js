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
})();

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
