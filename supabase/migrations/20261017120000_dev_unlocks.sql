-- Активация dev-режима DeiX: серверный реестр устройств и одноразовые токены.
--
-- Поток:
--   1. Заявку подаёт ОС (`dev request`) или посетитель сайта (форма на
--      /request) — функцией request_dev_unlock; status = 'pending'.
--   2. Владелец проекта в админ-панели сайта одобряет заявку
--      (approve_dev_unlock генерирует токен, status = 'approved') или
--      отклоняет (deny_dev_unlock, status = 'denied').
--   3. ОС при опросе вызывает claim_dev_unlock: при 'approved' токен
--      отдаётся ОДИН раз, строка переходит в 'consumed', токен из
--      таблицы удаляется. Повторный вызов токен уже не вернёт.
--
-- Прямого доступа к таблицам у anon/authenticated нет вообще: RLS
-- включён без политик, права отозваны. Публичный ключ ОС может только
-- вызвать request_dev_unlock и claim_dev_unlock. Админ-функции и
-- таблицы доступны одному service_role (ключ хранится только в
-- переменных окружения сервера сайта).
--
-- Скрипт идемпотентен и доводит до нужной схемы как пустой проект, так
-- и таблицу-заготовку public.dev_unlocks, созданную ранее.

create table if not exists public.dev_unlocks (
  device_id  text primary key,
  hostname   text,
  status     text not null default 'pending',
  unlock_key text,
  created_at timestamptz not null default now()
);

alter table public.dev_unlocks alter column hostname drop not null;
alter table public.dev_unlocks alter column unlock_key drop not null;
alter table public.dev_unlocks alter column unlock_key drop default;
update public.dev_unlocks set unlock_key = null where unlock_key = '';

alter table public.dev_unlocks add column if not exists note        text;
alter table public.dev_unlocks add column if not exists source      text not null default 'os';
alter table public.dev_unlocks add column if not exists decided_at  timestamptz;
alter table public.dev_unlocks add column if not exists decided_by  text;
alter table public.dev_unlocks add column if not exists consumed_at timestamptz;

alter table public.dev_unlocks drop constraint if exists dev_unlocks_unlock_key_check;
alter table public.dev_unlocks drop constraint if exists dev_unlocks_status_check;
alter table public.dev_unlocks add  constraint dev_unlocks_status_check
  check (status in ('pending', 'approved', 'denied', 'consumed'));
alter table public.dev_unlocks drop constraint if exists dev_unlocks_source_check;
alter table public.dev_unlocks add  constraint dev_unlocks_source_check
  check (source in ('os', 'web'));
alter table public.dev_unlocks drop constraint if exists dev_unlocks_device_id_check;
alter table public.dev_unlocks add  constraint dev_unlocks_device_id_check
  check (device_id ~ '^[0-9a-f]{32}$');
alter table public.dev_unlocks drop constraint if exists dev_unlocks_key_matches_status;
alter table public.dev_unlocks add  constraint dev_unlocks_key_matches_status
  check ((status = 'approved') = (unlock_key is not null));

create index if not exists dev_unlocks_status_created_idx
  on public.dev_unlocks (status, created_at desc);

create table if not exists public.dev_settings (
  key   text primary key,
  value jsonb not null
);
insert into public.dev_settings (key, value)
  values ('requests_open', 'true'::jsonb)
  on conflict (key) do nothing;

alter table public.dev_unlocks enable row level security;
alter table public.dev_settings enable row level security;

do $$
declare pol record;
begin
  for pol in
    select schemaname, tablename, policyname from pg_policies
    where schemaname = 'public' and tablename in ('dev_unlocks', 'dev_settings')
  loop
    execute format('drop policy %I on %I.%I', pol.policyname, pol.schemaname, pol.tablename);
  end loop;
end $$;

revoke all on public.dev_unlocks from anon, authenticated;
revoke all on public.dev_settings from anon, authenticated;
grant all on public.dev_unlocks to service_role;
grant all on public.dev_settings to service_role;

-- ---------- публичные функции (ОС и форма сайта) ----------

create or replace function public.request_dev_unlock(
  p_device_id text,
  p_hostname  text default null,
  p_note      text default null,
  p_source    text default 'os'
) returns jsonb
language plpgsql
security definer
set search_path = public
as $$
declare
  cur     public.dev_unlocks;
  is_open boolean;
  existed boolean;
