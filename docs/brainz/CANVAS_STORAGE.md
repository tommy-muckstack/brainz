# Incremental canvas storage (preview)

Enable **Settings → Labs → Incremental canvas storage** and reopen a canvas to
try this preview. This work is not included in the 0.1.38 installer. It does not
require an API key or a cloud service.

Canvas files remain in the folders you chose. A version 2 `.brainzboard` file is
a small manifest; its content lives in `.brainz/boards/<id>/` in the same brain.
Move the visible file within that brain using the sidebar or the canvas folder
picker. When backing up or syncing the brain, include the hidden `.brainz`
directory as well as the visible files. Copying only a version 2 manifest to
another brain does not copy its content.

Opening an older canvas with this preview enabled upgrades it after the editor
successfully loads its contents. Brainz preserves the original bytes in a
neighboring `.brainzboard.v1.bak` file. If that backup already contains another
version, Brainz creates a uniquely named backup. Newer file formats are refused
without modifying them. Turning the preview off leaves upgraded canvases
readable and writable; it prevents further automatic migrations.

## Persistence

The storage directory contains:

- `board.json`: the board identity, title, engine schema and image metadata.
- `doc.ystate`: a binary Yjs snapshot.
- `doc.yupdates`: length-prefixed binary updates appended with a durability sync.
- `assets/<sha256>.<extension>`: images stored once by content hash.
- `canvas.json`: readable assistant context, refreshed after an idle interval.
- `.lock`: the operating-system lock that prevents competing Brainz windows.

Small edits append updates without encoding the entire board or embedding its
images again. Image uploads are durable before the corresponding block is
created. PNG, JPEG, WebP and GIF images can be up to 25 MB each. Brainz warns
when the images on a board exceed 200 MB.

Logs compact after 2 MB or 500 records, and on page close when all edits are
acknowledged. A checkpoint is accepted only for the exact current storage
stamp. External edits must be merged in the open editor before compaction can
proceed. An interrupted append retains every complete record; a crash between
the snapshot replacement and log truncation safely replays duplicate Yjs
updates.

Compaction moves unused images to `assets/.trash/`, where they remain readable
for undo. Later compactions remove images after seven days. Local locks, the
request cache, and image trash are excluded from Git. Pan and zoom positions
remain in the per-machine state file outside the brain.

Unmergeable external files are preserved. Recovery copies have their own board
identity and include the original image assets. They appear next to the canvas
in its existing folder.

## Validation and remaining work

Tests cover interrupted appends, stale checkpoints, external edits between an
append and checkpoint, exact migration backups, scoped asset reads, duplicate
window locks, recovery images, image trash, and small updates with 30 MB of
external assets. Browser tests migrate text, connectors and images; reopen the
result without another content save; and verify that later edits send small
updates.

The broader performance work remains open: worker-based serialization, an
incrementally maintained scene cache, thumbnails, and the complete 2,000-object
plus 30-image open-time benchmark in the native macOS app. The derived context
is currently rebuilt during idle time. The small-update tests measure document
persistence separately from that derived context refresh.
