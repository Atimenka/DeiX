# Сайт DeiX (Vercel)

Публичный сайт про DeiX, форма заявки на одноразовый токен dev-режима и
панель Owner. Без сборки и без зависимостей: статика в `public/` и две
serverless-функции в `api/` (Node 22).

| Путь | Назначение |
|---|---|
| `/` | о проекте |
| `/register`, `/login`, `/account` | регистрация, вход, смена пароля и выход |
| `/request` | заявка на токен для привязанного устройства (только для вошедших) и «Мои заявки» |
| `/admin` | панель Owner: принять/отклонить/удалить заявку, приём заявок вкл/выкл |
| `POST /api/auth` | `config`, `me`, `register`, `login`, `logout`, `change_password` |
| `POST /api/os?fn=...` | шлюз для ОС: `begin_device_link`, `device_link_status`, `request_dev_unlock`, `claim_dev_unlock` |
| `POST /api/devices` | `list`, `link` (код из `dev link`), `unlink` |
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

## Настройки проекта Vercel

* Root Directory `web`, Output Directory `public`, Install/Build Command —
  заглушки `echo ...` (сборки нет; пресет Framework выбран `Vite` только
  ради этого: API Vercel не принимает первый деплой проекта без пресета).
* Deployment Protection → Vercel Authentication должна быть **выключена**,
  иначе `*.vercel.app` просит вход в Vercel и сайт недоступен посетителям.
* Первый деплой сделан из ветки `arena/5fde79b0-deix`; автодеплой по
  push включается в Settings → Git (Create deployments).
