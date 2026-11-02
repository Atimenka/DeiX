-- Подтверждение почты и резервные коды двухфакторной аутентификации.
--
-- email_verified: почта подтверждена ссылкой из письма (письма шлёт сервер
-- сайта, если задан почтовый провайдер). У уже существующих аккаунтов
-- считается подтверждённой.
-- email_verifications: одноразовые токены ссылок подтверждения; хранится
-- только SHA-256 токена.
-- mfa_backup_codes: резервные коды входа вместо TOTP; хранится только
-- SHA-256 кода, использованный код получает used_at.
--
-- У anon и authenticated доступа к таблицам нет. Скрипт идемпотентен.

do $$
begin
  if not exists (
    select 1 from information_schema.columns
    where table_schema = 'public' and table_name = 'profiles' and column_name = 'email_verified'
  ) then
    alter table public.profiles add column email_verified boolean not null default false;
    update public.profiles set email_verified = true;
  end if;
end $$;

create table if not exists public.email_verifications (
  id         bigint generated always as identity primary key,
  user_id    uuid not null references public.profiles (id) on delete cascade,
  token_hash text not null unique check (token_hash ~ '^[0-9a-f]{64}$'),
  expires_at timestamptz not null,
  used_at    timestamptz,
  created_at timestamptz not null default now()
);
create index if not exists email_verifications_user_idx on public.email_verifications (user_id, created_at desc);

create table if not exists public.mfa_backup_codes (
  id         bigint generated always as identity primary key,
  user_id    uuid not null references public.profiles (id) on delete cascade,
  code_hash  text not null check (code_hash ~ '^[0-9a-f]{64}$'),
  used_at    timestamptz,
  created_at timestamptz not null default now(),
  unique (user_id, code_hash)
);

alter table public.email_verifications enable row level security;
alter table public.mfa_backup_codes enable row level security;
revoke all on public.email_verifications, public.mfa_backup_codes from anon, authenticated;
grant all on public.email_verifications, public.mfa_backup_codes to service_role;
