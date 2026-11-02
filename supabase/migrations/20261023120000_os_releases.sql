-- Версии ОС для скачивания: готовые образы диска.
--
-- os_releases хранит метаданные версии; сам файл лежит в приватном бакете
-- os-images (Supabase Storage) по пути «<версия>/<имя файла>». Владелец
-- загружает файл в браузере по одноразовой подписанной ссылке, после чего
-- сервер сверяет размер объекта и помечает версию ready. Скачивание идёт
-- через сайт: он выдаёт короткоживущую подписанную ссылку и считает загрузки.
--
-- ready — файл загружен и проверен; published — версия видна на странице
-- «Скачать». У anon и authenticated доступа к таблице нет. Скрипт идемпотентен.

create table if not exists public.os_releases (
  id           uuid primary key default gen_random_uuid(),
  version      text not null unique check (version ~ '^[0-9A-Za-z][0-9A-Za-z._-]{0,31}$'),
  title        text not null default '' check (char_length(title) <= 120),
  notes        text not null default '' check (char_length(notes) <= 4000),
  file_path    text not null,
  file_name    text not null,
  size_bytes   bigint not null check (size_bytes > 0 and size_bytes <= 52428800),
  sha256       text not null check (sha256 ~ '^[0-9a-f]{64}$'),
  ready        boolean not null default false,
  published    boolean not null default false,
  downloads    bigint not null default 0,
  created_by   text,
  created_at   timestamptz not null default now(),
  published_at timestamptz
);

create index if not exists os_releases_public_idx on public.os_releases (published, ready, published_at desc);

alter table public.os_releases enable row level security;
revoke all on public.os_releases from anon, authenticated;
grant all on public.os_releases to service_role;

create or replace function public.bump_release_downloads(p_id uuid)
returns void
language sql
security definer
set search_path to 'public'
as $$
  update public.os_releases set downloads = downloads + 1 where id = p_id;
$$;

revoke all on function public.bump_release_downloads(uuid) from public, anon, authenticated;
grant execute on function public.bump_release_downloads(uuid) to service_role;

-- Приватный бакет: файлы отдаются только по подписанным ссылкам сайта.
-- 50 МиБ — предел загрузки на бесплатном тарифе Supabase.
insert into storage.buckets (id, name, public, file_size_limit)
values ('os-images', 'os-images', false, 52428800)
on conflict (id) do update set public = false, file_size_limit = 52428800;
