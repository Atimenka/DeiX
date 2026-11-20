-- DeiX: обновление и улучшение базы данных (2026-11-20).
--
-- Добавляется:
--   1. site_meta   — служебные настройки сайта (версия, режим обслуживания, флаги).
--   2. audit_log   — журнал безопасных событий (вход, выдача ключей,
--                    действия Owner). Только service_role; anon и
--                    authenticated доступа не имеют.
-- 3. Дополнительные индексы для часто используемых запросов.
--
-- Скрипт идемпотентен: можно применять несколько раз без последствий.

-- ============================================
-- 1. site_meta — настройки сайта
-- ============================================
create table if not exists public.site_meta (
  key        text primary key,
  value      text not null,
  updated_at timestamptz not null default now()
);

alter table public.site_meta enable row level security;
revoke all on public.site_meta from anon, authenticated;
grant all on public.site_meta to service_role;

-- Начальные настройки (подаются service role; можно переставить later).
insert into public.site_meta (key, value) values
  ('site_version', '0.3.0-beta'),
  ('maintenance', 'false')
on conflict (key) do nothing;

-- ============================================
-- 2. audit_log — журнал безопасности
-- ============================================
-- Записываются события: вход/выход, смена пароля, 2FA, выдача dev-ключей,
-- решения по заявкам, действия Owner. Никаких IP и телеметрии — только
-- факты с хэшем actor для анонимизации (id аккаунта).
create table if not exists public.audit_log (
  id          bigint generated always as identity primary key,
  event       text not null check (char_length(event) <= 64),
  actor       uuid references public.profiles (id) on delete set null,
  detail      text not null default '' check (char_length(detail) <= 500),
  created_at  timestamptz not null default now()
);

create index if not exists audit_log_actor_idx
  on public.audit_log (actor, created_at desc);
create index if not exists audit_log_event_idx
  on public.audit_log (event, created_at desc);
create index if not exists audit_log_created_idx
  on public.audit_log (created_at desc);

alter table public.audit_log enable row level security;
revoke all on public.audit_log from anon, authenticated;
grant all on public.audit_log to service_role;

-- Утилита: записать событие (service role).
create or replace function public.log_audit(p_event text, p_actor uuid nullable, p_detail text default '')
returns void
language plpgsql
security definer
set search_path = public
as $$\
begin
  insert into public.audit_log (event, actor, detail)
  values (p_event, p_actor, p_detail);
end $$;

-- ============================================
-- 3. Дополнительные индексы
-- ============================================

-- Список устройств пользователя — по user_id.
create index if not exists device_links_user_idx
  on public.device_links (user_id);

-- Журнал ключей: по времени (очистка, аналитика).
create index if not exists dev_keys_created_idx
  on public.dev_keys (created_at desc);

-- Аккаунты: по времени регистрации.
create index if not exists profiles_created_idx
  on public.profiles (created_at desc);

-- Релизы: по времени создания.
create index if not exists os_releases_created_idx
  on public.os_releases (created_at desc);

-- Обновления: по каналу и времени.
create index if not exists os_updates_channel_created_idx
  on public.os_updates (channel, created_at desc);

-- settings и буфер: по времени обновления.
create index if not exists user_settings_updated_idx
  on public.user_settings (updated_at desc);
create index if not exists cloud_clipboard_created_idx
  on public.cloud_clipboard (created_at desc);
