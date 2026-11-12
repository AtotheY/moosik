import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRelay } from './server.mjs';

test('two listeners follow play, seek, pause, skip, unavailable tracks and expiry', async t => {
  let clock = 1000;
  const relay = createRelay({ now: () => clock });
  await new Promise(resolve => relay.listen(0, '127.0.0.1', resolve));
  t.after(() => new Promise(resolve => relay.close(resolve)));
  const base = 'http://127.0.0.1:' + relay.address().port + '/rooms';
  const { id, key } = await (await fetch(base, { method: 'POST' })).json();
  const url = base + '/' + id;
  const write = state => fetch(url, { method: 'PUT', headers: { Authorization: 'Bearer ' + key }, body: JSON.stringify(state) });
  const read = async () => (await fetch(url)).json();
  assert.equal((await fetch(url, { method: 'PUT' })).status, 403);
  assert.equal((await write({ id: '/etc/passwd' })).status, 400);
  const song = { id: 'dQw4w9WgXcQ', title: 'Song', artist: 'Artist', playing: true, position: 4 };
  assert.equal((await write(song)).status, 204);
  clock += 3000;
  assert.deepEqual(await Promise.all([read(), read()]), [7, 7].map(position => ({ ...song, position })));
  await write({ ...song, position: 20, playing: false });
  clock += 1000;
  assert.equal((await read()).position, 20);
  await write({ ...song, id: 'aaaaaaaaaaa', position: 0 });
  assert.equal((await read()).id, 'aaaaaaaaaaa');
  await write(null);
  assert.equal(await read(), null);
  clock += 46000;
  assert.equal((await fetch(url)).status, 404);
});

