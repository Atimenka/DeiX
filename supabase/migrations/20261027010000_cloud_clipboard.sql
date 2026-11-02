-- Общий буфер обмена между устройствами DeiX.
--
-- Поток:
--   1. В DeiX: `cloud clip set "текст"` — отправляет текст на сервер.
--   2. На другом устройстве: `cloud clip get` — забирает текст и
--      выводит его в терминал.
--   3. На сайте: вкладка «Синхронизация» показывает текущий буфер
--      и позволяет задать текст из браузера.
--
-- Только один активный буфер на пользователя (последний set).
-- У anon и authenticated доступа к таблице нет. Скрипт идемпотентен.

create table if not exists public.cloud_clipboard (
  id        uuid primary key default gen_random_uuid(),
  user_id   uuid not null references public.profiles (id) on delete cascade,
  content   text not null,
  created_at timestamptz not null default now(),
  unique (user_id)
);

create index if not exists cloud_clipboard_user_idx on public.cloud_clipboard (user_id);

alter table public.cloud_clipboard enable row level security;
revoke all on public.cloud_clipboard from anon, authenticated;
grant all on public.cloud_clipboard to service_role;

create or replace function public.save_clipboard(p_user_id uuid, p_content text)
returns jsonb
language plpgsql
security definer
set search_path = public
as $$
begin
  insert into public.cloud_clipboard (user_id, content, created_at)
  values (p_user_id, p_content, now())
  on conflict (user_id) do update
    set content = excluded.content, created_at = now();
  return jsonb_build_object('result', 'ok', 'created_at', to_jsonb(now()));
end $$;

create or replace function public.get_clipboard(p_user_id uuid)
returns jsonb
language plpgsql
security definer
set search_path = public
as $$
declare
  row public.cloud_clipboard;
begin
  select * into row from public.cloud_clipboard where user_id = p_user_id;
  if not found then
    return jsonb_build_object('result', 'none');
  end if;
  return jsonb_build_object('result', 'ok', 'content', row.content, 'created_at', to_jsonb(row.created_at));
end $$;

revoke all on function public.save_clipboard(uuid, text) from public, anon, authenticated;
revoke all on function public.get_clipboard(uuid) from public, anon, authenticated;
grant execute on function public.save_clipboard(uuid, text) to service_role;
grant execute on function public.get_clipboard(uuid) to service_role;

notify pgrst, 'reload schema';
