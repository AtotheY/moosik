-- Additive migration. Listening-minute reports from earlier releases remain valid.
CREATE TABLE IF NOT EXISTS active_recent (
  day INTEGER NOT NULL,
  token TEXT NOT NULL,
  PRIMARY KEY (day, token)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS active_archive (
  day INTEGER PRIMARY KEY,
  devices INTEGER NOT NULL
);

CREATE VIEW IF NOT EXISTS daily_activity AS
WITH active AS (
  SELECT date(day * 86400, 'unixepoch') AS utc_date, count(*) AS devices
  FROM active_recent GROUP BY day
  UNION ALL
  SELECT date(day * 86400, 'unixepoch'), devices FROM active_archive
), dates AS (
  SELECT utc_date FROM active UNION SELECT utc_date FROM daily_usage
)
SELECT dates.utc_date, active.devices AS active_devices,
       coalesce(daily_usage.opted_in_listeners, 0) AS opted_in_listeners,
       coalesce(daily_usage.listening_hours, 0) AS listening_hours
FROM dates
LEFT JOIN active USING (utc_date)
LEFT JOIN daily_usage USING (utc_date);
