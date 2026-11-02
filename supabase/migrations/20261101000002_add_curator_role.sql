-- Добавление роли "curator" (ранг 60, между owner=100 и admin=50)
-- Curator может управлять ролями admin/moderator/tester/user, но не curator и не owner
-- Также создаётся таблица role_rules с правилами иерархии ролей

-- Добавляем curator в roles таблицу
INSERT INTO roles (role_name, description, rank)
VALUES ('curator', 'Куратор: управляет ролями ниже своей (admin, moderator, tester, user), но не может назначать curator или owner', 60)
ON CONFLICT (role_name) DO UPDATE SET description = EXCLUDED.description, rank = EXCLUDED.rank;

-- Создаём таблицу role_rules для описания правил иерархии
CREATE TABLE IF NOT EXISTS role_rules (
  id SERIAL PRIMARY KEY,
  description TEXT NOT NULL,
  active BOOLEAN DEFAULT true,
  created_at TIMESTAMPTZ DEFAULT now()
);

-- Разрешаем доступ через service_role
ALTER TABLE role_rules ENABLE ROW LEVEL SECURITY;
CREATE POLICY "Enable service_role access on role_rules" ON role_rules
  USING (auth.role() = 'service_role')
  WITH CHECK (auth.role() = 'service_role');

-- Добавляем правила иерархии
INSERT INTO role_rules (description) VALUES
  ('Curator может назначать роль admin, moderator, tester, user', true, now()),
  ('Curator НЕ может назначать роль curator или owner', true, now()),
  ('Owner может назначать любую роль', true, now()),
  ('Curator может снимать роли admin, moderator, tester, user', true, now()),
  ('Curator НЕ может снимать роль curator или owner', true, now());
