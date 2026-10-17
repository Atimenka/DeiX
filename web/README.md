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
