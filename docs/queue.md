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

These controls move songs within the current queue. Dragging between Library,
playlists, and the queue is outside this change.
