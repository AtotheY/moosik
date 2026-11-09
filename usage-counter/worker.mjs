// Write-only on the public Internet. Reports are read through authenticated D1 Studio.
const DAY = 86_400_000;
export const record = `
  INSERT INTO recent(day, token, minutes)
  SELECT ?1, ?2, ?3 WHERE ?1 BETWEEN CAST(unixepoch()/86400 AS INTEGER)-1 AND CAST(unixepoch()/86400 AS INTEGER)
  ON CONFLICT(day, token) DO UPDATE SET minutes = excluded.minutes
  WHERE excluded.minutes > recent.minutes`;
export const rollup = `INSERT INTO archive(day, listeners, minutes)
  SELECT day, count(*), sum(minutes) FROM recent WHERE day < ? GROUP BY day`;
export const expire = 'DELETE FROM recent WHERE day < ?';

export function parse(text, now) {
  const match = /^(\d{5}) ([a-f0-9]{32}) ([1-9]\d{0,3})$/.exec(text);
  if (!match || text !== match[0]) return;
  const day = Number(match[1]), minutes = Number(match[3]), today = Math.floor(now / DAY);
  if ((day !== today && day !== today - 1) || minutes > 1440) return;
  return [day, match[2], minutes];
}

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    if (url.pathname !== '/v1/minutes' || url.search) return new Response(null, {status: 404});
    if (request.method !== 'POST') return new Response(null, {status: 405, headers: {Allow: 'POST'}});
    // Only fixed-length native client bodies. Cloudflare sets Content-Length from the actual transfer.
    const len = Number(request.headers.get('content-length'));
    if (!Number.isInteger(len) || len < 40 || len > 43) return new Response(null, {status: 400});
    const {success} = await env.LIMIT.limit({key: request.headers.get('cf-connecting-ip') || 'unknown'});
    if (!success) return new Response(null, {status: 429});
    const report = parse(await request.text(), Date.now());
    if (!report) return new Response(null, {status: 400});
    await env.DB.prepare(record).bind(...report).run();
    return new Response(null, {status: 204});
  },
  async scheduled(_event, env) {
    const cutoff = Math.floor(Date.now() / DAY) - 1;
    // D1 batches are transactional: never delete the inputs without saving their totals.
    await env.DB.batch([rollup, expire].map(sql => env.DB.prepare(sql).bind(cutoff)));
  },
};
