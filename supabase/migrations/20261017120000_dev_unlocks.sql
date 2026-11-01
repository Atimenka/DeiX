-- Активация dev-режима DeiX: серверный реестр устройств.
--
-- Поток:
--   1. ОС командой `dev request` создаёт заявку (status = 'pending').
--   2. Администратор одобряет её в дашборде (см. docs/DEVAUTH.md):
--      ставит status = 'approved' и генерирует unlock_key.
--   3. ОС командой `dev on` читает свою строку; при 'approved'
--      сохраняет unlock_key в /userdata и включает dev-режим.
--
-- ОС ходит сюда ТОЛЬКО через PostgREST с anon-ключом; права anon
-- ограничены политиками ниже: подать заявку и прочитать статус.
-- Одобрять/менять строки anon не может — это делает только владелец
-- проекта из дашборда (service role).

create table public.dev_unlocks (
  device_id    text primary key,
  hostname     text,
  status       text not null default 'pending'
               check (status in ('pending', 'approved', 'denied')),
  unlock_key   text,
  requested_at timestamptz not null default now(),
  decided_at   timestamptz
);

alter table public.dev_unlocks enable row level security;

-- ОС может подать заявку: только pending и без ключа.
create policy "os can request"
  on public.dev_unlocks
  for insert
  to anon
  with check (status = 'pending' and unlock_key is null);

-- ОС может читать строки (запросы фильтруют по своему device_id).
-- ВНИМАНИЕ: anon-ключ публичный, поэтому перечислить заявки может
-- любой, у кого он есть. Для реестра хобби-ОС это осознанный компромисс
-- (см. docs/DEVAUTH.md, раздел «Честные границы»).
create policy "os can read status"
  on public.dev_unlocks
  for select
  to anon
  using (true);

-- Политик INSERT/UPDATE/DELETE для одобрения нет намеренно:
-- status и unlock_key меняются только из дашборда (service role
-- обходит RLS).
