# Changelog

## Unreleased

### New

- Dropbox is in the connector catalog: add it from the MCP tab and sign in with your Dropbox account.

### Fixed

- Send a message with a screenshot even when reading its text takes too long: the composer waits at most six seconds for the OCR helper, then sends the image on its own instead of sitting in "Loading Added Context".
- Offer "Connect" instead of "Reconnect" when a Codex chat mentions a connector that only Claude has configured, so reconnecting no longer fails with "Codex has no connector named granola".
- Queued messages show their Send Now, Edit, and Remove controls again; the message text no longer pushes them off the row.

### Improved

- The canvas header keeps its menu chevron right after the title, and only mentions saving when a save fails.

## 0.1.41 (2026-10-13)

### Improved

- Review every outgoing commit before a sync publishes it. The check reads the exact staged content and each unpublished commit, including content removed later, and stops on credential filenames, secret markers, or files over 25 MB.
- Commit reviewed changes from a private index, so edits made during the review are kept locally and never published unreviewed; the sync stops if the branch or its commits change mid-review.

### Fixed

- Reject stale task toggles in the Markdown viewer instead of editing the wrong line, and keep read-only notes unchanged.
- Undo and redo through the Markdown viewer, show its dirty state in the tab, and leave untouched files unchanged on save.
- Keep trailing whitespace and the final-newline choice in Markdown notes when saving.
- Alt-click a Markdown link to open the source editor without the automatic preview taking over.

## 0.1.40 (2026-10-13)

### New

- Preview local Markdown templates through the CLI, with ten built-ins, brain-folder overrides, and read-only rendering without an API key.
- Create notes with a searchable native template picker, Markdown preview, calendar-event selection, and the existing folder hierarchy; open directly at the template caret.
- Open today's daily note without replacing previous edits, and create notes through `brainz new` with atomic no-overwrite saves.
- Insert templates and common blocks with `/` in Markdown notes, including file and person links; keep URLs and code literal and undo each insertion in one step.
- Prepare chat drafts from built-in or local templates through `//`, with input forms and preview; keep `/` for files and canvases and send only when ready.
- Rank note templates by folder defaults and recent use, configure the daily-note template, and apply automatic folder rules through the CLI.

- Open the Brainz launcher with Cmd+K to search templates and tools, or use Cmd+Enter to explicitly ask Claude.
- Offer folder templates in empty notes, with Use, Blank, Change, and Undo; fill eligible new notes on opening without replacing existing content.

### Improved

- Refresh template pickers when local templates or configuration change, preserving typed inputs and blocking invalid or deleted templates.
- Use meaningful area/file summaries and session identifiers for manual sync commits, without requiring an API key.

### Fixed

- Focus the chat composer when clicking its empty padding, including while a canvas is open.
- Keep template results clear of input fields and previews, with a stacked layout in narrow windows.
- Keep the canvas from covering the pane's + and split popover menus.
- Preserve newer local edits when updating after a GitHub merge, and stop pulls that cannot fast-forward safely without stashing or rebasing the live notes.

## 0.1.39 (2026-10-13)

### New

- Preview local canvas tools for Claude and Codex, including object relationships, screenshots, and reviewable edit proposals.
- Apply a canvas proposal as one undo step, with conflict checks that preserve intervening edits.
- Draw with shape, frame, mind-map, lasso, eraser, and laser tools; find tools through the searchable command menu.
- Keep brainstorming with Tab to add a sticky or mind-map child, and Enter to add a mind-map sibling.
- Paste screenshots directly onto the canvas, with local image text recognition available to assistants on macOS.
- Add connected objects through four quick-create dots or a keyboard-operated picker.

### Improved

- Run native and canvas regression checks automatically on proposed changes.
- Verify downloaded updates against their checksum, Brainz signing identity, and Apple notarization before installation.
- Choose Stable or Preview updates and read the installed version's release notes inside Brainz.
- Wait for a pause in typing before showing an update reminder.
- Keep inherited usage telemetry off, including with older opt-in settings.
- Manage upcoming Brainz features through local Labs switches.
- Merge external canvas changes and preserve a recovery copy when a file cannot be merged.
- Save pan and zoom positions locally without rewriting canvas files or adding Git changes.
- Read the live canvas selection and viewport through the assistant tools, with an explicit saved-context fallback for closed canvases.
- Preview incremental canvas saves and separate image storage, with migration backups, recoverable image deletion, and protection against compacting unseen external edits.
- Keep canvas actions in one selection toolbar, with object filtering, sticky text sizing, and file and link actions.
- Use floating link entry, searchable shortcuts, rounded elbow connectors, and Command to temporarily disable snapping.
- Preserve labels when moving from sticky editing to a new connected shape.

## 0.1.38 (2026-10-11)

### Improved

- Move canvases into existing folders with a native picker, fuzzy typeahead, and the sidebar's icons and hierarchy.
- Navigate folders with the keyboard, preview the destination, and see confirmation after moving.
- Use the shared Save icon in connector, agent, and keyboard settings and profiler exports.

## 0.1.37 (2026-10-11)

### New

- Copy selected canvas objects as PNG with equal padding, directly to the clipboard.
- Create canvases and move them into folders from the menu beside their name.

### Improved

- Show attachment dots whenever the Connector tool is selected.
- Keep connector names on one line with a compact reconnect action.
- Reference canvases through the chat slash picker and show a temporary confirmation after copying a link.

### Fixed

- Restore chat keyboard input after using the canvas.
- Preserve transparent fills, pending edits, and links when reopening or moving canvases.

## 0.1.36 (2026-10-11)

### New

- Draw boxes, sticky notes, text, freehand strokes, and connected lines on a persistent infinite canvas.
- Reference canvases in Claude and Codex with their visual snapshot and explicit object relationships.
- Embed images, add file cards and deep links, and capture areas beyond the visible canvas.
- Connect Railway through its hosted MCP server and OAuth.

### Improved

- Drag objects with a single click and edit with a double click.
- Fit PDFs to the pane with zoom controls and sharper receipt text.

