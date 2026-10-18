'use strict';

const result = document.getElementById('register-result');
const next = nextPath();
document.getElementById('to-login').href = `${lp('/login')}?next=${encodeURIComponent(next)}`;

(async () => {
  if (await fetchMe()) location.replace(next);
})();

const captchaReady = mountCaptcha(document.getElementById('captcha'));

const STRENGTH = [tr('Не короче 8 символов'), tr('Слабый пароль'), tr('Можно надёжнее'), tr('Хороший пароль'), tr('Отличный пароль')];

function passwordScore(password) {
  if (password.length < 8) return password ? 1 : 0;
  let score = 1;
  if (password.length >= 12) score += 1;
  if (/[a-z]/.test(password) && /[A-Z]/.test(password)) score += 1;
  if (/\d/.test(password) && /[^A-Za-z0-9]/.test(password)) score += 1;
  return Math.min(score, 4);
}

document.getElementById('password').addEventListener('input', (event) => {
  const score = passwordScore(event.target.value);
  document.getElementById('meter').className = `meter${score ? ` s${score}` : ''}`;
  document.getElementById('meter-hint').textContent = STRENGTH[score];
});

document.getElementById('register-form').addEventListener('submit', async (event) => {
  event.preventDefault();
  const password = document.getElementById('password').value;
  if (password !== document.getElementById('password2').value) {
    showResult(result, 'err', tr('Пароли не совпадают'));
    return;
  }
  const captcha = await captchaReady;
  const button = document.getElementById('register-btn');
  button.disabled = true;
  try {
    const data = await authApi({
      action: 'register',
      username: document.getElementById('username').value,
      email: document.getElementById('email').value,
      password,
      password2: document.getElementById('password2').value,
      website: document.getElementById('website').value,
      captcha: captcha.token(),
    });
    if (data.verification === 'sent') {
      sessionStorage.setItem('deix-flash', tr('Аккаунт создан. Мы отправили письмо со ссылкой подтверждения.'));
    } else if (data.verification === 'failed') {
      sessionStorage.setItem('deix-flash', tr('Аккаунт создан, но письмо отправить не удалось: запросите его ещё раз в аккаунте.'));
    }
    location.assign(next);
  } catch (err) {
    showResult(result, 'err', err.message);
    captcha.reset();
    button.disabled = false;
  }
});
