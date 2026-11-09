# Daily activity and listening hours

Open **Cloudflare → D1 → moosik-usage → Studio → daily_activity**, or run:

```sql
SELECT * FROM daily_activity ORDER BY utc_date DESC LIMIT 30;
```

The primary column, `active_devices`, counts profiles using that release once per UTC day. A manual launch, a click, key press, scroll or playback qualifies. An unattended login launch or idle window does not qualify on its own. Multiple devices or profiles can count a person more than once. Older releases and devices that cannot reach the collector may be missing. A null count indicates that only the older listening-time signal was available for that date.

The app submits the UTC day and a random daily token to `/v1/active`, independent of the listening switch. Its local receipt allows retries and restarts without counting again. The separate activity token carries no listening data. Set `MOOSIK_DISABLE_ACTIVE_COUNT=1` in the launch environment to disable these requests, including in automated fixtures.

The remaining columns show opt-in listeners and listening hours. One device qualifies after a full minute of observed playback. Time is rounded down to whole minutes per device. The current and previous day can still receive reports; missing dates have no reports. Do not add daily listener counts to estimate unique people across days.

**Optional listening-time reports:** only the UTC day, a random 128-bit token for that day, and cumulative whole minutes. The client has a separate HTTP client with no session cookies and follows no redirects. No historical backfill. It reports at most every five minutes during normal operation (including retrying pending totals); no real-time presence. The day’s token survives restarts. Turning sharing off cancels outstanding work and discards the local counter; already delivered counts remain. Re-enabling that day or using a second profile/device can count someone twice. Long offline periods, short sessions and people who leave the switch off are missed. The write endpoint is public, so these are directional product metrics, not audited statistics.

**Retention:** the collector accepts today and yesterday. At 00:05 UTC a daily job folds older rows into totals and deletes their tokens from the live database in the same transaction. Infrastructure backups can outlive the live rows. Worker request logs and tracing are disabled. Cloudflare processes connection IPs for routing and temporary rate limiting; the app and usage database do not collect or store them. There is no public read endpoint.

**Operations:** the standalone Worker uses the configuration in `wrangler.jsonc`. Apply `schema.sql` and then `activity.sql` to the bound D1 database before deploying. Re-deploy with `wrangler deploy --config usage-counter/wrangler.jsonc` from the repo root. Do not enable request logging or add identifying dimensions. Check ingestion in a local fixture; keep synthetic rows out of the production report. Run `node --test usage-counter/counter.test.mjs` (Node 22.13+).
