# Moosik roadmap

The goal: a YouTube Music desktop experience that serious listeners choose for its controls, organization, and understanding of their listening habits. Better everyday UX than Spotify is an ambition, not a claim that the preview already achieves it.

This is a direction for development, not a release-date promise. [Suggest a workflow](https://github.com/AtotheY/moosik/issues/new?template=feature.yml) or discuss a larger contribution before implementing it.

## Available in the source preview

- Refryt's native browsing, playback, queue, lyrics, themes, YouTube playlist editing, and local-file support.
- Local listening ledger; song, artist, album, and manually tagged genre reports.
- Custom date ranges, local ratings and mood tags, recap generation, and data exports.
- macOS Keychain session migration and restricted authenticated reporting destinations.
- Moosik Eclipse branding, macOS icon picker, and stable hover menus on song rows.

The fork is tested locally on Apple Silicon macOS. Inherited platform support does not establish that Moosik is verified on Windows or Linux.

## Next: make the daily interactions effortless

### Drag songs where they belong

Queue reordering and song drops into playlists, with a clear insertion line, a compact artwork preview, auto-scroll, keyboard cancellation, and no accidental playback. Distinguish moving within a queue from adding a copy to a playlist. Test duplicate tracks, shuffle, long lists, and the currently playing item. Existing playlist-row reordering is only part of this work.

### Keep private organization separate from YouTube

Client-only playlists of streamed tracks, visibly distinct from playlists that sync to YouTube. Existing local playlists support local files only. Avoid silent publishing or changing account playlist visibility. Make persistence, export, and duplicate handling predictable.

### Make stats useful over time

Improve comparisons and filtering, help listeners rediscover neglected favorites, and refine recap layouts. Keep measurement definitions visible, label unknown metadata honestly, and preserve raw exportability. Automatic genre enrichment, historical imports, and cross-device sync need separate design and privacy decisions.

## Before the first packaged release

- Adapt inherited packaging and release checks to Moosik's identity and versioning.
- Verify clean builds, installation, login, playback, updates, and removal on each advertised platform.
- Establish stable macOS signing and a distribution/notarization path.
- Publish corresponding source, dependency notices, checksums, and clear platform limitations.
- Record a short walkthrough using a demo account or synthetic library. Current Moosik screenshots with synthetic listening history are in the README.

## Design principles

1. Artwork and useful controls take priority over explanatory text.
2. Hovering must not move the interface. Dragging must show exactly what will happen.
3. Common actions stay close to the song; advanced tools use progressive disclosure.
4. Local actions and changes to a YouTube account must be distinguishable.
5. Measure real listening conservatively. Never invent history or metadata.
6. Preserve Refryt's credit and contribute broadly useful fixes upstream where practical.
