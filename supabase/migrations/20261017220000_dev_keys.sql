-- Журнал выданных офлайн-ключей разблокировки dev-режима.
--
-- Ключ подписывается сервером сайта (ECDSA P-256) для конкретного device_id;
-- сам ключ нигде не хранится, в журнал попадает только nonce. Журнал нужен
-- для лимита выдачи и для разбора, кто и для какого устройства получал ключ.
-- У anon и authenticated доступа к таблице нет.
--
-- Скрипт идемпотентен.

create table if not exists public.dev_keys (
  id         bigint generated always as identity primary key,
  user_id    uuid not null references public.profiles (id) on delete cascade,
  device_id  text not null check (device_id ~ '^[0-9a-f]{32}$'),
  nonce      text not null check (nonce ~ '^[0-9a-f]{16}$'),
  created_at timestamptz not null default now()
);

create index if not exists dev_keys_user_idx on public.dev_keys (user_id, created_at desc);

alter table public.dev_keys enable row level security;
revoke all on public.dev_keys from anon, authenticated;
grant all on public.dev_keys to service_role;
