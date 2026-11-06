// Only Cloudflare holds this secret. IDs stay 32 lowercase hex characters for v0.1.26.
const encoder = new TextEncoder();
const hex = bytes => Array.from(new Uint8Array(bytes), b => b.toString(16).padStart(2, '0')).join('');
const maxAgeSeconds = 12 * 60 * 60;
const dayMs = 86_400_000;
const daily = { create: 100, playback: 60_000 };
const grants = { create: 1, playback: 32 };

export function hmacKey(secret) {
  if (!secret) throw new Error('Missing relay signing key');
  return crypto.subtle.importKey('raw', encoder.encode(secret), { name: 'HMAC', hash: 'SHA-256' }, false, ['sign']);
}

async function sign(key, value) {
  return hex(await crypto.subtle.sign('HMAC', key, encoder.encode(value)));
}

export async function issueId(key, now) {
  const prefix = Math.floor(now / 1000).toString(16).padStart(8, '0') + hex(crypto.getRandomValues(new Uint8Array(4)));
  return prefix + (await sign(key, 'invite:' + prefix)).slice(0, 16);
}

export async function validId(key, id, now) {
  if (!/^[0-9a-f]{32}$/.test(id)) return false;
  const age = Math.floor(now / 1000) - parseInt(id.slice(0, 8), 16);
  if (age < -30 || age >= maxAgeSeconds) return false;
  const expected = (await sign(key, 'invite:' + id.slice(0, 16))).slice(0, 16);
  return crypto.subtle.timingSafeEqual(encoder.encode(expected), encoder.encode(id.slice(16)));
}

export const addressKey = (key, ip) => sign(key, 'source:' + ip);
export const writeKey = (key, id) => sign(key, 'host:' + id);

// Called inside a synchronous storage transaction on the same object for all locations.
export function reserve(kv, kind, now, address) {
  const day = Math.floor(now / dayMs);
  if (!Object.hasOwn(daily, kind)) throw new Error('Unknown quota');
  const counter = kv.get(kind);
  const used = counter?.day === day ? counter.used : 0;
  const count = Math.min(grants[kind], daily[kind] - used);
  if (count && kind === 'create') {
    if (!address) throw new Error('Missing source');
    // Only successful starts enter this list; the daily budget also bounds its size.
    const recent = (kv.get('recentStarts') || []).filter(s => now - s.at < 60_000);
    if (recent.filter(s => s.address === address).length >= 4) return { day, count: 0, sourceLimited: true };
    kv.put('recentStarts', [...recent, { address, at: now }]);
  }
  if (count > 0) kv.put(kind, { day, used: used + count });
  return { day, count };
}

// Keep only prepaid units in isolate memory. Eviction loses spare units; it cannot mint more.
// Parallel refills are charged separately. Never share a request-bound promise across invocations.
export function allowance(kind) {
  let day = -1, remaining = 0, exhausted = false;
  return async (env, now, address) => {
    const today = Math.floor(now / dayMs);
    if (today !== day) { day = today; remaining = 0; exhausted = false; }
    if (remaining > 0) { remaining--; return true; }
    if (exhausted) return false;
    if (!(await env.ADMISSIONS.limit({ key: kind })).success) return false;
    const grant = await env.BUDGET.getByName('global-v1').reserve(kind, address ?? null);
    if (grant.day !== day || grant.day !== Math.floor(Date.now() / dayMs)) return false;
    if (!grant.count) { exhausted = !grant.sourceLimited; return false; }
    remaining += grant.count - 1;
    return true;
  };
}
