'use strict';

const result = document.getElementById('login-result');
const next = nextPath();
document.getElementById('to-register').href = `/register?next=${encodeURIComponent(next)}`;

(async () => {
  if (await fetchMe()) location.replace(next);
})();

const captchaBox = document.getElementById('captcha');
const captchaReady = mountCaptcha(captchaBox);

document.getElementById('login-form').addEventListener('submit', async (event) => {
  event.preventDefault();
  const button = document.getElementById('login-btn');
  const captcha = await captchaReady;
  button.disabled = true;
  try {
    await authApi({
      action: 'login',
      identifier: document.getElementById('identifier').value,
      password: document.getElementById('password').value,
      captcha: captcha.token(),
    });
    location.assign(next);
  } catch (err) {
    showResult(result, 'err', err.message);
    captcha.reset();
    button.disabled = false;
  }
});
