-- Синхронизация настроек рабочего стола DeiX между устройствами.
--
-- Поток:
--   1. ОС команду `cloud sync` отправляет текущие настройки
--      (тема, акцент, стиль обоев, профиль производительности, язык)
--      на шлюз сайта; шлюз записывает их в user_settings.
--   2. На новом устройстве ОС команду `cloud pull` забирает настройки
--      из user_settings и применяет их локально.
--   3. Сайт показывает синхронизированные настройки в аккаунте.
--
-- User_settings привязан к аккаунту (profiles.id), а не к устройству:
-- настройки следуют за пользователем, а не за машиной.
--
-- У anon и authenticated доступа к таблице нет. Скрипт идемпотентен.

create table if not exists public.user_settings (
  id        uuid primary key default gen_random_uuid(),
  user_id   uuid not null references public.profiles (id) on delete cascade,
  settings  jsonb not null,
  updated_at timestamptz not null default now(),
  unique (user_id)
);

create index if not exists user_settings_user_idx on public.user_settings (user_id);

alter table public.user_settings enable row level security;
revoke all on public.user_settings from anon, authenticated;
grant all on public.user_settings to service_role;

-- Синоним для backward compatibility с older migrations.
create or replace function public.save_user_settings(p_user_id uuid, p_settings jsonb)
returns jsonb
language plpgsql
security definer
set search_path = public
as $$\
begin
  insert into public.user_settings (user_id, settings, updated_at)
  values (p_user_id, p_settings, now())
  on conflict (user_id) do update
    set settings = excluded.settings, updated_at = now();
  return jsonb_build_object('result', 'ok', 'updated_at', to_jsonb(now()));
end $$;

create or replace function public.get_user_settings(p_user_id uuid)
returns jsonb
language plpgsql
security definer
set search_path = public
as $$\
declare
  row public.user_settings;
begin
  select * into row from public.user_settings where user_id = p_user_id;
  if not found then
    return jsonb_build_object('result', 'none');
  end if;
  return jsonb_build_object('result', 'ok', 'settings', row.settings, 'updated_at', to_jsonb(row.updated_at));
end $$;

revoke all on function public.save_user_settings(uuid, jsonb) from public, anon, authenticated;
revoke all on function public.get_user_settings(uuid) from public, anon, authenticated;
grant execute on function public.save_user_settings(uuid, jsonb) to service_role;
grant execute on function public.get_user_settings(uuid) to service_role;

notify pgrst, 'reload schema';
