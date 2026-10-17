-- Аккаунты сайта DeiX на Supabase Auth.
--
-- Пользователи живут в auth.users (почта + пароль, их хранит и проверяет
-- Supabase Auth). Здесь — профиль с публичным именем и ролью, привязка
-- заявок на dev-токен к аккаунту и несколько функций только для
-- service_role (ключ есть лишь на сервере сайта).
--
-- Публичная регистрация в Auth выключена (disable_signup): аккаунты
-- создаёт сервер сайта через admin API после проверки капчи, поэтому
-- обойти капчу прямым вызовом /auth/v1/signup нельзя.
--
-- Скрипт идемпотентен.

create table if not exists public.profiles (
  id         uuid primary key references auth.users (id) on delete cascade,
  username   text not null,
  role       text not null default 'user',
  created_at timestamptz not null default now()
);

alter table public.profiles drop constraint if exists profiles_username_format;
alter table public.profiles add  constraint profiles_username_format
  check (username ~ '^[A-Za-z0-9_]{3,24}$');
alter table public.profiles drop constraint if exists profiles_role_check;
alter table public.profiles add  constraint profiles_role_check
  check (role in ('user', 'owner'));

create unique index if not exists profiles_username_lower_key
  on public.profiles (lower(username));
create unique index if not exists profiles_single_owner_key
  on public.profiles ((true)) where role = 'owner';

alter table public.profiles enable row level security;
revoke all on public.profiles from anon, authenticated;
grant all on public.profiles to service_role;

create or replace function public.handle_new_user()
returns trigger
language plpgsql
security definer
set search_path = public
as $$
begin
  insert into public.profiles (id, username)
    values (new.id, new.raw_user_meta_data ->> 'username');
  return new;
end $$;

revoke all on function public.handle_new_user() from public, anon, authenticated;

drop trigger if exists on_auth_user_created on auth.users;
create trigger on_auth_user_created
  after insert on auth.users
  for each row execute function public.handle_new_user();

alter table public.dev_unlocks
  add column if not exists user_id uuid references public.profiles (id) on delete set null;
create index if not exists dev_unlocks_user_idx on public.dev_unlocks (user_id);

-- Старая отдельная учётная запись Owner (хеш в dev_settings) заменена
-- аккаунтом Supabase Auth с ролью owner.
delete from public.dev_settings where key = 'owner';

create or replace function public.email_for_username(p_username text)
returns text
language sql
security definer
set search_path = public, auth
as $$
  select u.email::text
    from public.profiles p
    join auth.users u on u.id = p.id
   where lower(p.username) = lower(coalesce(p_username, ''))
$$;

create or replace function public.verify_user_password(p_user_id uuid, p_password text)
returns boolean
language sql
security definer
set search_path = public, auth, extensions
as $$
  select coalesce(
    (select u.encrypted_password = crypt(p_password, u.encrypted_password)
       from auth.users u where u.id = p_user_id),
    false)
$$;

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
declare
  res jsonb;
begin
  if not exists (select 1 from public.profiles where id = p_user_id) then
    raise exception 'account not found';
  end if;
  if (select count(*) from public.dev_unlocks
       where user_id = p_user_id and status = 'pending') >= 3 then
    raise exception 'you already have 3 pending requests';
  end if;
  res := public.request_dev_unlock(p_device_id, p_name, p_note, 'web');
  if res ->> 'result' = 'created' then
    update public.dev_unlocks set user_id = p_user_id where device_id = p_device_id;
  else
    update public.dev_unlocks set user_id = p_user_id
     where device_id = p_device_id and user_id is null;
  end if;
  return res;
end $$;

revoke all on function public.email_for_username(text) from public, anon, authenticated;
revoke all on function public.verify_user_password(uuid, text) from public, anon, authenticated;
revoke all on function public.web_request_dev_unlock(uuid, text, text, text) from public, anon, authenticated;
grant execute on function public.email_for_username(text) to service_role;
grant execute on function public.verify_user_password(uuid, text) to service_role;
grant execute on function public.web_request_dev_unlock(uuid, text, text, text) to service_role;

notify pgrst, 'reload schema';