begin
  p_device_id := coalesce(p_device_id, '');
  if p_device_id !~ '^[0-9a-f]{32}$' then
    raise exception 'device id must be 32 lowercase hex characters';
  end if;
  if p_source is null or p_source not in ('os', 'web') then
    raise exception 'unknown request source';
  end if;
  p_hostname := left(nullif(btrim(p_hostname), ''), 64);
  p_note     := left(nullif(btrim(p_note), ''), 280);

  select * into cur from public.dev_unlocks where device_id = p_device_id for update;
  existed := found;
  if existed and cur.status <> 'consumed' then
    return jsonb_build_object('result', 'exists', 'status', cur.status);
  end if;

  select (value = 'true'::jsonb) into is_open from public.dev_settings where key = 'requests_open';
  if not coalesce(is_open, true) then
    raise exception 'the project owner is not accepting dev requests right now';
  end if;

  if existed then
    update public.dev_unlocks
       set status = 'pending', unlock_key = null, consumed_at = null,
           decided_at = null, decided_by = null, created_at = now(),
           hostname = coalesce(p_hostname, hostname),
           note = coalesce(p_note, note), source = p_source
     where device_id = p_device_id;
  else
    if (select count(*) from public.dev_unlocks where status = 'pending') >= 500 then
      raise exception 'too many pending requests, try again later';
    end if;
    insert into public.dev_unlocks (device_id, hostname, note, source)
      values (p_device_id, p_hostname, p_note, p_source);
  end if;
  return jsonb_build_object('result', 'created', 'status', 'pending');
end $$;

create or replace function public.claim_dev_unlock(p_device_id text)
returns jsonb
language plpgsql
security definer
set search_path = public
as $$
declare
  cur public.dev_unlocks;
begin
  select * into cur from public.dev_unlocks where device_id = coalesce(p_device_id, '') for update;
  if not found then
    return jsonb_build_object('status', 'none');
  end if;
  if cur.status = 'approved' then
    update public.dev_unlocks
       set status = 'consumed', unlock_key = null, consumed_at = now()
     where device_id = cur.device_id;
    return jsonb_build_object('status', 'approved', 'unlock_key', cur.unlock_key);
  end if;
  return jsonb_build_object('status', cur.status);
end $$;

-- ---------- админ-функции (только service_role) ----------

create or replace function public.approve_dev_unlock(p_device_id text, p_by text)
returns public.dev_unlocks
language plpgsql
security definer
set search_path = public
as $$
declare
  cur public.dev_unlocks;
begin
  select * into cur from public.dev_unlocks where device_id = p_device_id for update;
  if not found then
    raise exception 'request not found';
  end if;
  if cur.status not in ('pending', 'denied') then
    raise exception 'request is already %', cur.status;
  end if;
  update public.dev_unlocks
     set status = 'approved',
         unlock_key = replace(gen_random_uuid()::text, '-', ''),
         decided_at = now(), decided_by = p_by
   where device_id = p_device_id
   returning * into cur;
  return cur;
end $$;

create or replace function public.deny_dev_unlock(p_device_id text, p_by text)
returns public.dev_unlocks
language plpgsql
security definer
set search_path = public
as $$
declare
  cur public.dev_unlocks;
begin
  select * into cur from public.dev_unlocks where device_id = p_device_id for update;
  if not found then
    raise exception 'request not found';
  end if;
  if cur.status not in ('pending', 'approved') then
    raise exception 'request is already %', cur.status;
  end if;
  update public.dev_unlocks
     set status = 'denied', unlock_key = null,
         decided_at = now(), decided_by = p_by
   where device_id = p_device_id
   returning * into cur;
  return cur;
end $$;

revoke all on function public.request_dev_unlock(text, text, text, text) from public, anon, authenticated;
revoke all on function public.claim_dev_unlock(text) from public, anon, authenticated;
revoke all on function public.approve_dev_unlock(text, text) from public, anon, authenticated;
revoke all on function public.deny_dev_unlock(text, text) from public, anon, authenticated;

grant execute on function public.request_dev_unlock(text, text, text, text) to anon, authenticated, service_role;
grant execute on function public.claim_dev_unlock(text) to anon, authenticated, service_role;
grant execute on function public.approve_dev_unlock(text, text) to service_role;
grant execute on function public.deny_dev_unlock(text, text) to service_role;

notify pgrst, 'reload schema';
