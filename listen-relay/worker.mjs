import { DurableObject } from 'cloudflare:workers';
import { publicState, validState } from './state.mjs';

const ttl = 45_000;
const reply = (status, state) => new Response(state === undefined ? null : JSON.stringify(state), {
  status, headers: { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' },
});
const token = () => crypto.randomUUID().replaceAll('-', '');

export class Room extends DurableObject {
  async create() {
    const key = token() + token();
    const now = Date.now();
    await this.ctx.storage.put('room', { key, state: null, seen: now });
    await this.ctx.storage.setAlarm(now + ttl);
    return key;
  }
  async read() {
    const room = await this.ctx.storage.get('room');
    if (!room || Date.now() - room.seen >= ttl) return reply(404);
    const state = room.state;
    return reply(200, state && { ...state, position: state.position +
      (state.playing ? Math.max(0, Date.now() - room.seen) / 1000 : 0) });
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

async function readState(req) {
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
  const state = JSON.parse(text + decoder.decode());
  if (!validState(state)) throw new Error('invalid state');
  return publicState(state);
}

export default {
  async fetch(req, env) {
    const path = new URL(req.url).pathname;
    if (path === '/rooms' && req.method === 'POST') {
      const quota = await env.CREATIONS.limit({ key: req.headers.get('CF-Connecting-IP') || 'unknown' });
      if (!quota.success) return reply(429);
      const id = token();
      const key = await env.ROOMS.getByName(id).create();
      return reply(201, { id, key });
    }
    const id = /^\/rooms\/([a-f0-9]{32})$/.exec(path)?.[1];
    if (!id) return reply(404);
    const room = env.ROOMS.getByName(id);
    if (req.method === 'GET') return room.read();
    if (!['PUT', 'DELETE'].includes(req.method)) return reply(405);
    const key = /^Bearer ([a-f0-9]{64})$/.exec(req.headers.get('Authorization') || '')?.[1];
    if (!key) return reply(403);
    let state = null;
    if (req.method === 'PUT') {
      try { state = await readState(req); }
      catch (e) { return reply(e instanceof RangeError ? 413 : 400); }
    }
    return room.update(key, state, req.method === 'DELETE');
  },
};
