// Runs atomically inside the room's storage transaction (or the local relay's event loop).
import { publicState, validState } from './state.mjs';

export const sessionVersion = '1';
const maxQueue = 48;
const maxGuests = 31;
const memberTTL = 15_000;

export function playback(room, now) {
  const s = room.state;
  return s && { ...s, position: s.position + (s.playing ? Math.max(0, now - room.seen) / 1000 : 0) };
}

// Store compact CDN keys only. Older clients omit this field.
function publicMember({ id, name, avatar }) {
  return avatar ? { id, name, avatar } : { id, name };
}

export function syncRoom(room, token, host, body, now) {
  if (!body || typeof body !== 'object' || typeof body.name !== 'string' || body.name.length > 128
    || (host ? !validState(body.state) : ('state' in body || 'played' in body || 'keep_entries' in body || 'play_entry' in body))
    || (body.played !== undefined && (!Number.isSafeInteger(body.played) || body.played < 0))
    || (body.keep_entries !== undefined && (!Array.isArray(body.keep_entries) || body.keep_entries.length > maxQueue
      || !body.keep_entries.every(id => Number.isSafeInteger(id) && id > 0 && id <= body.played)))
    || (body.play_entry !== undefined && (!Number.isSafeInteger(body.play_entry) || body.play_entry < 1
      || !(body.play_entry <= body.played)))
    || (body.add !== undefined && (!body.add || !Number.isSafeInteger(body.add.seq)
      || body.add.seq < 1 || !body.add.song || !validState(body.add.song)))) return { status: 400 };
  if (!host && !room.session) return { status: 409 };
  const s = room.session ??= { members: {}, queue: [], nextMember: 1, nextEntry: 1 };
  for (const [key, member] of Object.entries(s.members)) {
    if (key !== 'host' && now - member.seen >= 45_000) delete s.members[key];
  }
  const key = host ? 'host' : token;
  if (!host && body.leave === true) {
    delete s.members[key];
    return { status: 204 };
  }
  if (!s.members[key] && !host && Object.keys(s.members).length >= maxGuests + 1) return { status: 429 };
  const member = s.members[key] ??= { id: host ? 0 : s.nextMember++, lastSeq: 0 };
  member.seen = now;
  const name = Array.from(body.name.replace(/[\p{Cc}\p{Cf}]/gu, '').trim()).slice(0, 32).join('');
  member.name = name || (host ? 'Host' : 'Listener ' + member.id);
  member.avatar = typeof body.avatar === 'string' && body.avatar.length <= 55
    && /^(?:default\/[0-5]|[0-9]{1,20}\/(?:a_)?[0-9a-f]{32})$/.test(body.avatar) ? body.avatar : null;
  if (host) {
    room.state = publicState(body.state);
    room.seen = now;
    const current = body.played && s.queue.find(item => item.entry === (body.play_entry ?? body.played));
    if (body.played > 0) {
      const keep = new Set(body.keep_entries);
      s.queue = s.queue.filter(item => item.entry > body.played || keep.has(item.entry));
    }
    if (current) {
      s.current = { id: current.song.id, by: current.by };
    } else if (room.state?.id !== s.current?.id) {
      delete s.current;
    }
  }

  if (body.add && body.add.seq > member.lastSeq) {
    member.lastSeq = body.add.seq;
    member.error = s.queue.length >= maxQueue ? 'The session queue is full' : undefined;
    if (!member.error) {
      const song = publicState(body.add.song);
      song.title = Array.from(song.title).slice(0, 128).join('');
      song.artist = Array.from(song.artist).slice(0, 128).join('');
      s.queue.push({ entry: s.nextEntry++, song, by: publicMember(member) });
    }
  }
  const members = Object.values(s.members)
    .filter(m => m.id === 0 || now - m.seen < memberTTL)
    .map(publicMember)
    .sort((a, b) => a.id - b.id);
  const currentBy = room.state && room.state.id === s.current?.id ? s.current.by : null;
  return { status: 200, value: {
    state: playback(room, now), members, queue: s.queue, current_by: currentBy,
    can_pick_entry: true, you: member.id, error: body.add ? member.error : undefined,
  } };
}
