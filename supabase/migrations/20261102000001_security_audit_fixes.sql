-- DeiX Security Audit Remediation
-- Fixes confirmed vulnerabilities in Supabase Backend
-- Date: 2026-11-02

-- ============================================================================
-- 1. CRITICAL: Enable RLS on roles table
-- ============================================================================
-- Previous state: RLS NOT enabled, anon/authenticated had full CRUD via ACL
-- Anonymous INSERT/UPDATE/DELETE on public.roles confirmed via public API
-- Fix: Enable RLS, create service_role policy, revoke ACL from anon/authenticated

ALTER TABLE public.roles ENABLE ROW LEVEL SECURITY;

-- service_role can manage roles (bypasses RLS anyway, but policy for clarity)
CREATE POLICY "service can manage roles" ON public.roles
  FOR ALL TO service_role
  USING (true)
  WITH CHECK (true);

-- No policies for anon or authenticated → they get NO access
-- Revoke ACL grants for defense in depth
REVOKE ALL ON TABLE public.roles FROM anon, authenticated;

-- ============================================================================
-- 2. CRITICAL: Revoke anon access to claim_dev_unlock
-- ============================================================================
-- Previous state: claim_dev_unlock callable by anon, returns unlock_key
-- Vulnerability: Anyone who knows a device_id with approved request can steal unlock_key
-- Fix: Revoke anon/authenticated access, keep service_role (OS gateway uses it)

REVOKE ALL ON FUNCTION public.claim_dev_unlock(text) FROM anon, authenticated;

-- ============================================================================
-- 3. HIGH: Revoke anon access to request_dev_unlock
-- ============================================================================
-- Previous state: request_dev_unlock callable by anon
-- Vulnerability: Anyone can submit requests for linked devices
-- Fix: Revoke anon/authenticated access, keep service_role

REVOKE ALL ON FUNCTION public.request_dev_unlock(text, text, text, text) FROM anon, authenticated;

-- ============================================================================
-- 4. Defense in depth: Add x-device-id header check in claim_dev_unlock
-- ============================================================================
-- Prevents abuse if OS gateway (Vercel function) is compromised
-- Only the device itself can claim its unlock key

CREATE OR REPLACE FUNCTION public.claim_dev_unlock(p_device_id text)
 RETURNS jsonb
 LANGUAGE plpgsql
 SECURITY DEFINER
 SET search_path TO 'pg_catalog', 'extensions', 'pg_temp'
AS $function$
declare
  cur public.dev_unlocks;
  caller_device text := current_setting('request.headers.x-device-id', true);
begin
  -- Verify caller is the device itself
  if caller_device is null or caller_device != p_device_id then
    raise exception 'device identity mismatch';
  end if;

  select * into cur from public.dev_unlocks where device_id = coalesce(p_device_id, '') for update;
  if not found then
    return jsonb_build_object('status', 'none');
  end if;
  if cur.status = 'approved' then
    update public.dev_unlocks
       set status = 'consumed', unlock_key = null, consumed_at = now()
     where device_id = cur.device_id;
    return jsonb_build_object('status', 'approved', 'unlock_key', cur.unlock_key);
  end if;
  return jsonb_build_object('status', cur.status);
end $function$;

-- ============================================================================
-- 5. Defense in depth: Add x-device-id header check in request_dev_unlock
-- ============================================================================
-- Only the device itself can submit a request

CREATE OR REPLACE FUNCTION public.request_dev_unlock(p_device_id text, p_hostname text DEFAULT NULL::text, p_note text DEFAULT NULL::text, p_source text DEFAULT 'os'::text)
 RETURNS jsonb
 LANGUAGE plpgsql
 SECURITY DEFINER
 SET search_path TO 'pg_catalog', 'extensions', 'pg_temp'
AS $function$
declare
  cur      public.dev_unlocks;
  owner_id uuid;
  is_open  boolean;
  existed  boolean;
  caller_device text := current_setting('request.headers.x-device-id', true);