test('session members share an attributed FIFO queue, while only the host controls playback', async t => {
  let now = 1000;
  const relay = createRelay({ now: () => now });
  await new Promise(resolve => relay.listen(0, '127.0.0.1', resolve));
  t.after(() => new Promise(resolve => relay.close(resolve)));
  const base = 'http://127.0.0.1:' + relay.address().port + '/rooms';
  const { id, key, session } = await (await fetch(base, { method: 'POST' })).json();
  const room = base + '/' + id;
  const host = { Authorization: 'Bearer ' + key };
  const alice = { 'X-Moosik-Listener': 'a'.repeat(32) };
  const bob = { 'X-Moosik-Listener': 'b'.repeat(32) };
  const sync = (auth, body) => fetch(room, { method: 'PUT', headers: { ...auth, 'X-Moosik-Session': session }, body: JSON.stringify(body) });
  const song = { id: 'dQw4w9WgXcQ', title: 'Track', artist: 'Artist', playing: false, position: 0 };
  const first = { name: 'Alice', avatar: 'default/2', add: { seq: 1, song } };
  assert.equal((await sync(alice, first)).status, 409);
  assert.equal((await sync(host, { name: 'DJ', state: song })).status, 200);
  assert.equal((await fetch(room)).headers.get('X-Moosik-Session'), '1');
  assert.equal((await sync(alice, { name: 'DJ', state: null })).status, 400);
  assert.equal((await sync(alice, { name: 'DJ', played: 42 })).status, 400);
  assert.equal((await sync(alice, { ...first, add: { seq: 1, song: { ...song, id: 'file:///tmp' } } })).status, 400);
  const [a, b] = await Promise.all([sync(alice, first), sync(bob, { ...first, name: 'Bob', avatar: 'https://example.com/image.png' })]);
  assert.equal(a.status, 200);
  assert.equal(b.status, 200);
  const view = await (await sync(alice, first)).json();
  assert.equal(view.queue.length, 2);
  assert.equal(view.members.find(m => m.name === 'Alice').avatar, 'default/2');
  assert.equal(view.queue.find(e => e.by.name === 'Alice').by.avatar, 'default/2');
  assert.equal(view.members.find(m => m.name === 'Bob').avatar, undefined);
  assert.deepEqual(view.queue.map(e => e.entry), [1, 2]);
  assert.deepEqual(view.queue.map(e => e.by.name).sort(), ['Alice', 'Bob']);
  assert.deepEqual(view.members.map(m => m.name).sort(), ['Alice', 'Bob', 'DJ']);
  assert.equal(view.members.find(m => m.id === view.you).name, 'Alice');
  assert.ok(!JSON.stringify(view).includes(alice['X-Moosik-Listener']));
  const current = await (await sync(host, { name: 'DJ', state: song, played: 1,
    add: { seq: 1, song: { ...song, title: 'Host pick' } } })).json();
  assert.deepEqual(current.current_by, view.queue[0].by);
  assert.deepEqual(current.queue.map(e => e.entry), [2, 3]);
  assert.equal(current.queue[1].by.name, 'DJ');
  assert.equal((await sync(bob, { name: '', leave: true })).status, 204);
  assert.equal((await (await sync(alice, first)).json()).members.length, 2);
  now += 16000;
  const after = await (await sync(host, { name: 'DJ', state: song, played: 1 })).json();
  assert.deepEqual(after.members, [{ id: 0, name: 'DJ' }]);
  assert.deepEqual(after.queue[0].by, view.queue[1].by);
  assert.deepEqual(await (await fetch(room)).json(), song);
  // Play a later occurrence, keep the earlier one, then advance back to it. Retries
  // retain the contributor credit even when both occurrences have the same video ID.
  const pick = { name: 'DJ', state: song, played: 3, keep_entries: [2] };
  assert.equal(after.can_pick_entry, true);
  assert.equal((await sync(alice, { name: 'Alice', keep_entries: [2] })).status, 400);
  for (let attempt = 0; attempt < 2; attempt++) {
    const picked = await (await sync(host, pick)).json();
    assert.deepEqual(picked.queue.map(e => e.entry), [2]);
    assert.equal(picked.current_by.name, 'DJ');
  }
  const remaining = await (await sync(host, { name: 'DJ', state: song, played: 3, play_entry: 2 })).json();
  assert.deepEqual(remaining.queue, []);
  assert.deepEqual(remaining.current_by, view.queue[1].by);
  now += 46000;
  assert.equal((await sync(alice, first)).status, 404);
});

test('guest skips coalesce and expire when the host advances, even to another copy of the song', async t => {
  const relay = createRelay();
  await new Promise(resolve => relay.listen(0, '127.0.0.1', resolve));
  t.after(() => new Promise(resolve => relay.close(resolve)));
  const rooms = 'http://127.0.0.1:' + relay.address().port + '/rooms';
  const { id, key, session } = await (await fetch(rooms, { method: 'POST' })).json();
  const sync = (headers, body) => fetch(rooms + '/' + id, { method: 'PUT',
    headers: { 'X-Moosik-Session': session, ...headers }, body: JSON.stringify(body) });
  const song = { id: 'aaaaaaaaaaa', title: 'Track', artist: '', playing: true, position: 0 };
  const host = target => sync({ Authorization: 'Bearer ' + key }, { name: 'DJ', state: song, skip_target: target }).then(r => r.json());
  const guest = (token, body) => sync({ 'X-Moosik-Listener': token.repeat(32) }, { name: 'Listener', ...body });

  const { skip_target: first } = await host('1-0');
  for (const result of await Promise.all([guest('a', {skip: first}), guest('b', {skip: first})])) {
    assert.equal(result.status, 200);
  }
  assert.equal((await host(first)).skip_requested, true);
  assert.equal((await host(first)).skip_requested, true); // Lost response, same playback.
  assert.equal((await host('2-1')).skip_requested, false);
  await guest('b', {skip: first});
  assert.equal((await host('2-1')).skip_requested, false);
  await guest('b', {skip: '2-1'});
  assert.equal((await host('2-1')).skip_requested, true);
});
