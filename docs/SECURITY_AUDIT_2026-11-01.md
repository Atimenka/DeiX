# DeiX Security Audit Report

**Date:** 2026-11-01
**Branch:** arena/e8a29038-deix
**Commit:** 645e803
**Scope:** src/ (kernel), supabase/migrations (database)

---

## Summary

Audited 40+ source files across authentication, encryption, networking, process management, and filesystem layers. Found and fixed 8 vulnerabilities (2 CRITICAL, 3 HIGH, 3 MEDIUM).

---

## CRITICAL

### 1. MEX API — Capability Bypass via VFS Fallback

**File:** `src/mex.rs` (`api_read_file`, `api_write_file`)
**Severity:** CRITICAL
**Impact:** Any MEX process can read or write any file on the filesystem, including USERS.DB (password hashes), device.id, dev_unlock.key, and system configuration.

**Root Cause:** When `process::open()` failed (process lacked required capability), the code fell back to `vfs::read_file()`/`vfs::write_file()` which perform no capability checks. This completely bypassed the capability-based security model.

```rust
// BEFORE — fallback bypassed capability checks
Err(_) => crate::vfs::read_file(&path),
```

**Fix:** Removed the fallback for process context. When a process has a PID, file access goes strictly through `process::open()`. The `vfs::read_file` fallback remains only for CLI context (no PID), where the caller manages access itself.

```rust
// AFTER — strict capability enforcement for processes
let result = match crate::process::current_pid() {
    Some(pid) => match crate::process::open(pid, &path, false) { ... },
    None => crate::vfs::read_file(&path), // CLI context only
};
```

---

### 2. dev_unlocks RLS — All unlock_keys Publicly Readable

**File:** `supabase/migrations/20261017120000_dev_unlocks.sql`
**Severity:** CRITICAL
**Impact:** Anyone with the public anon key (embedded in the OS binary and website) can read all `unlock_key` values and unlock any approved device.

**Root Cause:** The SELECT RLS policy used `using (true)`, allowing any anon user to read ALL rows including `unlock_key`:

```sql
-- BEFORE — anyone can read all rows
create policy "os can read status"
  on public.dev_unlocks
  for select
  to anon
  using (true);
```

**Fix:** New migration (`20261101000000_fix_dev_unlocks_rls.sql`) changes the policy to require `x-device-id` header matching the row's `device_id`:

```sql
-- AFTER — only the device owner can read its row
create policy "os can read own status"
  on public.dev_unlocks
  for select
  to anon
  using (device_id = current_setting('request.headers.x-device-id', true));
```

OS code updated (`src/devauth.rs`) to send `x-device-id` header in all requests to the `dev_unlocks` table.

---

## HIGH

### 3. process::open — No Capability Check for Reading /system

**File:** `src/process.rs` (`open` function)
**Severity:** HIGH
**Impact:** Any process can read files from the read-only `/system` partition, including kernel binaries, system configuration, and potentially sensitive data.

**Root Cause:** The `open()` function only checked `WRITE_USERDATA` capability for writing. For reading, it performed no capability check at all.

**Fix:** Added `READ_ANY` capability check for reading from read-only mounts (`/system`):

```rust
// AFTER — reading /system requires READ_ANY cap
if st.read_only {
    let caps = ...;
    if caps & cap::READ_ANY == 0 {
        return Err(ProcessError::Vfs(String::from(
            "нет права чтения (cap READ_ANY требуется для /system)",
        )));
    }
}
```

---

### 4. DNS — Predictable Source Ports (Cache Poisoning)

**File:** `src/net/dns.rs` (`resolve_a` function)
**Severity:** HIGH
**Impact:** Network attacker can poison the DNS cache by predicting query source ports and injecting spoofed responses.

**Root Cause:** Source port was `49152 + (seq & 0x3FFF)`, where `seq` is a simple incrementing counter. An attacker observing one query could predict all future ports.

**Fix:** Source port now uses hardware RNG:

```rust
// AFTER — random port via hardware RNG
let mut rnd = [0u8; 2];
crate::rng::fill_random(&mut rnd);
let sport = 49152 + (((rnd[0] as u16) << 8 | rnd[1] as u16) & 0x3FFF);
```

---

### 5. sys_getrandom — Predictable PRNG

**File:** `src/linux/syscall.rs` (`sys_getrandom` function)
**Severity:** HIGH
**Impact:** Applications using `getrandom()` for cryptographic keys would generate predictable keys, allowing key recovery by an attacker who knows the approximate system uptime.

**Root Cause:** Used an LCG (linear congruential generator) seeded from `uptime_ms()`:

```rust
// BEFORE — predictable LCG
let mut state = crate::timer::uptime_ms().wrapping_mul(6364136223846793005).wrapping_add(1);
for i in 0..len {
    state ^= state << 13;
    state ^= state >> 7;
    state ^= state << 17;
    // ...
}
```

**Fix:** Uses the kernel's cryptographic RNG (RDRAND + jitter fallback):

```rust
// AFTER — cryptographic RNG
crate::rng::fill_random(&mut chunk);
```

---

## MEDIUM

### 6. TLS open_record — Zero-Stripping Data Corruption

