// Single-replica room relay. A room contains only its latest playback state.
import http from 'node:http';
import { publicState, validState } from './state.mjs';
import { randomBytes } from 'node:crypto';
import { pathToFileURL } from 'node:url';

export function createRelay({ now = Date.now, ttl = 45_000, capacity = 4096 } = {}) {
  const rooms = new Map();
  const creators = new Map();
  function expire() {
    const time = now();
    for (const [id, room] of rooms) if (time - room.seen >= ttl) rooms.delete(id);
    for (const [address, quota] of creators) if (quota.until <= time) creators.delete(address);
  }
  const sweep = setInterval(expire, 15_000).unref();
  const server = http.createServer({ requestTimeout: 5000, headersTimeout: 5000 }, async (req, res) => {
    function reply(code, value) {
      res.writeHead(code, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' });
      res.end(value === undefined ? undefined : JSON.stringify(value));
    }
    if (req.url === '/rooms' && req.method === 'POST') {
      const address = req.socket.remoteAddress;
      let quota = creators.get(address);
      if (!quota || quota.until <= now()) {
        if (creators.size >= capacity) expire();
        if (creators.size >= capacity) return reply(503);
        quota = { count: 0, until: now() + 60_000 };
        creators.set(address, quota);
      }
      if (quota.count >= 30) return reply(429);
      if (rooms.size >= capacity) return reply(503);
      quota.count++;
      const id = randomBytes(16).toString('hex');
      const key = randomBytes(32).toString('hex');
      rooms.set(id, { key, state: null, seen: now() });
      return reply(201, { id, key });
    }
    const id = /^\/rooms\/([a-f0-9]{32})$/.exec(req.url)?.[1];
    const room = rooms.get(id);
    if (!room || now() - room.seen >= ttl) return reply(404);
    if (req.method === 'GET') {
      const state = room.state;
      return reply(200, state && { ...state, position: state.position +
        (state.playing ? Math.max(0, now() - room.seen) / 1000 : 0) });
    }
    if (req.headers.authorization !== 'Bearer ' + room.key) return reply(403);
    if (req.method === 'DELETE') {
      rooms.delete(id);
      return reply(204);
    }
    if (req.method !== 'PUT') return reply(405);
    try {
      let size = 0;
      const chunks = [];
      for await (const part of req) {
        size += part.length;
        if (size > 4096) return reply(413);
        chunks.push(part);
      }
      const state = JSON.parse(Buffer.concat(chunks));
      if (!validState(state)) return reply(400);
      room.state = publicState(state);
      room.seen = now();
      reply(204);
    } catch { reply(400); }
  });
  server.once('close', () => clearInterval(sweep));
  return server;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  createRelay().listen(Number(process.env.PORT || 8787), process.env.HOST || '127.0.0.1');
}
