'use strict';

const LANG = document.documentElement.lang === 'en' ? 'en' : 'ru';
const LOCALE = LANG === 'en' ? 'en-GB' : 'ru-RU';

/** Адрес страницы сайта на текущем языке: `/login` → `/en/login` в английской версии. */
function lp(path) {
  if (LANG !== 'en' || path.startsWith('/en/') || path === '/en') return path;
  if (path === '/') return '/en';
  if (path.startsWith('/#')) return `/en${path.slice(1)}`;
  return `/en${path}`.replace('next=/', 'next=/en/');
}

/** Тот же адрес в другом языке: для переключателя RU / EN. */
function otherLangPath() {
  const path = location.pathname;
  const target = LANG === 'en' ? path.replace(/^\/en(?=\/|$)/, '') || '/' : `/en${path === '/' ? '' : path}`;
  return `${target}${location.search}${location.hash}`;
}

const EN = {
  ' командой dev link в DeiX.': ' with the dev link command in DeiX.',
  '2FA включена': '2FA on',
  '2FA выключена': '2FA off',
  'Dev-режим': 'Dev mode',
  'QR-код построить не удалось, введите ключ вручную.': 'Could not draw the QR code, enter the key manually.',
  'Аккаунт создан, но письмо отправить не удалось: запросите его ещё раз в аккаунте.': 'Account created, but the email could not be sent: request it again from your account.',
  'Аккаунт создан. Мы отправили письмо со ссылкой подтверждения.': 'Account created. We sent you an email with a confirmation link.',
  'Браузер не поддерживается капчей Cloudflare. Обновите браузер.': 'This browser is not supported by Cloudflare captcha. Please update it.',
  'В аккаунт': 'To account',
  'Ваш аккаунт не имеет прав Owner.': 'Your account does not have Owner rights.',
  'Введите один из резервных кодов. Он сработает один раз и отключит 2FA: потом её можно подключить заново.': 'Enter one of your backup codes. It works once and turns 2FA off; you can enable it again afterwards.',
  'Вернуться к коду из приложения': 'Back to the app code',
  'Включена': 'On',
  'Включите 2FA': 'Turn on 2FA',
  'Включить 2FA': 'Turn on 2FA',
  'Возможности': 'Features',
  'Войти': 'Sign in',
  'Войти заново с кодом': 'Sign in again with a code',
  'Все': 'All',
  'Выключена': 'Off',
  'Главная': 'Home',
  'Двухфакторная аутентификация включена': 'Two-factor authentication is on',
  'Двухфакторная аутентификация отключена': 'Two-factor authentication is off',
  'Для панели Owner двухфакторная аутентификация обязательна: без неё доступ к заявкам закрыт.': 'Two-factor authentication is required for the Owner panel: without it, access to requests is closed.',
  'Для панели Owner обязательна двухфакторная аутентификация.': 'Two-factor authentication is required for the Owner panel.',
  'Дополнительная защита аккаунта: после пароля потребуется код из приложения-аутентификатора.': 'Extra account protection: after your password you will need a code from an authenticator app.',
  'Защита от ботов на сервере пока не включена.': 'Bot protection is not enabled on the server yet.',
  'Заявка одобрена': 'Request approved',
  'Заявка отклонена': 'Request rejected',
  'Заявка удалена': 'Request deleted',
  'Заявки': 'Requests',
  'Заявок пока нет.': 'No requests yet.',
  'Использовать резервный код': 'Use a backup code',
  'Ключ скопирован': 'Key copied',
  'Коды скопированы': 'Codes copied',
  'Меню': 'Menu',
  'Можно надёжнее': 'Could be stronger',
  'На главную': 'Home',
  'На рассмотрении': 'Pending',
  'Настроить 2FA': 'Set up 2FA',
  'Не короче 8 символов': 'At least 8 characters',
  'Не подтверждена': 'Not verified',
  'Не удалось загрузить капчу': 'Could not load the captcha',
  'Не удалось подтвердить': 'Could not verify',
  'Не удалось скопировать: выделите ключ вручную': 'Could not copy: select the key manually',
  'Не удалось скопировать: выделите коды вручную': 'Could not copy: select the codes manually',
  'Недавно выданные ключи:': 'Recently issued keys:',
  'Нет доступа': 'No access',
  'Нет привязанных устройств. ': 'No linked devices. ',
  'Нет связи с сервером': 'Cannot reach the server',
  'Новые пароли не совпадают': 'New passwords do not match',
  'Нужен вход': 'Sign-in required',
  'Нужен вход с кодом': 'Sign-in with a code required',
  'ОС': 'OS',
  'Одобрена': 'Approved',
  'Одобренные': 'Approved',
  'Одобрить': 'Approve',
  'Ожидают': 'Pending',
  'Основная навигация': 'Main navigation',
  'Отвязать': 'Unlink',
  'Отвязать устройство? Подать заявку с него можно будет только после новой привязки.': 'Unlink this device? You will only be able to file a request from it after linking it again.',
  'Отклонена': 'Rejected',
  'Отклонить': 'Reject',
  'Отклонённые': 'Rejected',
  'Отключить': 'Turn off',
  'Откройте приложение-аутентификатор и введите шесть цифр для DeiX.': 'Open your authenticator app and enter the six digits for DeiX.',
  'Отличный пароль': 'Excellent password',
  'Отозвать': 'Revoke',
  'Отправить письмо ещё раз': 'Send the email again',
  'Отправка писем на сервере не настроена, поэтому подтверждение пока не требуется.': 'Email sending is not configured on the server, so verification is not required for now.',
  'Панель': 'Panel',
  'Панель Owner открывается только с двухфакторной аутентификацией. Подключите её в аккаунте, затем вернитесь сюда.': 'The Owner panel opens only with two-factor authentication. Enable it in your account, then come back here.',
  'Панель доступна только владельцу проекта. Войдите в аккаунт Owner.': 'The panel is available to the project owner only. Sign in with the Owner account.',
  'Пароли не совпадают': 'Passwords do not match',
  'Пароль изменён': 'Password changed',
  'Письмо отправлено. Проверьте почту.': 'Email sent. Check your inbox.',
  'Подтверждена': 'Verified',
  'Показать пароль': 'Show password',
  'Пользователь': 'User',
  'Почта подтверждена': 'Email verified',
  'При входе нужен код из приложения. Резервные коды помогут, если телефон потерян.': 'A code from the app is required at sign-in. Backup codes help if you lose your phone.',
  'Привяжите устройство': 'Link a device',
  'Привязанных устройств пока нет.': 'No linked devices yet.',
  'Приём заявок закрыт': 'Requests are closed',
  'Приём заявок открыт': 'Requests are open',
  'Регистрация': 'Sign up',
  'Резервный код использован: 2FA отключена. Подключите её заново в аккаунте.': 'Backup code used: 2FA is off. Turn it on again in your account.',
  'Светлая / тёмная тема': 'Light / dark theme',
  'Сессия закончилась. Войдите снова.': 'Your session has expired. Please sign in again.',
  'Скрыть пароль': 'Hide password',
  'Слабый пароль': 'Weak password',
  'Сменить тему': 'Change theme',
  'Создать коды': 'Create codes',
  'Теперь вам доступны ключи dev-режима и заявки.': 'Dev-mode keys and requests are now available to you.',
  'Токен выдан': 'Token issued',
  'Удалить': 'Delete',
  'Устройство отвязано': 'Device unlinked',
  'Хороший пароль': 'Good password',
  'Эта сессия открыта без кода из приложения. Войдите заново и введите код.': 'This session was opened without an app code. Sign in again and enter the code.',
  'анонимно (из ОС)': 'anonymous (from the OS)',
  'без названия': 'untitled',
  'неизвестен': 'unknown',
  'сайт': 'site',
  'Ошибка сервера ({status})': 'Server error ({status})',
  'Почта {email} не подтверждена: без этого недоступны ключи и заявки.': 'Email {email} is not verified: keys and requests are unavailable until it is.',
  'Если виджет пишет «Unable to connect to website», добавьте {host} в Cloudflare → Turnstile → ваш виджет → Hostname Management.': 'If the widget says “Unable to connect to website”, add {host} in Cloudflare → Turnstile → your widget → Hostname Management.',
  'Капча отклонила адрес {host} (код {code}). {hint}': 'The captcha rejected {host} (code {code}). {hint}',
  'Капча не загрузилась (код {code}). Проверьте соединение, отключите блокировщики и VPN, затем обновите страницу.': 'The captcha did not load (code {code}). Check your connection, disable blockers and VPN, then reload the page.',
  'Капча не завершила проверку за 10 секунд. {hint}': 'The captcha did not finish within 10 seconds. {hint}',
  'Привязано {date}': 'Linked {date}',
  'Обновлено: {time}': 'Updated: {time}',
  'Адрес {email} подтверждён.': 'The address {email} is verified.',
  'Мы отправили письмо со ссылкой на {email}. Без подтверждения недоступны ключи и заявки.': 'We sent an email with a link to {email}. Keys and requests are unavailable until you verify it.',
  'Устройство {id}… привязано. В DeiX можно выполнить «dev request».': 'Device {id}… is linked. You can now run “dev request” in DeiX.',
  'Ключ для устройства {id}… готов. Он работает только на нём и только один раз.': 'The key for device {id}… is ready. It works on that device only, and only once.',
  'DeiX OS — резервные коды 2FA для {name}': 'DeiX OS — 2FA backup codes for {name}',
  'Каждый код работает один раз.': 'Each code works once.',
  'Удалить заявку {id}?': 'Delete request {id}?',
};

/** Перевод строки интерфейса: ключ — русский текст, `{имя}` — подстановки. */
function tr(text, vars) {
  let out = LANG === 'en' && Object.prototype.hasOwnProperty.call(EN, text) ? EN[text] : text;
  if (vars) out = out.replace(/\{(\w+)\}/g, (_, key) => (key in vars ? String(vars[key]) : `{${key}}`));
  return out;
}
