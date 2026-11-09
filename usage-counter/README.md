# Daily listening counts

Open **Cloudflare → D1 → moosik-usage → Studio → daily_usage**, or run:

```sql
SELECT * FROM daily_usage ORDER BY utc_date DESC LIMIT 30;
```

Each row shows participating devices and listening hours for one UTC day. One device qualifies after a full minute of observed playback. Time is rounded down to whole minutes per device. The current and previous day can still receive reports; missing dates have no reports. Do not add daily listener counts to estimate unique people across days.

**What goes over the wire:** only the UTC day, a random 128-bit token for that day, and cumulative whole minutes. The client has a separate HTTP client with no session cookies and follows no redirects. No historical backfill. It reports at most every five minutes during normal operation (including retrying pending totals); no real-time presence. The day’s token survives restarts. Turning sharing off cancels outstanding work and discards the local counter; already delivered counts remain. Re-enabling that day or using a second profile/device can count someone twice. Long offline periods, short sessions and people who leave the switch off are missed. The write endpoint is public, so these are directional product metrics, not audited statistics.

**Retention:** the collector accepts today and yesterday. At 00:05 UTC a daily job folds older rows into totals and deletes their tokens from the live database in the same transaction. Infrastructure backups can outlive the live rows. Worker request logs and tracing are disabled. Cloudflare processes connection IPs for routing and temporary rate limiting; the app and usage database do not collect or store them. There is no public read endpoint.

**Operations:** the standalone Worker uses the configuration in `wrangler.jsonc`. Apply `schema.sql` to the bound D1 database before deploying. Re-deploy with `wrangler deploy --config usage-counter/wrangler.jsonc` from the repo root. Do not enable request logging or add identifying dimensions. Check ingestion in a local fixture; keep synthetic rows out of the production report. Run `node --test usage-counter/counter.test.mjs` (Node 22.13+).
