# Moosik listening stats

Open **Listening stats** in the sidebar. Choose a period from the dropdown and use
the pill navigation to explore. Export and refresh actions are in **•••**; definitions
and privacy details are behind the **ⓘ** button. Recording begins with playback in Moosik
0.1.14+ark.5; there is no fabricated backfill from YouTube's history.

## What you can explore

- **Overview:** listening time, qualified plays, unique songs/artists, active days,
  longest day streak, newly heard songs, completed listens, activity heatmap,
  top songs/artists, hourly and weekday patterns, skips, seeks, pauses, buffering,
  app-muted time, average listening per active day, observation gaps, and sources.
- **Songs:** searchable table with listening time, play/skip/completion counts,
  a song detail view with unique coverage and first/last session dates, and sorting by time, plays,
  skips, completions, recency, title, or personal star rating. Click a row to
  add comma-separated genres and mood tags and a 0–5 rating (0 = unrated).
- **Artists / Albums / Genres:** rankings by listening time, with play and song
  counts. Every credited artist and every assigned genre receives full credit;
  these group totals overlap, while the headline total counts each second once.
- **History:** compact artwork rows for the latest 1,000 attempts in a selected range, including partial
  listens and unsuccessful attempts. All older rows remain in the ledger/export.
- **My recap:** an artwork-led recap for any date range, with top songs/artists, discovery,
  streaks, and biggest listening day. Export it as an editable Markdown document.

Use Today, 7 days, 30 days, This month, This year, All time, or inclusive custom
calendar dates. You can filter YouTube, downloaded YouTube tracks, or local files.
The dashboard refreshes about every ten seconds; ••• → Refresh requests an immediate
snapshot. Genres are user-assigned: missing source metadata stays **Unknown**.
No artist-name lookups or listening-history uploads to an enrichment service occur.

## Definitions

- **Listening time:** observed forward playback, in wall-clock seconds, at a
  nonzero app volume. Pauses, seeks, buffering, app mute, and unobserved intervals
  are excluded. Media progression is stored separately for speed-aware analysis.
- **Qualified play:** 30 audible seconds or half the known duration, whichever is
  shorter, within one playback session. In a filtered report this threshold is
  applied to listening time inside the range; counts across adjacent ranges are
  therefore not necessarily additive.
- **Completed listen:** an ended session with at least 90% distinct track coverage.
  Replaying the same section does not increase distinct coverage. Completions are
  assigned to the range containing the session's end. Unknown duration cannot
  establish completion. Coverage columns describe full sessions touching the range.
- **Skip:** a partly heard, under-90%-covered session replaced by another playback
  attempt. Pausing, closing the app, interruptions, and playback failures aren't
  skips. A manually replayed track after a natural ending is a new attempt.
- **New song:** first observed audible playback in this local ledger, not a claim
  that you have never heard it on another app, device, or account.
- **Time patterns:** local date/hour/weekday and UTC offset at observation time.
  UTC timestamps remain available for alternative timezone analysis. Sampling can
  place up to roughly one second on the other side of an hour/day boundary.

Playback is sampled approximately once a second, including when minimized. Timing
is conservative: transitions can omit fractions of a second. The app cannot detect
system mute, the actual sound output device, or whether someone is paying attention.
A UI stall, suspended computer, or blocking native dialog longer than ten seconds
is marked as an observation gap, never charged as listening. No historical device
activity outside this Moosik installation is captured.

## Stored data and durability

The local database is at:

`~/Library/Application Support/refryt/listening/listening.sqlite3`

The existing `refryt` data directory is intentional; it preserves the established
app identity and account settings. The listening folder is owner-only on macOS;
the database is created with owner read/write permissions. This is **not separate
application encryption**. Mac account/disk security still matters.

SQLite uses WAL, full synchronous commits, foreign keys, schema version checks,
and one worker thread. A process lock permits only one recorder per data directory.
Playback never waits on report queries or routine writes. Samples are committed
about every five seconds and at session boundaries. An abrupt crash can lose the
last uncommitted samples; previously committed data remains. An interrupted session
is recovered with its last committed time, not extended to the next launch.

Tables retained without automatic pruning:

| Table | Contents |
| --- | --- |
| `sessions` | Attempt ID, track ID/title, artist names/IDs, album/ID, known duration, explicit/liked/video flags and artwork URL snapshot; source, originating page, playlist ID, shuffle/repeat mode, initial volume; start/end/outcome and distinct coverage |
| `segments` | UTC endpoints, positions, audible/media/paused/buffering/muted/unobserved milliseconds, local date/hour/weekday/offset, volume, speed |
| `events` | Starts/ends, observed pause/resume/seek/buffering/volume changes, retries, video/audio reloads, gaps, and recovery events |
| `track_tags` | Current user genres, mood tags, and rating |
| `tag_edits` | Append-only history of local label/rating changes |

No Google cookies, credentials, signed stream URLs, or raw authenticated API
responses are stored here. Local tracks retain their app track identifier, which
can be a music file path. Artist and track text comes from the music metadata.
The database is local; it is not a backup unless you export/copy it safely.

## Exports

- **Song stats CSV:** selected date/source range; spreadsheet-formula prefixes in
  metadata are neutralized. Search text and table sort are presentation controls,
  not export filters.
- **My recap:** selected range as editable Markdown.
- **Everything JSON:** the selected report plus the **entire** raw sessions,
  segments, events, current tags, and tag-edit history, schema version 1.
- **Database backup:** a consistent SQLite snapshot including committed WAL data.
  Use this button instead of copying only the live main database file.

Export files are written through temporary files and installed after success.
The live database cannot be selected as an export target. Exports are never sent
anywhere automatically. Keep raw exports private if you don't want to share your
music habits. There is no automatic deletion, cross-device sync, or import yet.

## Verification

Focused tests cover audible progress versus pause/mute/buffering/seek, playback
speed, non-overlapping coverage, interruption recovery, date clipping, collaborator
attribution, source filtering, genre changes, JSON/CSV/recap exports, and a consistent
SQLite backup with integrity check. Existing playback/queue tests are also run.

Commands:

```
cargo test --locked -p refryt --lib listening
cargo test --locked -p refryt --lib app::tests
cargo clippy --locked -p refryt --lib --tests -- -D warnings
cargo build --locked --release -p refryt --bin refryt
```
