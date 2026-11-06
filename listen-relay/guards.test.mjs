import assert from 'node:assert/strict';
import { timingSafeEqual } from 'node:crypto';
import { test } from 'node:test';
import { allowance, hmacKey, issueId, reserve, validId, writeKey } from './guards.mjs';

// Workers' SubtleCrypto extension uses the same constant-time comparison.
crypto.subtle.timingSafeEqual ??= timingSafeEqual;

test('invites fit old clients and verify before storage; guest ID cannot authorize host writes', async () => {
  const k = await hmacKey('fixture-only');
  const now = Date.now();
  const id = await issueId(k, now);
  assert.match(id, /^[0-9a-f]{32}$/);
  assert(await validId(k, id, now));
  assert(!await validId(k, id.slice(0, -1) + (id.at(-1) === '0' ? '1' : '0'), now));
  assert(!await validId(k, id, now + 12 * 60 * 60 * 1000));
  assert(!await validId(await hmacKey('other-fixture'), id, now));
  assert.match(await writeKey(k, id), /^[0-9a-f]{64}$/);
});

test('global allowances saturate at the configured daily limits and reset at UTC midnight', () => {
  const data = new Map();
  const kv = { get: k => data.get(k), put: (k, v) => data.set(k, v) };
  const day = Date.UTC(2026, 10, 6);
  for (const [kind, cap] of [['create',100],['playback',60000]]) {
    let claimed = 0;
    while (claimed < cap) claimed += reserve(kv, kind, day, String(claimed)).count;
    assert.equal(claimed, cap);
    assert.equal(reserve(kv, kind, day + 86399999).count, 0);
    assert.ok(reserve(kv, kind, day + 86400000, "next-day").count > 0);
  }
});

test('concurrent isolates spend only already reserved units and cache exhaustion', async () => {
  const now = Date.now(), day = Math.floor(now / 86400000);
  let left = 40, calls = 0;
  const env = {
    ADMISSIONS: { limit: async () => ({ success: true }) },
    BUDGET: { getByName: () => ({ reserve: async () => {
      calls++; const count = Math.min(left, 32); left -= count;
      return { day, count };
    } }) },
  };
  const isolates = [allowance('playback'), allowance('playback')];
  const attempts = () => Promise.all(Array.from({length: 80}, (_, i) => isolates[i%2](env, now)));
  const results = [...await attempts(), ...await attempts()];
  assert.equal(results.filter(Boolean).length, 40);
  const before = calls;
  assert.ok((await attempts()).every(v => !v));
  assert.equal(calls, before);
});

test('a source gets four starts in a rolling minute across locations and midnight', () => {
  const data = new Map();
  const kv = { get: k => data.get(k), put: (k, v) => data.set(k, v) };
  const start = Date.UTC(2026, 10, 6) - 1000;
  for (let i = 0; i < 4; i++) assert.equal(reserve(kv, 'create', start, 'a').count, 1);
  assert.equal(reserve(kv, 'create', start + 3000, 'a').sourceLimited, true);
  assert.equal(reserve(kv, 'create', start + 3000, 'b').count, 1);
  assert.equal(reserve(kv, 'create', start + 60000, 'a').count, 1);
});
