-- Привязка устройств DeiX к аккаунтам сайта.
--
-- Поток:
--   1. В ОС `dev link` вызывает begin_device_link: устройство получает
--      одноразовый код вида XXXX-XXXX, действующий 10 минут.
--   2. Вошедший на сайте пользователь вводит код в /account; сервер сайта
--      вызывает link_device и записывает устройство на его аккаунт.
--   3. Заявку на dev-токен можно подать только для привязанного
--      устройства (из ОС или с сайта): request_dev_unlock отказывает
--      остальным, а владельцем заявки становится хозяин устройства.
--
-- У anon и authenticated доступа к таблице нет; ОС может вызвать только
-- begin_device_link и device_link_status. Привязка, отвязка и список
-- устройств — функции и таблица service_role.
--
-- Скрипт идемпотентен.

create table if not exists public.device_links (
  device_id       text primary key check (device_id ~ '^[0-9a-f]{32}$'),
  user_id         uuid references public.profiles (id) on delete cascade,
  code            text,
  code_expires_at timestamptz,
  created_at      timestamptz not null default now(),
  linked_at       timestamptz,
  constraint device_links_state check (
    (user_id is not null and code is null and linked_at is not null)
    or (user_id is null and code is not null and code_expires_at is not null)
  )
);

create unique index if not exists device_links_code_key
  on public.device_links (code) where code is not null;
create index if not exists device_links_user_idx on public.device_links (user_id);

alter table public.device_links enable row level security;
revoke all on public.device_links from anon, authenticated;
grant all on public.device_links to service_role;

create or replace function public.begin_device_link(p_device_id text)
returns jsonb
language plpgsql
security definer
set search_path = public, extensions
as $$
declare
  cur      public.device_links;
  alphabet constant text := 'ABCDEFGHJKLMNPQRSTUVWXYZ23456789';
  fresh    text;
  attempt  int := 0;
begin
  p_device_id := coalesce(p_device_id, '');
  if p_device_id !~ '^[0-9a-f]{32}$' then
    raise exception 'device id must be 32 lowercase hex characters';
  end if;

  delete from public.device_links
   where user_id is null and code_expires_at < now() - interval '1 hour';

  select * into cur from public.device_links where device_id = p_device_id for update;
  if found and cur.user_id is not null then
    return jsonb_build_object(
      'status', 'linked',
      'username', (select username from public.profiles where id = cur.user_id));
  end if;
  if found and cur.code_expires_at > now() then
    return jsonb_build_object(
      'status', 'code',
      'code', substr(cur.code, 1, 4) || '-' || substr(cur.code, 5),
      'expires_in', greatest(0, extract(epoch from cur.code_expires_at - now())::int));
  end if;

  if not found
     and (select count(*) from public.device_links where user_id is null) >= 2000 then
    raise exception 'too many pending device links, try again later';
  end if;

  loop
    fresh := '';
    for i in 1..8 loop
      fresh := fresh || substr(alphabet, 1 + (get_byte(gen_random_bytes(1), 0) % 32), 1);
    end loop;
    begin
      insert into public.device_links (device_id, code, code_expires_at)
        values (p_device_id, fresh, now() + interval '10 minutes')
        on conflict (device_id) do update
          set code = excluded.code, code_expires_at = excluded.code_expires_at;
      exit;
    exception when unique_violation then
      attempt := attempt + 1;
      if attempt > 5 then
        raise exception 'could not allocate a link code, try again';
      end if;
    end;
  end loop;

  return jsonb_build_object(
    'status', 'code',
    'code', substr(fresh, 1, 4) || '-' || substr(fresh, 5),
    'expires_in', 600);
end $$;

create or replace function public.device_link_status(p_device_id text)
returns jsonb
language plpgsql
security definer
set search_path = public
as $$
declare
  cur public.device_links;
begin
  select * into cur from public.device_links where device_id = coalesce(p_device_id, '');
  if not found then
    return jsonb_build_object('status', 'none');
  end if;
  if cur.user_id is not null then
    return jsonb_build_object(
      'status', 'linked',
      'username', (select username from public.profiles where id = cur.user_id));
  end if;
  if cur.code_expires_at > now() then
    return jsonb_build_object(
      'status', 'code',
      'expires_in', greatest(0, extract(epoch from cur.code_expires_at - now())::int));
  end if;
  return jsonb_build_object('status', 'none');
