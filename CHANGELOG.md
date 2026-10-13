# Changelog

## Unreleased

### New

- Preview local Markdown templates through the CLI, with ten built-ins, brain-folder overrides, and read-only rendering without an API key.

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

