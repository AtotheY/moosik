import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {DatabaseSync} from 'node:sqlite';
import worker, {parse} from './worker.mjs';

test('retries, restarts and out-of-order reports do not inflate daily totals; expiration keeps totals only', async () => {
  const db = new DatabaseSync(':memory:');
  db.exec(readFileSync(new URL('./schema.sql', import.meta.url), 'utf8'));
  const env = {DB: {prepare: sql => ({bind: (...args) => ({run: () => db.prepare(sql).run(...args)})}),
    batch: async statements => {for (const s of statements) s.run();}}, LIMIT: {limit: async () => ({success: true})}};
  const today = Math.floor(Date.now()/86400000);
  const post = async (day, id, minutes) => {
    const body = day+' '+id.repeat(32)+' '+minutes;
    return (await worker.fetch(new Request('https://usage.test/v1/minutes',
      {method:'POST', body, headers:{'Content-Length': String(body.length)}}), env)).status;
  };
  for (const n of [1, 30, 30, 2]) assert.equal(await post(today, 'a', n), 204);
  assert.equal(await post(today, 'b', 30), 204);
  assert.equal(await post(today-1, 'c', 20), 204);
  for (const n of [0, 1441]) assert.equal(await post(today, 'b', n), 400);
  assert.equal(await post(today-2, 'b', 1), 400);
  assert.equal(parse(today+' '+ 'a'.repeat(32)+' 3\n', Date.now()), undefined);
  db.prepare('UPDATE recent SET day=? WHERE day=?').run(today-2,today-1);
  await worker.scheduled({}, env); await worker.scheduled({}, env);
  const rows = db.prepare('SELECT * FROM daily_usage ORDER BY utc_date DESC').all();
  assert.deepEqual(rows.map(r => [r.opted_in_listeners,r.listening_hours]), [[2,1],[1,0.33]]);
  assert.equal(db.prepare('SELECT count(*) AS n FROM recent WHERE day < ?').get(today).n, 0);
});