end $$;

create or replace function public.link_device(p_user_id uuid, p_code text)
returns jsonb
language plpgsql
security definer
set search_path = public
as $$
declare
  norm text := upper(regexp_replace(coalesce(p_code, ''), '[^A-Za-z0-9]', '', 'g'));
  cur  public.device_links;
begin
  if not exists (select 1 from public.profiles where id = p_user_id) then
    raise exception 'account not found';
  end if;
  if (select count(*) from public.device_links where user_id = p_user_id) >= 5 then
    raise exception 'you can link at most 5 devices';
  end if;
  select * into cur from public.device_links
   where code = norm and user_id is null and code_expires_at > now()
   for update;
  if not found then
    raise exception 'invalid or expired code';
  end if;
  update public.device_links
     set user_id = p_user_id, code = null, code_expires_at = null, linked_at = now()
   where device_id = cur.device_id;
  return jsonb_build_object('device_id', cur.device_id);
end $$;

create or replace function public.unlink_device(p_user_id uuid, p_device_id text)
returns boolean
language plpgsql
security definer
set search_path = public
as $$
declare
  removed int;
begin
  delete from public.device_links where device_id = p_device_id and user_id = p_user_id;
  get diagnostics removed = row_count;
  return removed > 0;
end $$;

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
  cur      public.dev_unlocks;
  owner_id uuid;
  is_open  boolean;
  existed  boolean;
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

  select user_id into owner_id from public.device_links where device_id = p_device_id;
  if owner_id is null then
    raise exception 'device is not linked to an account: run ''dev link'' in DeiX';
  end if;

  select * into cur from public.dev_unlocks where device_id = p_device_id for update;
  existed := found;
  if existed and cur.status <> 'consumed' then
    return jsonb_build_object('result', 'exists', 'status', cur.status);
  end if;

  select (value = 'true'::jsonb) into is_open from public.dev_settings where key = 'requests_open';
  if not coalesce(is_open, true) then
    raise exception 'the project owner is not accepting dev requests right now';
  end if;

  if (select count(*) from public.dev_unlocks
       where user_id = owner_id and status = 'pending'
         and device_id <> p_device_id) >= 3 then
    raise exception 'this account already has 3 pending requests';
  end if;

  if existed then
    update public.dev_unlocks
       set status = 'pending', unlock_key = null, consumed_at = null,
           decided_at = null, decided_by = null, created_at = now(),
           hostname = coalesce(p_hostname, hostname),
           note = coalesce(p_note, note), source = p_source, user_id = owner_id
     where device_id = p_device_id;
  else
    if (select count(*) from public.dev_unlocks where status = 'pending') >= 500 then
      raise exception 'too many pending requests, try again later';
    end if;
    insert into public.dev_unlocks (device_id, hostname, note, source, user_id)
      values (p_device_id, p_hostname, p_note, p_source, owner_id);
  end if;
  return jsonb_build_object('result', 'created', 'status', 'pending');
end $$;

create or replace function public.web_request_dev_unlock(
  p_user_id   uuid,
  p_device_id text,
  p_name      text default null,
  p_note      text default null
) returns jsonb
language plpgsql
security definer
set search_path = public
as $$
begin
  if not exists (
    select 1 from public.device_links
     where device_id = coalesce(p_device_id, '') and user_id = p_user_id
  ) then
    raise exception 'this device is not linked to your account';
  end if;
  return public.request_dev_unlock(p_device_id, p_name, p_note, 'web');
end $$;

revoke all on function public.begin_device_link(text) from public, anon, authenticated;
revoke all on function public.device_link_status(text) from public, anon, authenticated;
revoke all on function public.link_device(uuid, text) from public, anon, authenticated;
revoke all on function public.unlink_device(uuid, text) from public, anon, authenticated;
revoke all on function public.web_request_dev_unlock(uuid, text, text, text) from public, anon, authenticated;

grant execute on function public.begin_device_link(text) to anon, authenticated, service_role;
grant execute on function public.device_link_status(text) to anon, authenticated, service_role;
grant execute on function public.link_device(uuid, text) to service_role;
grant execute on function public.unlink_device(uuid, text) to service_role;
grant execute on function public.web_request_dev_unlock(uuid, text, text, text) to service_role;

notify pgrst, 'reload schema';