**File:** `src/net/tls.rs` (`open_record` function)
**Severity:** MEDIUM
**Impact:** Application data ending with zero bytes is silently corrupted. A malicious TLS server could exploit this to manipulate the content type byte.

**Root Cause:** The code stripped trailing zero bytes from the plaintext before extracting the content type. This is NOT part of the TLS 1.3 specification — the content type is always the last byte of the inner plaintext:

```rust
// BEFORE — strips zero bytes from data
while plain.last() == Some(&0) {
    plain.pop();
}
let itype = plain.pop()...;
```

**Fix:** Removed the zero-stripping loop:

```rust
// AFTER — content type is always the last byte
let itype = plain.pop()...;
```

---

### 7. VFS normalize — Path Traversal via `..`

**File:** `src/vfs.rs` (`normalize` function)
**Severity:** MEDIUM
**Impact:** Paths containing `..` could traverse directory structure within a volume, potentially accessing files outside intended scope.

**Root Cause:** The `normalize()` function did not handle `..` path components. While ext2's `..` directory entry limits traversal to within the volume (root's parent is root), it allowed bypassing directory-level access restrictions.

**Fix:** Added `..` handling that pops the last component:

```rust
// AFTER — '..' pops last component, cannot escape root
if part == ".." {
    parts.pop();
    continue;
}
```

---

### 8. Login Screen — No Rate Limiting

**File:** `src/auth.rs` (`run_login_screen` function)
**Severity:** MEDIUM
**Impact:** Brute-force password attacks are feasible. Combined with weak SHA-256 password hashing (single round, acknowledged in code comments), offline and online brute-force are both practical.

**Root Cause:** The login loop had no attempt counter:

**Fix:** Added 5-attempt limit with 30-second delay and CPU halt:

```rust
// AFTER — 5 attempts, then halt
if attempts >= max_attempts {
    println!("Too many failed attempts. System halting in 30 seconds.");
    let deadline = crate::timer::uptime_ms() + 30_000;
    while crate::timer::uptime_ms() < deadline {
        unsafe { core::arch::asm!("hlt") };
    }
    unsafe { core::arch::asm!("cli; hlt"); }
    return String::new();
}
```

---

## Lower-Severity Findings (Not Fixed)

### x509 — No keyUsage/extendedKeyUsage Check
**File:** `src/net/x509.rs` (`verify_chain` function)
**Impact:** A CA certificate with `keyUsage=certSign` could be used as a leaf certificate. Per RFC 5280, leaf certificates should have `keyUsage=digitalSignature,keyEncipherment` and `extendedKeyUsage=serverAuth`.
**Rationale for not fixing:** Adding these checks could break compatibility with some certificate chains. The current implementation is conservative (rejects unknown algorithms) but doesn't enforce strict CA/leaf role separation.

### No DNSSEC
**File:** `src/net/dns.rs`
**Impact:** DNS responses are not cryptographically authenticated. Network attacker can spoof DNS responses.
**Rationale for not fixing:** DNSSEC implementation is a significant undertaking. TLS certificate validation limits the practical impact — an attacker would need a valid certificate for the target host.

### No Certificate Revocation (OCSP/CRL)
**File:** `src/net/x509.rs`, `src/net/tls.rs`
**Impact:** Compromised certificates remain valid until expiry.
**Rationale for not fixing:** Acknowledged in code comments as a deliberate trade-off. Implementing OCSP/CRL would require significant infrastructure.

### save_users — Non-Atomic Write
**File:** `src/auth.rs` (`save_users` function)
**Impact:** System crash during USERS.DB rewrite could corrupt the database.
**Rationale for not fixing:** ext2 journaling could be used, but this is a low-severity issue for a single-user hobby OS.

---

## Files Changed

| File | Change |
|------|--------|
| `src/mex.rs` | Removed capability bypass in api_read_file/api_write_file |
| `src/process.rs` | Added READ_ANY cap check for reading /system |
| `src/vfs.rs` | Fixed path traversal in normalize() |
| `src/net/tls.rs` | Removed zero-stripping in open_record |
| `src/devauth.rs` | Send x-device-id header in all dev_unlocks requests |
| `supabase/migrations/20261101000000_fix_dev_unlocks_rls.sql` | New migration fixing RLS SELECT policy |
| `src/linux/syscall.rs` | Fixed getrandom to use cryptographic RNG |
| `src/auth.rs` | Added login rate limiting (5 attempts + halt) |
| `src/net/dns.rs` | Fixed predictable DNS source ports |

## Commit

```
645e803 security: fix 8 vulnerabilities found in audit
```

## How to Apply

1. **SQL migration:** Run `20261101000000_fix_dev_unlocks_rls.sql` on the Supabase project:
   ```bash
   # Via Supabase CLI or dashboard SQL editor
   supabase db push
   ```

2. **Code changes:** Build from source:
   ```bash
   ./build.sh
   ```

3. **Merge:** The local branch `arena/e8a29038-deix` has unrelated history with the remote. Merge manually:
   ```bash
   git fetch origin
   git merge origin/arena/e8a29038-deix  # or cherry-pick specific commits
   git push
   ```
