# Сайт DeiX (Vercel)

Публичный сайт про DeiX, форма заявки на одноразовый токен dev-режима и
панель Owner. Без сборки и без зависимостей: статика в `public/` и две
serverless-функции в `api/` (Node 22).

| Путь | Назначение |
|---|---|
| `/` | о проекте |
| `/request` | заявка на токен и проверка статуса по Device ID |
| `/admin` | вход Owner, принятие и отклонение заявок, приём заявок вкл/выкл, смена пароля |
| `POST /api/request` | публичное API (`submit`, `status`) |
| `POST /api/admin` | API Owner (`login`, `me`, `list`, `decide`, `remove`, `set_open`, `change_password`, `logout`) |

Переменные окружения Vercel: `SUPABASE_URL`, `SUPABASE_SERVICE_ROLE_KEY`,
`SESSION_SECRET`. Схема базы и учётная запись Owner описаны в
`docs/DEVAUTH.md`; тесты API: `npm test` (без сети, база подменена).

Root Directory проекта Vercel — `web`.

## Настройки проекта Vercel

* Root Directory `web`, Output Directory `public`, Install/Build Command —
  заглушки `echo ...` (сборки нет; пресет Framework выбран `Vite` только
  ради этого: API Vercel не принимает первый деплой проекта без пресета).
* Deployment Protection → Vercel Authentication должна быть **выключена**,
  иначе `*.vercel.app` просит вход в Vercel и сайт недоступен посетителям.
* Первый деплой сделан из ветки `arena/5fde79b0-deix`; автодеплой по
  push включается в Settings → Git (Create deployments).
