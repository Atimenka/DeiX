'use strict';

const { DEVICE_ID_RE, HttpError, clientIp, overLimit, readBody, recordHit, rpc, send, sb, auth, verifiedTotp } = require('./_lib');

/**
 * Gateway for the OS: DeiX talks here, not to supabase.co directly
 * (some providers block it by SNI). Whitelisted dev-unlock functions
 * and cloud sign-in; everything else is rejected. Responses mirror the
 * PostgREST format the OS parses: success — jsonb function, rejection —
 * {"message": ...} with a 4xx/5xx code.
 */

// cloud_login(p_email, p_password) — sign in with email + password.
// The anon key calls GoTrue password grant. If the account has 2FA
// (verified TOTP factor in /auth/v1/user) — challenge is NOT created
// here; we reply {status:"totp_required", factor_id, access_token},
// challenge is created in the verify phase so it doesn't expire before
// the code is entered. Otherwise the profile is read via the service
// key (RLS on profiles does not allow anon reads) and we reply
// {status:"ok", username, user_id}.
async function cloudLogin(body) {
  const email = String(body.p_email || '').trim().toLowerCase();
  const password = String(body.p_password || '');
  if (!email || !password) throw new HttpError(400, 'Email and password required');
  const grant = await auth('token?grant_type=password', { body: { email, password } });
  if (grant.status !== 200 || !grant.data || !grant.data.access_token) {
    return { status: 400, message: 'invalid email or password' };
  }
  const token = grant.data.access_token;
  const me = await auth('user', { method: 'GET', token });
  const factor = verifiedTotp(me.data);
  if (factor) {
    return { status: 200, body: { status: 'totp_required', factor_id: factor.id, access_token: token } };
  }
  const profile = await loadProfileByToken(token);
  if (!profile) return { status: 502, message: 'profile not found' };
  return { status: 200, body: { status: 'ok', username: profile.username, user_id: profile.id } };
}

// cloud_verify(p_access_token, p_factor_id, p_code) — check the 6-digit
// TOTP code. Challenge+verify yield an aal2 session; username and
// user_id come from the profile.
async function cloudVerify(body) {
  const token = String(body.p_access_token || '');
  const factorId = String(body.p_factor_id || '');
  const code = String(body.p_code || '');
  if (!token || !factorId || !/^\d{6}$/.test(code)) {
    throw new HttpError(400, 'Enter 6 digits from the app');
  }
  const challenge = await auth(`factors/${factorId}/challenge`, { token, body: {} });
  if (challenge.status !== 200 || !challenge.data || !challenge.data.id) {
    throw new HttpError(502, 'Could not request the code');
  }
  const verified = await auth(`factors/${factorId}/verify`, { token, body: { challenge_id: challenge.data.id, code } });
  if (verified.status !== 200 || !verified.data || !verified.data.access_token) {
    throw new HttpError(401, 'Invalid code. Check the time in the app.');
  }
  const profile = await loadProfileByToken(verified.data.access_token);
  if (!profile) throw new HttpError(502, 'profile not found');
  return { status: 200, body: { status: 'ok', username: profile.username, user_id: profile.id } };
}

/** Profile of a user by their access token (service key: RLS on profiles does not allow anon). */
async function loadProfileByToken(token) {
  const me = await auth('user', { method: 'GET', token });
  if (me.status !== 200 || !me.data || !me.data.id) return null;
  const rows = await sb(`profiles?id=eq.${me.data.id}&select=id,username,role,email_verified`);
  return Array.isArray(rows) && rows[0] ? rows[0] : null;
}

// dev functions: (id, body) => PostgREST body for RPC.
// cloud functions: async (id, body) => {status, body} | {status, message}.
// cloud_sync_push(p_settings) — save settings to user_settings.
// Called by OS `cloud sync`; the OS sends its cloud user_id from the
// link file. Settings are a flat JSON object with theme preset,
// accent, wallpaper style, perf profile, and language.
async function cloudSyncPush(body) {
  const user_id = String(body.p_user_id || '');
  if (!user_id) throw new HttpError(400, 'user_id required');
  const settings = body.p_settings;
  if (!settings || typeof settings !== 'object') {
    throw new HttpError(400, 'settings must be a JSON object');
  }
  const result = await rpc('save_user_settings', {
    p_user_id: user_id,
    p_settings: settings,
  });
  return { status: 200, body: { status: 'ok', result: result.result, updated_at: result.updated_at } };
}

