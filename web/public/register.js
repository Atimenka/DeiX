'use strict';

const result = document.getElementById('register-result');
const next = nextPath();
document.getElementById('to-login').href = `/login?next=${encodeURIComponent(next)}`;

(async () => {
  if (await fetchMe()) location.replace(next);
})();

const captchaReady = mountCaptcha(document.getElementById('captcha'));

document.getElementById('register-form').addEventListener('submit', async (event) => {
  event.preventDefault();
  const password = document.getElementById('password').value;
  if (password !== document.getElementById('password2').value) {
    showResult(result, 'err', 'Пароли не совпадают');
    return;
  }
  const captcha = await captchaReady;
  const button = document.getElementById('register-btn');
  button.disabled = true;
  try {
    await authApi({
      action: 'register',
      username: document.getElementById('username').value,
      email: document.getElementById('email').value,
      password,
      password2: document.getElementById('password2').value,
      website: document.getElementById('website').value,
      captcha: captcha.token(),
    });
    location.assign(next);
  } catch (err) {
    showResult(result, 'err', err.message);
    captcha.reset();
    button.disabled = false;
  }
});