begin
  -- Verify caller is the device itself
  if caller_device is null or caller_device != p_device_id then
    raise exception 'device identity mismatch';
  end if;

  p_device_id := coalesce(p_device_id, '');
  if p_device_id !~ '^[0-9a-f]{32}$' then
    raise exception 'device id must be 32 lowercase hex characters';
  end if;
  if p_source is null or p_source not in ('os', 'web') then
    raise exception 'unknown request source';
  end if;
  p_hostname := left(nullif(btrim(p_hostname), ''), 64);
  p_note     := left(nullif(btrim(p_note), ''), 280);

  select user_id into owner_id from public.device_links where device_id = p_device_id;
  if owner_id is null then
    raise exception 'device is not linked to an account: run ''dev link'' in DeiX';
  end if;

  select * into cur from public.dev_unlocks where device_id = p_device_id for update;
  existed := found;
  if existed and cur.status <> 'consumed' then
    return jsonb_build_object('result', 'exists', 'status', cur.status);
  end if;

  select (value = 'true'::jsonb) into is_open from public.dev_settings where key = 'requests_open';
  if not coalesce(is_open, true) then
    raise exception 'the project owner is not accepting dev requests right now';
  end if;

  if (select count(*) from public.dev_unlocks
       where user_id = owner_id and status = 'pending'
         and device_id <> p_device_id) >= 3 then
    raise exception 'this account already has 3 pending requests';
  end if;

  if existed then
    update public.dev_unlocks
       set status = 'pending', unlock_key = null, consumed_at = null,
           decided_at = null, decided_by = null, created_at = now(),
           hostname = coalesce(p_hostname, hostname),
           note = coalesce(p_note, note), source = p_source, user_id = owner_id
     where device_id = p_device_id;
  else
    if (select count(*) from public.dev_unlocks where status = 'pending') >= 500 then
      raise exception 'too many pending requests, try again later';
    end if;
    insert into public.dev_unlocks (device_id, hostname, note, source, user_id)
      values (p_device_id, p_hostname, p_note, p_source, owner_id);
  end if;
  return jsonb_build_object('result', 'created', 'status', 'pending');
end $function$;

-- ============================================================================
-- 6. HIGH: Add rate limiting to begin_device_link
-- ============================================================================
-- Previous state: begin_device_link repeats same code, no rate limit
-- Fix: Limit to 3 new codes per device_id per hour

CREATE OR REPLACE FUNCTION public.begin_device_link(p_device_id text)
 RETURNS jsonb
 LANGUAGE plpgsql
 SECURITY DEFINER
 SET search_path TO 'pg_catalog', 'extensions', 'pg_temp'
AS $function$
declare
  cur      public.device_links;
  alphabet constant text := 'ABCDEFGHJKLMNPQRSTUVWXYZ23456789';
  fresh    text;
  attempt  int := 0;
  recent_count int;
begin
  p_device_id := coalesce(p_device_id, '');
  if p_device_id !~ '^[0-9a-f]{32}$' then
    raise exception 'device id must be 32 lowercase hex characters';
  end if;

  -- Rate limit: max 3 new codes per device per hour
  select count(*) into recent_count
  from public.device_links
  where device_id = p_device_id
    and code_expires_at > now() - interval '1 hour';
  if recent_count >= 3 then
    raise exception 'too many link codes for this device, try again later';
  end if;

  delete from public.device_links
   where user_id is null and code_expires_at < now() - interval '1 hour';

  select * into cur from public.device_links where device_id = p_device_id for update;
  if found and cur.user_id is not null then
    return jsonb_build_object(
      'status', 'linked',
      'username', (select username from public.profiles where id = cur.user_id));
  end if;
  if found and cur.code_expires_at > now() then
    return jsonb_build_object(
      'status', 'code',
      'code', substr(cur.code, 1, 4) || '-' || substr(cur.code, 5),
      'expires_in', greatest(0, extract(epoch from cur.code_expires_at - now())::int));
  end if;

  if not found
     and (select count(*) from public.device_links where user_id is null) >= 2000 then
    raise exception 'too many pending device links, try again later';
  end if;

  loop
    fresh := '';
    for i in 1..8 loop
      fresh := fresh || substr(alphabet, 1 + (get_byte(gen_random_bytes(1), 0) % 32), 1);
    end loop;
    begin
      insert into public.device_links (device_id, code, code_expires_at)
        values (p_device_id, fresh, now() + interval '10 minutes')
        on conflict (device_id) do update
          set code = excluded.code, code_expires_at = excluded.code_expires_at;
      exit;
    exception when unique_violation then
      attempt := attempt + 1;
      if attempt > 5 then
        raise exception 'could not allocate a link code, try again';
      end if;
    end;
  end loop;

  return jsonb_build_object(
    'status', 'code',
    'code', substr(fresh, 1, 4) || '-' || substr(fresh, 5),
    'expires_in', 600);
end $function$;

-- ============================================================================
-- 7. Defense in depth: Add RLS policies for profiles table
-- ============================================================================
-- Currently: RLS enabled, no policies, anon/authenticated have no ACL grants
-- This is safe but we add policies for defense in depth

-- Authenticated users can read their own profile
CREATE POLICY "users can read own profile" ON public.profiles
  FOR SELECT TO authenticated
  USING (id = auth.uid());