// cloud_sync_pull — return saved settings. Called by OS `cloud pull`.
async function cloudSyncPull(body) {
  const user_id = String(body.p_user_id || '');
  if (!user_id) throw new HttpError(400, 'user_id required');
  const result = await rpc('get_user_settings', { p_user_id: user_id });
  if (result.result === 'none') {
    return { status: 200, body: { status: 'none' } };
  }
  return { status: 200, body: { status: 'ok', settings: result.settings, updated_at: result.updated_at } };
}

// cloud_clip_get — return clipboard text. Called by OS `cloud clip get`.
async function cloudClipGet(body) {
  const user_id = String(body.p_user_id || '');
  if (!user_id) throw new HttpError(400, 'user_id required');
  const result = await rpc('get_clipboard', { p_user_id: user_id });
  if (result.result === 'none') {
    return { status: 200, body: { status: 'none' } };
  }
  return { status: 200, body: { status: 'ok', content: result.content, created_at: result.created_at } };
}

// cloud_clip_set — save clipboard text. Called by OS `cloud clip set`.
async function cloudClipSet(body) {
  const user_id = String(body.p_user_id || '');
  if (!user_id) throw new HttpError(400, 'user_id required');
  const content = String(body.p_content || '');
  if (!content) throw new HttpError(400, 'content required');
  const result = await rpc('save_clipboard', { p_user_id: user_id, p_content: content });
  return { status: 200, body: { status: 'ok', created_at: result.created_at } };
}

const FUNCTIONS = {
  begin_device_link: (id) => ({ p_device_id: id }),
  device_link_status: (id) => ({ p_device_id: id }),
  claim_dev_unlock: (id) => ({ p_device_id: id }),
  request_dev_unlock: (id, body) => ({
    p_device_id: id,
    p_hostname: typeof body.p_hostname === 'string' ? body.p_hostname.slice(0, 64) : null,
    p_source: 'os',
  }),
  cloud_login: (id, body) => cloudLogin(body),
  cloud_verify: (id, body) => cloudVerify(body),
  cloud_sync_push: (id, body) => cloudSyncPush(body),
  cloud_sync_pull: (id, body) => cloudSyncPull(body),
  cloud_clip_get: (id, body) => cloudClipGet(body),
  cloud_clip_set: (id, body) => cloudClipSet(body),
};

function reject(res, status, message) {
  return send(res, status, { message });
}

module.exports = async function handler(req, res) {
  try {
    if (req.method !== 'POST') {
      res.setHeader('Allow', 'POST');
      return reject(res, 405, 'POST only');
    }
    const url = new URL(req.url, 'http://x');
    const name = url.searchParams.get('fn') || '';
    const build = Object.prototype.hasOwnProperty.call(FUNCTIONS, name) ? FUNCTIONS[name] : null;
    if (!build) return reject(res, 404, 'unknown function');

    const body = readBody(req);
    const id = String(body.p_device_id || '').trim().toLowerCase();
    if (!DEVICE_ID_RE.test(id)) return reject(res, 400, 'device id must be 32 lowercase hex characters');

    const ipKey = `os-ip:${clientIp(req)}`;
    const devKey = `os-dev:${id}`;
    if (overLimit(ipKey, 120, 60 * 60 * 1000) || overLimit(devKey, 60, 60 * 60 * 1000)) {
      return reject(res, 429, 'too many requests, try again later');
    }
    recordHit(ipKey);
    recordHit(devKey);

    const out = await build(id, body);
    // cloud functions return {status, body} or {status, message}.
    if (out && out.body) return send(res, out.status, out.body);
    if (out && out.message) return reject(res, out.status, out.message);
    // dev functions: out — PostgREST call body.
    const result = await rpc(name, out);
    return send(res, 200, result);
  } catch (err) {
    if (err instanceof HttpError) return reject(res, err.status, err.message);
    console.error('os gateway error:', err && err.message);
    return reject(res, 500, 'internal error');
  }
};
