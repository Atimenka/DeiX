-- Центр обновлений ОС: подписанные манифесты и нагрузка пакета обновления.
--
-- Подпись манифеста проверяет сама ОС (ECDSA P-256, публичный ключ вшит в
-- ядро); сервер хранит текст манифеста байт-в-байт, потому что подпись
-- считается по точной последовательности байт. Нагрузка лежит в приватном
-- бакете os-images по пути «updates/<build>/payload.bin» и отдаётся ОС
-- кусками через /api/update. У anon и authenticated доступа нет.
-- Скрипт идемпотентен.

create table if not exists public.os_updates (
  id           uuid primary key default gen_random_uuid(),
  build        bigint not null unique check (build > 0),
  version      text not null check (version ~ '^[0-9A-Za-z][0-9A-Za-z._-]{0,31}$'),
  channel      text not null check (channel in ('stable', 'beta')),
  manifest     text not null check (char_length(manifest) between 2 and 16000),
  signature    text not null check (signature ~ '^[0-9a-f]{128}$'),
  payload_path text not null,
  payload_size bigint not null check (payload_size > 0 and payload_size <= 6291456),
  ready        boolean not null default false,
  published    boolean not null default false,
  downloads    bigint not null default 0,
  created_by   text,
  created_at   timestamptz not null default now(),
  published_at timestamptz
);

create index if not exists os_updates_public_idx
  on public.os_updates (channel, published, ready, build desc);

alter table public.os_updates enable row level security;
revoke all on public.os_updates from anon, authenticated;
grant all on public.os_updates to service_role;

create or replace function public.bump_update_downloads(p_id uuid)
returns void
language sql
security definer
set search_path to 'public'
as $$
  update public.os_updates set downloads = downloads + 1 where id = p_id;
$$;

revoke all on function public.bump_update_downloads(uuid) from public, anon, authenticated;
grant execute on function public.bump_update_downloads(uuid) to service_role;
