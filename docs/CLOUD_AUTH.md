# Cloud sign-in (Supabase)

DeiX OS can sign you in with the same account you use on the DeiX
website (email + password, optional 2FA). The first online sign-in is
cached locally; later sign-ins work offline.

## How it works

```
boot screen → [LOCAL] [CLOUD] → email + password
                        │
                        ▼
              https://<site>/api/os?fn=cloud_login
                        │
              (gateway on Vercel, holds Supabase service key)
                        │
                        ▼
              GoTrue password grant (anon key)
                        │
              ┌─────────┴─────────┐
              ▼                   ▼
        no 2FA              verified TOTP factor
              │                   │
              ▼                   ▼
    {ok, username,      {totp_required, factor_id,
      user_id}           access_token}
              │                   │
              ▼                   ▼
       cache + done       6-digit code → cloud_verify
                                    │
                                    ▼
                          challenge + verify → aal2
                                    │
                                    ▼
                            {ok, username, user_id}
```

- **Gateway, not direct.** The OS talks to the site's `/api/os`
  endpoint (same as `dev request`), not to `supabase.co` directly —
  some providers block `supabase.co` by SNI. The gateway calls
  GoTrue with the anon key and reads `profiles` with the service key
  (RLS on `profiles` does not allow anon reads).
- **Password grant.** GoTrue's `POST /auth/v1/token?grant_type=password`
  always returns a token (AAL1) for valid credentials, even for
  accounts with 2FA. The gateway checks `/auth/v1/user` for a
  `verified` TOTP factor:
  - no factor → sign-in complete, return username + user_id
  - factor present → return `totp_required` + factor_id + access_token
- **2FA verification.** The OS asks for a 6-digit code from an
  authenticator app. The gateway calls `POST /auth/v1/factors/{id}/challenge`
  then `POST /auth/v1/factors/{id}/verify` with the code. A valid
  code returns an AAL2 session; the username comes from `profiles`.
- **Challenge placement.** Challenge is created in the verify phase
  (not in the login phase), so it does not expire before the user
  enters the code.
- **Username.** Comes from `public.profiles.username` (set at
  registration on the website). Sanitised to `[A-Za-z0-9_-]`, max
  32 chars. If the username is taken by an unlinked local account,
  a `_c<n>` suffix is added — a cloud account never overwrites an
  existing local account.
- **Password sync.** After a successful cloud sign-in, the OS
  writes the cloud password to USERS.DB via `auth::force_set_password`
  (no old-password check, no LUKS slot changes). This means a
  password change on the website propagates to the OS on the next
  cloud sign-in.

## Disk encryption

The disk (XTS-AES-256) is encrypted with the **local** account
password (set during first-run setup). The cloud password is a
separate credential. After a cloud sign-in on an encrypted disk:

1. `cloud_disk_status()` tries to add the cloud password to LUKS
   (`luks::add_password_with_master`). This works only if the disk
   is already unlocked (e.g. cloud password was added in a previous
   sign-in).
2. If that fails, it tries `try_unlock(cloud_password)` — works if
   the cloud password is already in LUKS from a previous sign-in.
3. If both fail (first cloud sign-in, disk still locked with the
   local password), the OS shows "cloud verified - enter disk
   password". The user enters the **local** disk password. On
   success, the cloud password is added to LUKS and both passwords
   unlock the disk.

On unencrypted disks no disk password is needed.

## Caching

After the first successful online sign-in:

- USERS.DB gets a record `username:salt:hash` with the cloud
  password (force-set).
- `/userdata/etc/cloud_link` stores `user_id:email:username`.

Later boots can sign in locally with the cached username + password
(no network needed). `cloud status` shows the link; `cloud unlink`
removes it (the local account stays).

## Honest boundaries (§42)

- This is a **sign-in convenience**, not a security boundary. The
  anon key is public by design; the gateway has the same access
  level as the website.
- Passwords travel over TLS to the Supabase GoTrue service, same as
  the website. No additional protection beyond what the website
  provides.
- A user with access to the machine can read USERS.DB (if the disk
  is not encrypted) and the link file. Encryption of `/userdata` is
  not part of this feature.
- Rate limiting exists at the gateway (per-IP and per-device) and in
  GoTrue, but a determined attacker can still attempt passwords
  through the public API.
- Cloud sign-in does **not** replace the website's own 2FA, email
  confirmation, or owner-role enforcement.
- Registration stays on the website (Turnstile + email
  confirmation). The OS does not register accounts.

## CLI

- `cloud status` — show the cloud link (user_id, email, username) or
  "not linked".
- `cloud unlink` — remove the link file (local account stays).
