'use strict';

/**
 * Английские варианты сообщений API. Ключ — русский текст, как он написан
 * в коде; язык берётся из заголовка `x-deix-lang`, который шлёт сайт.
 */
const EN = {
  'Файл манифеста не распознан': 'The manifest file was not recognised',
  'Подпись манифеста: нужно 128 hex-символов': 'Manifest signature: 128 hex characters are required',
  'Манифест должен быть одной строкой ASCII-JSON до 16000 символов': 'The manifest must be a single ASCII JSON line of up to 16000 characters',
  'Манифест не является корректным JSON': 'The manifest is not valid JSON',
  'Манифест содержит некорректные поля': 'The manifest contains invalid fields',
  'Подпись манифеста не сходится с ключом обновлений': 'The manifest signature does not match the update key',
  'Обновление не найдено': 'Update not found',
  'Обновление с такой сборкой уже добавлено': 'An update with this build has already been added',
  'Файл обновления ещё не загружен': 'The update file has not been uploaded yet',
  'Слишком много запросов с этого адреса. Попробуйте позже.': 'Too many requests from this address. Try again later.',
  'Неизвестный канал обновлений': 'Unknown update channel',
  'Некорректный номер сборки или части': 'Invalid build or part number',
  'Такой сборки нет или она не опубликована': 'No such build, or it is not published',
  'Такой части нет': 'No such part',
  'Не удалось прочитать файл из хранилища': 'Could not read the file from storage',
  'Некорректный номер сборки': 'Invalid build number',
  'Укажите корректную почту': 'Enter a valid email address',
  'Одноразовые адреса почты не принимаются': 'Disposable email addresses are not accepted',
  'У этого домена нет почтового сервера. Проверьте адрес.': 'This domain has no mail server. Check the address.',
  'Внутренняя ошибка сервера': 'Internal server error',
  'Сервер не настроен: не заданы переменные окружения': 'The server is not configured: environment variables are missing',
  'База данных недоступна': 'The database is unavailable',
  'Ошибка базы данных': 'Database error',
  'Сервер аккаунтов недоступен': 'The account server is unavailable',
  'Некорректный JSON': 'Invalid JSON',
  'Метод не поддерживается': 'Method not allowed',
  'Запрос отклонён': 'Request rejected',
  'Подтвердите, что вы не робот': 'Please confirm you are not a robot',
  'Не удалось проверить капчу, попробуйте ещё раз': 'Could not check the captcha, please try again',
  'Капча не пройдена, попробуйте ещё раз': 'The captcha was not passed, please try again',
  'Требуется вход в аккаунт': 'Sign-in required',
  'Нужны права Owner': 'Owner rights required',
  'Для панели Owner нужна двухфакторная аутентификация: включите её в аккаунте и войдите заново.': 'The Owner panel requires two-factor authentication: enable it in your account and sign in again.',
  'Подтвердите почту: ссылка отправлена в письме при регистрации.': 'Verify your email: the link was sent in the registration email.',
  'Некорректный Device ID': 'Invalid Device ID',
  'Неизвестное решение': 'Unknown decision',
  'Нужно значение open: true или false': 'The value open: true or false is required',
  'Неизвестное действие': 'Unknown action',
  'Пароль не должен быть длиннее 72 символов': 'The password must not be longer than 72 characters',
  'Регистрация отклонена': 'Registration rejected',
  'Слишком много регистраций с этого адреса. Попробуйте позже.': 'Too many registrations from this address. Try again later.',
  'Имя пользователя: 3–24 символа, латинские буквы, цифры и _': 'Username: 3–24 characters, Latin letters, digits and _',
  'Пароли не совпадают': 'Passwords do not match',
  'Это имя пользователя уже занято': 'This username is already taken',
  'Аккаунт с такой почтой уже существует': 'An account with this email already exists',
  'Не удалось создать аккаунт': 'Could not create the account',
  'Аккаунт создан, но войти не удалось. Попробуйте войти вручную.': 'The account was created, but signing in failed. Try signing in manually.',
  'Почта уже подтверждена': 'The email is already verified',
  'Отправка писем на сервере не настроена': 'Email sending is not configured on the server',
  'Письмо уже отправлялось несколько раз. Попробуйте через час.': 'The email was already sent several times. Try again in an hour.',
  'Не удалось отправить письмо. Попробуйте позже.': 'Could not send the email. Try again later.',
  'Слишком много попыток': 'Too many attempts',
  'Ссылка подтверждения повреждена': 'The confirmation link is damaged',
  'Ссылка недействительна или устарела. Запросите новое письмо в аккаунте.': 'The link is invalid or has expired. Request a new email in your account.',
  'Слишком много неудачных попыток входа. Подождите 15 минут.': 'Too many failed sign-in attempts. Wait 15 minutes.',
  'Введите логин и пароль': 'Enter your username and password',
  'Неверный логин или пароль': 'Wrong username or password',
  'Профиль аккаунта не найден': 'Account profile not found',
  'Не удалось проверить код. Попробуйте ещё раз.': 'Could not check the code. Try again.',
  'Не удалось отключить двухфакторную аутентификацию': 'Could not turn off two-factor authentication',
  'Время ввода кода истекло. Войдите заново.': 'The code entry time has expired. Sign in again.',
  'Слишком много неверных кодов. Подождите 10 минут.': 'Too many wrong codes. Wait 10 minutes.',
  'Введите 6 цифр из приложения или резервный код': 'Enter the 6 digits from the app or a backup code',
  'Неверный код': 'Wrong code',
  'Двухфакторная аутентификация уже включена': 'Two-factor authentication is already on',
  'Не удалось начать подключение 2FA': 'Could not start 2FA setup',
  'Подключение не начато или уже завершено. Начните заново.': 'Setup has not started or is already finished. Start again.',
  'Введите 6 цифр из приложения': 'Enter the 6 digits from the app',
  'Неверный код. Проверьте время на телефоне и попробуйте ещё раз.': 'Wrong code. Check the time on your phone and try again.',
  'Двухфакторная аутентификация не включена': 'Two-factor authentication is not on',
  'Новый пароль совпадает с текущим': 'The new password is the same as the current one',
  'Текущий пароль указан неверно': 'The current password is wrong',
  'Не удалось сменить пароль': 'Could not change the password',
  'Код неверный или просрочен. Выполните «dev link» в DeiX и введите новый код.': 'The code is wrong or has expired. Run “dev link” in DeiX and enter a new code.',
  'К аккаунту можно привязать не больше 5 устройств.': 'You can link at most 5 devices to an account.',
  'Слишком много неверных кодов. Попробуйте через 15 минут.': 'Too many wrong codes. Try again in 15 minutes.',
  'Код состоит из 8 символов вида XXXX-XXXX.': 'The code is 8 characters long, like XXXX-XXXX.',
  'Такого устройства в вашем аккаунте нет': 'There is no such device in your account',
  'Device ID — это 32 символа 0-9 и a-f. Его печатает команда «dev id» в DeiX.': 'A Device ID is 32 characters 0-9 and a-f. The “dev id” command in DeiX prints it.',
  'Выдача ключей не настроена на сервере': 'Key issuing is not configured on the server',
  'Владелец проекта сейчас не выдаёт ключи.': 'The project owner is not issuing keys right now.',
  'Заявка на рассмотрении у владельца проекта.': 'The request is awaiting the project owner.',
  'Заявка одобрена. Токен получит ОС при ближайшей проверке: выполните в DeiX «dev request» или «dev on».': 'The request is approved. The OS gets the token on its next check: run “dev request” or “dev on” in DeiX.',
  'Одноразовый токен уже выдан устройству. Для нового токена подайте заявку ещё раз.': 'The one-time token was already issued to the device. File a new request for another token.',
  'Владелец проекта отклонил заявку.': 'The project owner rejected the request.',
  'Это устройство не привязано к вашему аккаунту. Привяжите его в разделе «Аккаунт».': 'This device is not linked to your account. Link it under “Account”.',
  'У вас уже 3 заявки на рассмотрении.': 'You already have 3 requests pending.',
  'Владелец проекта сейчас не принимает заявки.': 'The project owner is not accepting requests right now.',
  'Слишком много заявок. Попробуйте позже.': 'Too many requests. Try again later.',
  'Заявка отправлена владельцу проекта.': 'The request was sent to the project owner.',
  'Хранилище файлов недоступно': 'The file storage is unavailable',
  'Некорректный идентификатор версии': 'Invalid release identifier',
  'Версия не найдена': 'Release not found',
  'Версия: до 32 символов, латинские буквы, цифры, точка, дефис и _': 'Version: up to 32 characters, Latin letters, digits, dot, hyphen and _',
  'Название не длиннее 120 символов': 'The title must be at most 120 characters',
  'Описание не длиннее 4000 символов': 'The description must be at most 4000 characters',
  'Файл пустой': 'The file is empty',
  'Файл больше 50 МиБ: это предел хранилища': 'The file is larger than 50 MiB: that is the storage limit',
  'Некорректная контрольная сумма SHA-256': 'Invalid SHA-256 checksum',
  'Допустимые файлы: .img, .img.gz, .img.xz, .zip, .qcow2': 'Allowed files: .img, .img.gz, .img.xz, .zip, .qcow2',
  'Такая версия уже добавлена': 'This version has already been added',
  'Не удалось подготовить загрузку в хранилище': 'Could not prepare the upload to storage',
  'Файла нет в хранилище: загрузка не завершилась': 'The file is not in storage: the upload did not finish',
  'Не удалось проверить файл в хранилище': 'Could not check the file in storage',
  'Размер загруженного файла не совпал с заявленным, файл удалён': 'The uploaded file size does not match the declared one; the file was deleted',
  'Не удалось прочитать файл для проверки суммы': 'Could not read the file back to verify its checksum',
  'Контрольная сумма файла не совпала с манифестом, файл удалён': 'The file checksum does not match the manifest; the file was deleted',
  'Нужно значение published: true или false': 'The value published: true or false is required',
  'Файл версии ещё не загружен': 'The release file has not been uploaded yet',
  'Не удалось удалить файл из хранилища': 'Could not delete the file from storage',
  'Некорректная версия': 'Invalid version',
  'Слишком много загрузок с этого адреса. Попробуйте позже.': 'Too many downloads from this address. Try again later.',
  'Такой версии нет или она не опубликована': 'There is no such version or it is not published',
  'Не удалось получить ссылку на файл': 'Could not get the file link',
};

