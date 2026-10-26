-- Роль owner получает аккаунт с именем Atimenka, пока в системе нет ни одного
-- owner. Назначение делает триггер при создании пользователя, поэтому оно
-- атомарно. Когда owner есть, имя Atimenka — обычное и роли не даёт.

create or replace function public.handle_new_user()
returns trigger
language plpgsql
security definer
set search_path to 'public'
as $$
declare
  v_username text := new.raw_user_meta_data ->> 'username';
  v_role text := 'user';
begin
  if lower(v_username) = 'atimenka' and not exists (select 1 from public.profiles where role = 'owner') then
    v_role := 'owner';
  end if;
  insert into public.profiles (id, username, role) values (new.id, v_username, v_role);
  return new;
end $$;