-- Authenticated users can update their own profile (but NOT role field)
-- Use a trigger to prevent role changes through direct UPDATE
CREATE POLICY "users can update own profile" ON public.profiles
  FOR UPDATE TO authenticated
  USING (id = auth.uid())
  WITH CHECK (id = auth.uid() AND (role IS NULL OR role = old.role));

-- service_role can manage all profiles
CREATE POLICY "service can manage profiles" ON public.profiles
  FOR ALL TO service_role
  USING (true)
  WITH CHECK (true);

-- ============================================================================
-- 8. Defense in depth: Add RLS policies for dev_keys table
-- ============================================================================
-- Currently: RLS enabled, no policies, anon/authenticated have no ACL grants

CREATE POLICY "users can read own keys" ON public.dev_keys
  FOR SELECT TO authenticated
  USING (user_id = auth.uid());

CREATE POLICY "service can manage keys" ON public.dev_keys
  FOR ALL TO service_role
  USING (true)
  WITH CHECK (true);

-- ============================================================================
-- 9. Defense in depth: Add RLS policies for device_links table
-- ============================================================================

CREATE POLICY "users can read own devices" ON public.device_links
  FOR SELECT TO authenticated
  USING (user_id = auth.uid() OR (user_id IS NULL AND device_id = current_setting('request.headers.x-device-id', true)));

CREATE POLICY "users can link own devices" ON public.device_links
  FOR INSERT TO authenticated
  WITH CHECK (user_id = auth.uid());

CREATE POLICY "users can unlink own devices" ON public.device_links
  FOR DELETE TO authenticated
  USING (user_id = auth.uid());

CREATE POLICY "service can manage devices" ON public.device_links
  FOR ALL TO service_role
  USING (true)
  WITH CHECK (true);

-- ============================================================================
-- 10. Defense in depth: Add RLS policies for mfa_backup_codes
-- ============================================================================

CREATE POLICY "users can read own backup codes" ON public.mfa_backup_codes
  FOR SELECT TO authenticated
  USING (user_id = auth.uid());

CREATE POLICY "service can manage backup codes" ON public.mfa_backup_codes
  FOR ALL TO service_role
  USING (true)
  WITH CHECK (true);

-- ============================================================================
-- 11. Defense in depth: Add RLS policies for cloud_clipboard
-- ============================================================================

CREATE POLICY "users can read own clipboard" ON public.cloud_clipboard
  FOR SELECT TO authenticated
  USING (user_id = auth.uid());

CREATE POLICY "users can write own clipboard" ON public.cloud_clipboard
  FOR ALL TO authenticated
  USING (user_id = auth.uid())
  WITH CHECK (user_id = auth.uid());

CREATE POLICY "service can manage clipboard" ON public.cloud_clipboard
  FOR ALL TO service_role
  USING (true)
  WITH CHECK (true);

-- ============================================================================
-- 12. Defense in depth: Add RLS policies for user_settings
-- ============================================================================

CREATE POLICY "users can read own settings" ON public.user_settings
  FOR SELECT TO authenticated
  USING (user_id = auth.uid());

CREATE POLICY "users can write own settings" ON public.user_settings
  FOR ALL TO authenticated
  USING (user_id = auth.uid())
  WITH CHECK (user_id = auth.uid());

CREATE POLICY "service can manage settings" ON public.user_settings
  FOR ALL TO service_role
  USING (true)
  WITH CHECK (true);

-- ============================================================================
-- 13. Defense in depth: Add RLS policies for dev_settings
-- ============================================================================

CREATE POLICY "authenticated can read settings" ON public.dev_settings
  FOR SELECT TO authenticated
  USING (true);

CREATE POLICY "service can manage settings" ON public.dev_settings
  FOR ALL TO service_role
  USING (true)
  WITH CHECK (true);

-- ============================================================================
-- 14. Defense in depth: Add RLS policies for email_verifications
-- ============================================================================

CREATE POLICY "users can read own verifications" ON public.email_verifications
  FOR SELECT TO authenticated
  USING (user_id = auth.uid());

CREATE POLICY "service can manage verifications" ON public.email_verifications
  FOR ALL TO service_role
  USING (true)
  WITH CHECK (true);

-- ============================================================================
-- 15. Defense in depth: Add RLS policies for os_releases and os_updates
-- ============================================================================

CREATE POLICY "authenticated can read releases" ON public.os_releases
  FOR SELECT TO authenticated
  USING (ready = true AND published = true);

CREATE POLICY "service can manage releases" ON public.os_releases
  FOR ALL TO service_role
  USING (true)
  WITH CHECK (true);