const PATTERNS = [
  [/^Не удалось подготовить загрузку в хранилище \(Storage (.+)\)$/, 'Could not prepare the upload to storage (Storage $1)'],
  [/^Пароль должен быть не короче (\d+) символов$/, 'The password must be at least $1 characters long'],
  [/^Не больше (\d+) ключей в час\. Попробуйте позже\.$/, 'No more than $1 keys per hour. Try again later.'],
];

function translate(text, lang) {
  if (lang !== 'en' || typeof text !== 'string') return text;
  if (Object.prototype.hasOwnProperty.call(EN, text)) return EN[text];
  for (const [re, out] of PATTERNS) {
    if (re.test(text)) return text.replace(re, out);
  }
  return text;
}

function requestLang(req) {
  return req && req.headers && req.headers['x-deix-lang'] === 'en' ? 'en' : 'ru';
}

/** Переводит поля `error` и `message` во всём ответе, включая вложенные списки. */
function translatePayload(value, lang) {
  if (lang !== 'en') return value;
  if (Array.isArray(value)) return value.map((item) => translatePayload(item, lang));
  if (value && typeof value === 'object') {
    const out = {};
    for (const [key, item] of Object.entries(value)) {
      out[key] = key === 'error' || key === 'message' ? translate(item, lang) : translatePayload(item, lang);
    }
    return out;
  }
  return value;
}

const MAIL = {
  ru: {
    subject: 'Подтвердите почту для DeiX OS',
    text: (name, link) => `Здравствуйте, ${name}!\n\nПодтвердите почту по ссылке (действует 24 часа):\n${link}\n\nЕсли вы не регистрировались в DeiX OS, просто проигнорируйте письмо.`,
    html: (name, link) => `<p>Здравствуйте, <b>${name}</b>!</p><p><a href="${link}">Подтвердить почту</a> (ссылка действует 24 часа).</p><p>Если вы не регистрировались в DeiX OS, просто проигнорируйте письмо.</p>`,
  },
  en: {
    subject: 'Verify your email for DeiX OS',
    text: (name, link) => `Hello, ${name}!\n\nVerify your email with this link (valid for 24 hours):\n${link}\n\nIf you did not sign up for DeiX OS, just ignore this email.`,
    html: (name, link) => `<p>Hello, <b>${name}</b>!</p><p><a href="${link}">Verify your email</a> (the link is valid for 24 hours).</p><p>If you did not sign up for DeiX OS, just ignore this email.</p>`,
  },
};

module.exports = { EN, PATTERNS, translate, requestLang, translatePayload, MAIL };
