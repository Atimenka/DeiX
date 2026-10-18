# Сайт DeiX (Vercel)

Публичный сайт про DeiX, форма заявки на одноразовый токен dev-режима и
панель Owner. Без сборки и без зависимостей: статика в `public/` и две
serverless-функции в `api/` (Node 22).

| Путь | Назначение |
|---|---|
| `/` | о проекте |
| `/register`, `/login` | регистрация (проверка адреса, письмо подтверждения) и вход (с шагом TOTP, если 2FA включена) |
| `/account` | обзор, устройства, dev-ключ, безопасность: подтверждение почты, 2FA с резервными кодами, смена пароля |
| `/verify?token=...` | подтверждение почты по ссылке из письма |
| `/request` | заявка на токен для привязанного устройства (только для вошедших) и «Мои заявки» |
| `/admin` | панель Owner (нужна сессия с 2FA): принять/отклонить/удалить заявку, приём заявок вкл/выкл |
| `POST /api/auth` | `config`, `me`, `register`, `login`, `logout`, `change_password`, `verify_email`, `resend_verification`, `mfa_verify`, `mfa_status`, `mfa_enroll_start`, `mfa_enroll_finish`, `mfa_disable`, `mfa_backup_regenerate` |
| `POST /api/os?fn=...` | шлюз для ОС: `begin_device_link`, `device_link_status`, `request_dev_unlock`, `claim_dev_unlock` |
| `POST /api/devices` | `list`, `link` (код из `dev link`), `unlink` |
| `POST /api/keys` | `list`, `issue` (офлайн-ключ `dev unlock` для Device ID; нужна переменная `DEV_KEY_PRIVATE`) |
| `POST /api/request` | `mine`, `submit` |
| `POST /api/admin` | `list`, `decide`, `remove`, `set_open` (только role=owner) |

Все POST требуют заголовок `X-DeiX-CSRF: 1` и same-origin. Сессия — cookies
`deix_at`/`deix_rt` (HttpOnly, Secure, SameSite=Strict, 30 дней) поверх
Supabase Auth.

Переменные окружения Vercel: `SUPABASE_URL`, `SUPABASE_ANON_KEY`,
`SUPABASE_SERVICE_ROLE_KEY`, `TURNSTILE_SITE_KEY`, `TURNSTILE_SECRET`
(без двух последних капча выключена). Схема и создание Owner описаны в
`docs/DEVAUTH.md`; тесты API: `npm test` (без сети, Supabase подменён).

Root Directory проекта Vercel — `web`.

## Почта и двухфакторная аутентификация

* **Проверка адреса при регистрации** работает всегда: домены одноразовой
  почты отклоняются, у домена должен быть MX (или A) в DNS. Сбой самого
  DNS-запроса адрес не отклоняет.
* **Письмо со ссылкой подтверждения** уходит, если заданы `MAIL_FROM` и
  один из ключей: `RESEND_API_KEY` (resend.com) или `BREVO_API_KEY`
  (brevo.com). Необязательная `SITE_URL` задаёт адрес ссылки (по умолчанию —
  хост запроса). Пока ключа нет, письма не отправляются и подтверждение
  ничего не блокирует; с ключом неподтверждённые аккаунты не получают
  dev-ключи и не подают заявки. Вход подтверждение не блокирует.
  Отправитель в Resend должен принадлежать подтверждённому домену, в Brevo —
  подтверждённому адресу отправителя.
* **2FA (TOTP)** использует MFA Supabase Auth (в проекте включён TOTP).
  Вход: пароль → cookie `deix_mfa` (5 минут, только `/api`) → код →
  обычная сессия уровня aal2. Подключение и отключение — в «Аккаунт» →
  «Безопасность». 8 резервных кодов хранятся хэшами SHA-256; резервный код
  срабатывает один раз и отключает 2FA. Не больше 5 неверных кодов за
  10 минут на аккаунт.
* **Owner**: `/admin` и `POST /api/admin` требуют сессию aal2, иначе
  отвечают 403 с `code: "mfa_required"`.
* Миграция: `supabase/migrations/20261018100000_security.sql`.

## Настройки проекта Vercel

* Root Directory `web`, Output Directory `public`, Install/Build Command —
  заглушки `echo ...` (сборки нет; пресет Framework выбран `Vite` только
  ради этого: API Vercel не принимает первый деплой проекта без пресета).
* Deployment Protection → Vercel Authentication должна быть **выключена**,
  иначе `*.vercel.app` просит вход в Vercel и сайт недоступен посетителям.
* Первый деплой сделан из ветки `arena/5fde79b0-deix`; автодеплой по
  push включается в Settings → Git (Create deployments).
