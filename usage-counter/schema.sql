CREATE TABLE IF NOT EXISTS recent (
  day INTEGER NOT NULL,
  token TEXT NOT NULL,
  minutes INTEGER NOT NULL CHECK (minutes BETWEEN 1 AND 1440),
  PRIMARY KEY (day, token)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS archive (
  day INTEGER PRIMARY KEY,
  listeners INTEGER NOT NULL,
  minutes INTEGER NOT NULL
);

CREATE VIEW IF NOT EXISTS daily_usage AS
SELECT date(day * 86400, 'unixepoch') AS utc_date,
       listeners AS opted_in_listeners, round(minutes / 60.0, 2) AS listening_hours
FROM (
  SELECT day, count(*) AS listeners, sum(minutes) AS minutes FROM recent GROUP BY day
  UNION ALL SELECT day, listeners, minutes FROM archive
);
