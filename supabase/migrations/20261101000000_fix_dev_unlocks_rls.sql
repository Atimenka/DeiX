-- Фиккс RLS: unlock_key не должен быть readable без device_id.
-- Раньше policy SELECT была using (true) — любой с anon-ключом
-- мог читать ВСЕ строки включая unlock_key.
-- Теперь: SELECT требует x-device-id header, который совпадает с
-- device_id строки. ОС отправляет свой device_id в этом заголовке.

-- Отзываем старую политику SELECT.
drop policy if exists "os can read status" on public.dev_unlocks;

-- Новая политика: читать можно только свою строку.
-- current_setting('request.headers.x-device-id', true) — PostgREST
-- прокидывает заголовки запроса в Postgres settings. Если заголовка
-- нет (NULL) — строка не видна (protection от прямого SQL без заголовка).
create policy "os can read own status"
  on public.dev_unlocks
  for select
  to anon
  using (
    device_id = current_setting('request.headers.x-device-id', true)
  );

-- INSERT-политика без изменений: подать заявку может любой.
