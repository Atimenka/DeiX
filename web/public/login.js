'use strict';

const result = document.getElementById('login-result');
const next = nextPath();
document.getElementById('to-register').href = `${lp('/register')}?next=${encodeURIComponent(next)}`;

(async () => {
  if (await fetchMe()) location.replace(next);
})();

const captchaReady = mountCaptcha(document.getElementById('captcha'));
const stepPassword = document.getElementById('step-password');
const stepCode = document.getElementById('step-code');
const codeInput = document.getElementById('code');
const codeMode = document.getElementById('code-mode');
let backupMode = false;

function showStep(code) {
  stepPassword.hidden = code;
  stepCode.hidden = !code;
  result.hidden = true;
  if (code) {
    codeInput.value = '';
    codeInput.focus();
  }
}

function setBackupMode(on) {
  backupMode = on;
  codeInput.maxLength = on ? 11 : 6;
  codeInput.placeholder = on ? 'XXXXX-XXXXX' : '······';
  codeInput.classList.toggle('backup', on);
  codeInput.inputMode = on ? 'text' : 'numeric';
  codeInput.autocomplete = on ? 'off' : 'one-time-code';
  codeMode.textContent = on ? tr('Вернуться к коду из приложения') : tr('Использовать резервный код');
  document.getElementById('code-hint').textContent = on
    ? tr('Введите один из резервных кодов. Он сработает один раз и отключит 2FA: потом её можно подключить заново.')
    : tr('Откройте приложение-аутентификатор и введите шесть цифр для DeiX.');
  codeInput.value = '';
  codeInput.focus();
}

document.getElementById('login-form').addEventListener('submit', async (event) => {
  event.preventDefault();
  const button = document.getElementById('login-btn');
  const captcha = await captchaReady;
  button.disabled = true;
  try {
    const data = await authApi({
      action: 'login',
      identifier: document.getElementById('identifier').value,
      password: document.getElementById('password').value,
      captcha: captcha.token(),
    });
    if (data.mfa) {
      setBackupMode(false);
      showStep(true);
      button.disabled = false;
      return;
    }
    location.assign(next);
  } catch (err) {
    showResult(result, 'err', err.message);
    captcha.reset();
    button.disabled = false;
  }
});

async function submitCode() {
  const button = document.getElementById('code-btn');
  button.disabled = true;
  try {
    const data = await authApi({ action: 'mfa_verify', code: codeInput.value });
    if (data.mfa_reset) toast(tr('Резервный код использован: 2FA отключена. Подключите её заново в аккаунте.'), 'ok');
    location.assign(data.mfa_reset ? lp('/account#security') : next);
  } catch (err) {
    if (err.code === 'mfa_expired') {
      showStep(false);
      const captcha = await captchaReady;
      captcha.reset();
    }
    showResult(result, 'err', err.message);
    codeInput.select();
    button.disabled = false;
  }
}

document.getElementById('code-form').addEventListener('submit', (event) => {
  event.preventDefault();
  submitCode();
});

codeInput.addEventListener('input', () => {
  if (!backupMode) {
    codeInput.value = codeInput.value.replace(/\D/g, '').slice(0, 6);
    if (codeInput.value.length === 6) submitCode();
  } else {
    codeInput.value = codeInput.value.toUpperCase();
  }
});

codeMode.addEventListener('click', () => setBackupMode(!backupMode));
document.getElementById('code-back').addEventListener('click', async () => {
  showStep(false);
  (await captchaReady).reset();
});