CREATE POLICY "authenticated can read updates" ON public.os_updates
  FOR SELECT TO authenticated
  USING (ready = true AND published = true);

CREATE POLICY "service can manage updates" ON public.os_updates
  FOR ALL TO service_role
  USING (true)
  WITH CHECK (true);

-- ============================================================================
-- 16. Defense in depth: Add RLS policies for role_rules
-- ============================================================================
-- Currently: RLS enabled, only service_role policy
-- Revoke ACL from anon/authenticated for defense in depth

REVOKE ALL ON TABLE public.role_rules FROM anon, authenticated;

-- ============================================================================
-- 17. Fix: Revoke anon/authenticated ACL on user_roles (defense in depth)
-- ============================================================================
-- Currently: RLS enabled with proper policies
-- Revoke ACL for defense in depth

REVOKE ALL ON TABLE public.user_roles FROM anon, authenticated;

-- ============================================================================
-- 18. Ensure service_role retains access to all tables
-- ============================================================================
-- service_role bypasses RLS, so it can access all tables regardless of policies
-- No action needed, but we verify the grants are in place

GRANT ALL ON TABLE public.roles TO service_role;
GRANT ALL ON TABLE public.profiles TO service_role;
GRANT ALL ON TABLE public.user_roles TO service_role;
GRANT ALL ON TABLE public.role_rules TO service_role;
GRANT ALL ON TABLE public.dev_keys TO service_role;
GRANT ALL ON TABLE public.device_links TO service_role;
GRANT ALL ON TABLE public.mfa_backup_codes TO service_role;
GRANT ALL ON TABLE public.cloud_clipboard TO service_role;
GRANT ALL ON TABLE public.user_settings TO service_role;
GRANT ALL ON TABLE public.dev_settings TO service_role;
GRANT ALL ON TABLE public.email_verifications TO service_role;
GRANT ALL ON TABLE public.os_releases TO service_role;
GRANT ALL ON TABLE public.os_updates TO service_role;
GRANT ALL ON TABLE public.dev_unlocks TO service_role;
GRANT ALL ON TABLE public.dev_mode TO service_role;
GRANT ALL ON TABLE public.feature_flags TO service_role;

-- Grant function execute to service_role for all RPC functions
GRANT ALL ON FUNCTION public.claim_dev_unlock(text) TO service_role;
GRANT ALL ON FUNCTION public.request_dev_unlock(text, text, text, text) TO service_role;
GRANT ALL ON FUNCTION public.begin_device_link(text) TO service_role;
GRANT ALL ON FUNCTION public.device_link_status(text) TO service_role;
GRANT ALL ON FUNCTION public.approve_dev_unlock(text, text) TO service_role;
GRANT ALL ON FUNCTION public.deny_dev_unlock(text, text) TO service_role;
GRANT ALL ON FUNCTION public.link_device(uuid, text) TO service_role;
GRANT ALL ON FUNCTION public.unlink_device(uuid, text) TO service_role;
GRANT ALL ON FUNCTION public.email_for_username(text) TO service_role;
GRANT ALL ON FUNCTION public.verify_user_password(uuid, text) TO service_role;
GRANT ALL ON FUNCTION public.save_user_settings(uuid, jsonb) TO service_role;
GRANT ALL ON FUNCTION public.get_user_settings(uuid) TO service_role;
GRANT ALL ON FUNCTION public.save_clipboard(uuid, text) TO service_role;
GRANT ALL ON FUNCTION public.get_clipboard(uuid) TO service_role;
GRANT ALL ON FUNCTION public.web_request_dev_unlock(uuid, text, text, text) TO service_role;

-- ============================================================================
-- 19. Trigger: Prevent role changes through direct UPDATE on profiles
-- ============================================================================
-- Only service_role (backend) can change roles. Direct UPDATE by authenticated
-- users is blocked for the role field.

CREATE OR REPLACE FUNCTION public.prevent_role_change() RETURNS trigger
 LANGUAGE plpgsql SECURITY DEFINER SET search_path TO 'public'
AS $function$
BEGIN
  IF NEW.role IS DISTINCT FROM OLD.role AND auth.role() != 'service_role' THEN
    RAISE EXCEPTION 'role changes are not allowed through direct updates';
  END IF;
  RETURN NEW;
END;
$function$;

DROP TRIGGER IF EXISTS prevent_role_change ON public.profiles;
CREATE TRIGGER prevent_role_change BEFORE UPDATE ON public.profiles
  FOR EACH ROW EXECUTE FUNCTION public.prevent_role_change();
