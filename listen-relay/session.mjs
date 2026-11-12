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

const validSkipTarget = value => typeof value === 'string' && /^[0-9a-f]{1,16}-[0-9a-f]{1,16}$/.test(value);

const entryId = n => Number.isSafeInteger(n) && n > 0;
function validEdit(edit) {
  if (!edit || !entryId(edit.seq)) return false;
  if ('undo' in edit) return entryId(edit.undo) && !('entries' in edit) && !('before' in edit);
  return Array.isArray(edit.entries) && edit.entries.length > 0 && edit.entries.length <= maxQueue
    && edit.entries.every(entryId) && new Set(edit.entries).size === edit.entries.length
    && (!('before' in edit) || edit.before === null || entryId(edit.before) && !edit.entries.includes(edit.before));
}

function editQueue(s, member, edit) {
  if (edit.seq <= (member.lastEdit ?? 0)) return;
  member.lastEdit = edit.seq;
  if ('undo' in edit) {
    if (s.undo?.member === member.id && s.undo.seq === edit.undo) {
      s.queue = s.undo.queue;
      delete s.undo;
    }
    return;
  }
  const selected = new Set(edit.entries);
  const remaining = s.queue.filter(e => !selected.has(e.entry));
  if (remaining.length === s.queue.length) return;
  if ('before' in edit) {
    const at = edit.before === null ? remaining.length : remaining.findIndex(e => e.entry === edit.before);
    if (at < 0) return; // Another listener removed the drop target.
    remaining.splice(at, 0, ...s.queue.filter(e => selected.has(e.entry)));
    if (remaining.every((e, i) => e === s.queue[i])) return;
  }
  s.undo = { member: member.id, seq: edit.seq, queue: s.queue };
  s.queue = remaining;
}

export function syncRoom(room, token, host, body, now) {
  if (!body || typeof body !== 'object' || typeof body.name !== 'string' || body.name.length > 128
    || (host ? !validState(body.state) : ('state' in body || 'played' in body || 'keep_entries' in body || 'play_entry' in body))
    || ('played_entries' in body && (!host || !Array.isArray(body.played_entries) || body.played_entries.length > maxQueue
      || !body.played_entries.every(entryId)))
    || ('edit' in body && !validEdit(body.edit))
    || ('skip_target' in body && (!host || !validSkipTarget(body.skip_target) || !body.state))
    || ('skip' in body && (host || !validSkipTarget(body.skip)))
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
    // The host changes this token on a new playback or after handling Next (including at
    // the end of its queue). A retried guest request can only affect the original track.
    if (body.skip_target !== s.skipTarget || body.state?.id !== room.state?.id) {
      s.skipRequested = false;
      delete s.undo;
    }
    s.queueEdits = Array.isArray(body.played_entries);
    s.skipTarget = body.skip_target;
    room.state = publicState(body.state);
    room.seen = now;
    const current = body.played && s.queue.find(item => item.entry === (body.play_entry ?? body.played));
    if (body.played > 0) {
      const consumed = new Set(body.played_entries);
      const keep = new Set(body.keep_entries);
      const previousLength = s.queue.length;
      s.queue = s.queue.filter(item => s.queueEdits ? !consumed.has(item.entry) : item.entry > body.played || keep.has(item.entry));
      if (s.queue.length !== previousLength) delete s.undo;
    }
    if (current) {
      s.current = { id: current.song.id, by: current.by };
    } else if (room.state?.id !== s.current?.id) {
      delete s.current;
    }
  }

  if (!host && body.skip && body.skip === s.skipTarget) s.skipRequested = true;

  if (body.add && body.add.seq > member.lastSeq) {
    member.lastSeq = body.add.seq;
    member.error = s.queue.length >= maxQueue ? 'The session queue is full' : undefined;
    if (!member.error) {
      const song = publicState(body.add.song);
      song.title = Array.from(song.title).slice(0, 128).join('');
      song.artist = Array.from(song.artist).slice(0, 128).join('');
      delete s.undo;
      s.queue.push({ entry: s.nextEntry++, song, by: publicMember(member) });
    }
  }
  if (s.queueEdits && body.edit) editQueue(s, member, body.edit);
  const members = Object.values(s.members)
    .filter(m => m.id === 0 || now - m.seen < memberTTL)
    .map(publicMember)
    .sort((a, b) => a.id - b.id);
  const currentBy = room.state && room.state.id === s.current?.id ? s.current.by : null;
  return { status: 200, value: {
    state: playback(room, now), members, queue: s.queue, current_by: currentBy,
    skip_target: s.skipTarget, skip_requested: host && s.skipRequested === true,
    can_edit_queue: s.queueEdits, undo_edit: s.undo?.member === member.id ? s.undo.seq : undefined,
    can_pick_entry: true, you: member.id, error: body.add ? member.error : undefined,
  } };
}
