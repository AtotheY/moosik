# Queue controls

Open **Queue** in the sidebar. The same editing controls also work in the compact
queue panel and the fullscreen queue drawer.

- Drag an upcoming song above or below another row; the accent line shows its new
  position. Drag near the top or bottom to scroll through a long queue.
- **Cmd-click** on Mac / **Ctrl-click** elsewhere toggles songs in a selection.
  **Shift-click** selects a range; adding Cmd/Ctrl extends an existing selection.
- Drag any selected row to move the selected songs together in displayed order.
- Right-click or use **•••** for **Play next**, **Move up**, **Move down**, and
  **Remove from queue**. A selection's menu applies to the entire selection.
- **Clear queue** clears upcoming songs and stops the current autoplay river from
  refilling them. It keeps the current song playing. **Undo** restores the edit.

| Shortcut | Action |
| --- | --- |
| Cmd/Ctrl+A | Select the currently loaded upcoming queue |
| Delete / Backspace | Remove selected songs |
| Alt+Up / Alt+Down | Move selected songs up / down |
| Cmd/Ctrl+Z | Undo the latest queue edit |
| Escape | Cancel dragging, or clear selection |

Shortcuts apply while the queue has focus or the pointer is over it, without
intercepting text editing. The existing click-to-play preference still applies;
modifier clicks and dragging never start playback. Artist/album links and the
artwork play button retain their ordinary unmodified actions.

Reordering changes the actual upcoming playback sequence and prefetch, including
under shuffle. It preserves the playing song and position and never changes the
source playlist. The drawn shuffle order is saved with playback so manual moves
survive relaunch. Older playback files remain readable.

Undo is one level. Later queue changes or moving to another playing song invalidate
it; undoing an edit does not rewind audio playback. A stale drag is cancelled if
the underlying queue changes. Dropping outside the queue or pressing Escape leaves
the queue untouched. Repeated appearances of the same song are separate entries.

## Song transfers (0.1.16)

Drag a regular song row from search, an album, a playlist, stats, or listening history
onto **Queue** in the sidebar to add it. The open queue also offers a drop zone, and
its upcoming list shows an insertion line for an exact position. Dropping into that
list preserves the current song, follows the displayed shuffle order, and supports Undo.

Drag songs onto an editable sidebar playlist to add copies. Private playlists accept
both streamed tracks and local files and skip duplicates; YouTube playlists accept
streamed tracks, and file playlists accept local files. Smart playlists follow their
rule instead of accepting manual additions. Queue selections transfer together;
Cmd/Ctrl-click and Shift-click also select groups in ordinary song lists.

Use **Save as private playlist** on the queue page to keep the current song and its
upcoming sequence. Choose a name in the dialog. It saves song references on this Mac,
without uploading a playlist to YouTube or downloading the audio.
