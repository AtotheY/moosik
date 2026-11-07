import { DurableObject } from 'cloudflare:workers';
import { addressKey, allowance, hmacKey, issueId, reserve, validId, writeKey } from './guards.mjs';
import { publicState, validState } from './state.mjs';
import { playback, sessionVersion, syncRoom } from './session.mjs';

const ttl = 45_000;
const reply = (status, state, headers = {}) => new Response(state === undefined ? null : JSON.stringify(state), {
  status, headers: { 'Content-Type': 'application/json', 'Cache-Control': 'no-store', ...headers },
});
const allowCreate = allowance('create');
const allowPlayback = allowance('playback');

export class RelayBudget extends DurableObject {
  reserve(kind, address) {
    return this.ctx.storage.transactionSync(() => reserve(this.ctx.storage.kv, kind, Date.now(), address));
  }
}

export class Room extends DurableObject {
  async create(key) {
    const now = Date.now();
    if (await this.ctx.storage.get('room')) return false;
    await this.ctx.storage.put('room', { key, state: null, seen: now });
    await this.ctx.storage.setAlarm(now + ttl);
    return true;
  }
  async read() {
    const room = await this.ctx.storage.get('room');
    if (!room || Date.now() - room.seen >= ttl) return reply(404);
    return reply(200, playback(room, Date.now()), room.session ? { 'X-Moosik-Session': sessionVersion } : {});
  }
  sync(token, host, body) {
    return this.ctx.storage.transactionSync(() => {
      const kv = this.ctx.storage.kv;
      const room = kv.get('room');
      const now = Date.now();
      if (!room || now - room.seen >= ttl) return reply(404);
      const result = syncRoom(room, token, host, body, now);
      if (result.status < 300) kv.put('room', room);
      return reply(result.status, result.value);
    });
  }
  async update(key, state, close) {
    const room = await this.ctx.storage.get('room');
    const now = Date.now();
    if (!room || now - room.seen >= ttl) return reply(404);
    const enc = new TextEncoder();
    if (!/^[0-9a-f]{64}$/.test(key) || !crypto.subtle.timingSafeEqual(enc.encode(key), enc.encode(room.key))) {
      return reply(403);
    }
    if (close) {
      await this.ctx.storage.deleteAll();
    } else {
      await this.ctx.storage.put('room', { key, state, seen: now });
    }
    return reply(204);
  }
  async alarm() {
    const room = await this.ctx.storage.get('room');
    if (room && room.seen + ttl > Date.now()) {
      await this.ctx.storage.setAlarm(room.seen + ttl);
    } else {
      await this.ctx.storage.deleteAll();
    }
  }
}

async function readBody(req) {
  if (!req.body) throw new Error('missing body');
  const reader = req.body.getReader();
  let size = 0, text = '';
  const decoder = new TextDecoder();
  for (;;) {
    const { value, done } = await reader.read();
    if (done) break;
    size += value.byteLength;
    if (size > 4096) {
      await reader.cancel();
      throw new RangeError('too large');
    }
    text += decoder.decode(value, { stream: true });
  }
  return JSON.parse(text + decoder.decode());
}

export default {
  async fetch(req, env) {
    const path = new URL(req.url).pathname;
    if (path === '/rooms' && req.method === 'POST') {
      const ip = req.headers.get('CF-Connecting-IP') || 'unknown';
      const quota = await env.CREATIONS.limit({ key: ip });
      if (!quota.success) return reply(429);
      const signer = await hmacKey(env.ROOM_SIGNING_KEY);
      if (!await allowCreate(env, Date.now(), await addressKey(signer, ip))) return reply(429);
      const id = await issueId(signer, Date.now());
      const key = await writeKey(signer, id);
      if (!await env.ROOMS.getByName(id).create(key)) return reply(503);
      return reply(201, { id, key, session: sessionVersion });
    }
    const id = /^\/rooms\/([a-f0-9]{32})$/.exec(path)?.[1];
    if (!id) return reply(404);
    if (!['GET', 'PUT', 'DELETE'].includes(req.method)) return reply(405);
    const ip = req.headers.get('CF-Connecting-IP') || 'unknown';
    if (!(await env.TRAFFIC.limit({ key: ip })).success) return reply(429);
    const signer = await hmacKey(env.ROOM_SIGNING_KEY);
    if (!await validId(signer, id, Date.now())) return reply(404);
    if (req.method === 'GET') {
      if (!await allowPlayback(env, Date.now())) return reply(429);
      return env.ROOMS.getByName(id).read();
    }
    const key = /^Bearer ([a-f0-9]{64})$/.exec(req.headers.get('Authorization') || '')?.[1];
    const sync = req.method === 'PUT' && req.headers.get('X-Moosik-Session') === sessionVersion;
    const guest = req.headers.get('X-Moosik-Listener') || '';
    if (key) {
      const enc = new TextEncoder();
      if (!crypto.subtle.timingSafeEqual(enc.encode(await writeKey(signer, id)), enc.encode(key))) return reply(403);
    } else if (!sync || req.headers.has('Authorization') || !/^[0-9a-f]{32}$/.test(guest)) return reply(403);
    let state = null;
    if (req.method === 'PUT') {
      try { state = await readBody(req); }
      catch (e) { return reply(e instanceof RangeError ? 413 : 400); }
      if (!sync && !validState(state)) return reply(400);
    }
    if (!await allowPlayback(env, Date.now())) return reply(429);
    if (sync) return env.ROOMS.getByName(id).sync(guest, Boolean(key), state);
    return env.ROOMS.getByName(id).update(key, publicState(state), req.method === 'DELETE');
  },
};
