-- Оптимизация БД + роли + dev-toggle + feature-flags.
-- Документация: docs/SECURITY_AUDIT_2026-11-01.md

-- ============================================
-- 1. Индекс на dev_unlocks.status
-- ============================================
-- dev on/off проверяет статус — без индекса PostgREST делает full table scan.
-- На тысячах записей это заметно.
create index if not exists idx_dev_unlocks_status
  on public.dev_unlocks (status);

-- Индекс по времени — для сортировки и cleanup давних заявок.
create index if not exists idx_dev_unlocks_requested_at
  on public.dev_unlocks (requested_at);

-- ============================================
-- 2. dev_mode — глобальный тумблер dev-режима
-- ============================================
-- OS при `dev on` спрашивает: включен ли dev-режим на сервере?
-- Если NO — отказ, даже если unlock_key правильный.
-- Если YES — dev-режим активируется.
create table public.dev_mode (
  id            integer primary key default 1,
  enabled       boolean not null default false,
  updated_at    timestamptz not null default now(),
  updated_by    text
);

-- RLS: чтение — только anon (OS спрашивает), письмо — только service role.
alter table public.dev_mode enable row level security;

-- OS может читать тумблер.
create policy "os can read dev_mode"
  on public.dev_mode
  for select
  to anon
  using (id = 1);

-- INSERT/UPDATE — только service role (Owner через дашборд или API).
-- anon не может менять тумблер — это и есть "сервер говорит НЕТ".
create policy "owner can manage dev_mode"
  on public.dev_mode
  for all
  to service_role
  using (true)
  with check (true);

-- Настраиваем: dev-режим по умолчанию отключен.
insert into public.dev_mode (id, enabled) values (1, false)
on conflict (id) do nothing;

-- ============================================
-- 3. feature_flags — тумблер по частям
-- ============================================
-- Owner может отключать функционал по частям:
--   dev_mode       — включить dev-режим на всех устройствах
--   cloud_sync     — облачная синхронизация настроек
--   cloud_clipboard — общий буфер обмена
--   os_updates     — обновления ОС
--   media_player   — медиаплеер
--   wallpapers     — обои
--   widgets        — виджеты
--   2048           — пасхалка 2048
create table public.feature_flags (
  name        text primary key,
  enabled     boolean not null default true,
  description text,
  updated_at  timestamptz not null default now(),
  updated_by  text
);

alter table public.feature_flags enable row level security;

-- OS читает flags, чтобы знать что включено/отключено.
create policy "os can read feature_flags"
  on public.feature_flags
  for select
  to anon
  using (true);

-- Owner меняет flags.
create policy "owner can manage feature_flags"
  on public.feature_flags
  for all
  to service_role
  using (true)
  with check (true);

-- Включаем всё по умолчанию.
insert into public.feature_flags (name, enabled, description) values
  ('dev_mode',       false, 'Dev-режим на всех устройствах'),
  ('cloud_sync',     true,  'Облачная синхронизация настроек'),
  ('cloud_clipboard',true,  'Общий буфер обмена между устройствами'),
  ('os_updates',     true,  'Обновления ОС'),
  ('media_player',   true,  'Медиаплеер (Photos/Video/Music)'),
  ('wallpapers',     true,  'Встроенные обои'),
  ('widgets',        true,  'Виджеты рабочего стола'),
  ('2048',           true,  'Пасхалка 2048')
on conflict (name) do nothing;

-- ============================================
-- 4. roles — роли для пользователей
-- ============================================
-- tester   — может тестировать dev-режим, нет прав на управление
-- moderator — может управлять заявками dev, нет прав на настройки
-- admin    — полные права (кроме owner)
-- owner    — superuser, единственный
create table public.roles (
  name        text primary key,
  description text,
  rank        integer not null
);

-- Ранги: выше = больше прав.
-- owner=100, admin=50, moderator=20, tester=10
insert into public.roles (name, description, rank) values
  ('owner',      'Superuser — полный доступ',      100),
  ('admin',      'Администратор — управление всем', 50),
  ('moderator',  'Модератор — управление заявками', 20),
  ('tester',     'Тестировщик — тестирование dev-режима', 10)
on conflict (name) do nothing;

-- Привязка роли к пользователю Supabase Auth.
create table public.user_roles (
  user_id      uuid primary key references auth.users(id) on delete cascade,
  role_name    text not null references public.roles(name),
  assigned_at  timestamptz not null default now(),
  assigned_by  text
);

alter table public.user_roles enable row level security;

-- Пользователь видит свою роль.
create policy "user can read own role"
  on public.user_roles
  for select
  to anon, authenticated
  using (user_id = auth.uid());

-- Пользователь сам не может менять роль — только service role.
create policy "service can manage roles"
  on public.user_roles
  for all
  to service_role
  using (true)
  with check (true);

-- ============================================
-- 5. dev_unlocks — добавляем column для роли заявителя
-- ============================================
-- Не критично, но полезно для модерации.
alter table public.dev_unlocks
  add column if not exists requester_role text
  default 'tester';
