# Brainz implementation tracker

Requested scope: all five specifications and the outstanding product requests.
This file records implementation and validation; an unchecked item is not shipped.

## Constraints

- Keep existing brain folders and files in place. Canvas files belong in the existing folder tree.
- Personal API keys are optional and stored securely. Existing Claude/Codex subscription chats keep working.
- Local search and deterministic workflows work without paid APIs.
- Use the shared Brainz icons, theme, typography, and native pickers.
- Canvas objects drag on single click and edit text on double click.
- Keep chat typing usable while canvases are open.
- Do not copy private notes, credentials, or the supplied research documents into this public repository.

## Shipped baseline: 0.1.38

- [x] Canvas, connected shapes, sticky notes, lines, persistence, deep links, and file cards.
- [x] Canvas relationships and images as chat context; slash references for canvases.
- [x] Selection PNG copying with uniform padding, connector dots, transparent fill, and shared expand/collapse icons.
- [x] Multiple canvases, existing-folder filing, native folder picker with fuzzy typeahead, and shared Save icon.
- [x] Chat focus fixes, simplified reconnect banners, Copy link feedback, and PDF rendering improvements.

## Additional requests

- [ ] Recheck connector installation directly from chat and Connectors navigation.
- [ ] Recheck Railway setup and connector-specific reconnect artwork.
- [ ] Rich Markdown editing inside the existing viewer, with comments, history, and collaboration.
- [x] Investigate reusable Delta source and licensing; document any unavailable upstream source.

## Rich editor and Git sync

- [x] K1/K2: remove destructive post-merge fallback and live autostash/rebase pulls.
- [ ] K3–K7/K9: session messages, path-scoped commits, automatic syncing, branch support, event-driven refresh, and review before every commit.
- [ ] E1. Lossless editing in the rendered Markdown viewer, source-range golden corpus, formatting, slash actions, tables, properties, paste, and block movement.
- [ ] E2. Autosave, session commits, optional AI messages, per-origin attribution, grace window, offline queue, and sync status pill.
- [ ] E3. Isolated integration, block conflicts, recoverable safety snapshots, and persisted resolution.
- [ ] E4. Staged Claude/Codex suggestions with partial acceptance, persistence, and attribution.
- [ ] E5. Changes since last reading with authors, navigation, and seen markers.
- [ ] E6. Note version history, fine-grained operations, and non-destructive restoration.
- [ ] E7. Block blame and agent attribution.
- [ ] E8. Anchored comment threads in sidecars.
- [ ] E9. Agent assignments through comments and note actions.
- [ ] E10. Local agent co-editing and future DeltaDB evaluation.

## Release basics

- [ ] P0-1. CI for Brainz: PR and `main` checks, required before merge
- [x] P0-2. One version source and a changelog
- [ ] P0-3. Update feed v2: verification, channels, release notes, rollout
- [ ] P0-4. Signed, notarized DMG in CI on tag
- [ ] P0-5. Turn off Zed telemetry; nothing goes to zed.dev by default
- [ ] P1-1. Crash reporting (opt-in, privacy-preserving)
- [ ] P1-2. Error telemetry (opt-in health signals)
- [ ] P1-3. Feature flags for unfinished work
- [ ] P1-4. Trim entitlements and permissions
- [ ] P1-5. Release checklist, smoke test and rollback
- [ ] P2-1. Fork hygiene
- [ ] P2-2. Universal or Intel builds
- [ ] P2-3. Release health in the Brief
- [ ] P2-4. EdDSA-signed feed
- [ ] P2-5. Nightly preview builds

## Canvas

- [ ] P0-1. Agent read/write API (ops, file watcher, change events)
- [ ] P0-2. AI on the canvas (palette, prompt bar, selection actions, staged preview, Mermaid)
- [ ] P0-3. Performance and persistence (format v2)
- [ ] P0-4. Core UX: expose the engine, quick-create, connectors, palette, paste
- [ ] P1-1. Brainz integration: note cards, drag-in, board ↔ markdown, capture, multiple boards, search, backlinks
- [ ] P1-2. Frames as slides, presentation, export
- [ ] P1-3. Comments and annotations for agent review
- [ ] P1-4. Smaller P1 items
- [ ] P2-1 Templates
- [ ] P2-2 Multiplayer and presence
- [ ] P2-3 Facilitation
- [ ] P2-4 Semantic clustering
- [ ] P2-5 Library
- [ ] P2-6 Native Mermaid

## Templates and skills

- [ ] P0-1. Template format, engine and built-in library
- [ ] P0-2. Insert surfaces: `⌘N` picker, `/` in notes, `⌘K` palette, folder defaults
- [ ] P0-3. AI-filled sections (staged)
- [ ] P0-4. Skill index, Library basics, and running skills in Claude/Codex tabs
- [ ] P0-5. The Brainz skill runner: inputs, steps, approvals, outputs, run log
- [ ] P1-1. Make your own: save as template, save chat as skill, record a skill
- [ ] P1-2. Library: gallery, preview, editing, versions
- [ ] P1-3. Canvas templates and skills on the canvas
- [ ] P1-4. Triggers and suggestions (calendar, schedule, MyMan, folders, Brief)
- [ ] P2-1 Sharing and packs
- [ ] P2-2 Skill evals
- [ ] P2-3 Autorun
- [ ] P2-4 Semantic related notes
- [ ] P2-5 Snippets
- [ ] P2-6 Analytics

## Search and Related content

- [ ] P0-1. The brain index: extract, chunk, embed, store, watch
- [ ] P0-2. Semantic search panel
- [ ] P0-3. Related panel: suggested content for whatever you're viewing (the hero feature)
- [ ] P0-4. Agent context: `brainz search`, `brainz related`, MCP tools
- [ ] P0-5. Feedback, evaluation, privacy controls and repair
- [ ] P1-1. More context sources: canvas selection, PDF page, events, chat threads, live buffer
- [ ] P1-2. Auto-attach related context to new chats (opt-in, cited)
- [ ] P1-3. The same engine in more surfaces
- [ ] P1-4. Unlinked mentions → links
- [ ] P2-1. OCR for scanned PDFs and images
- [ ] P2-2. Chat history as a source
- [ ] P2-3. Learned ranking
- [ ] P2-4. Semantic layout on the canvas
- [ ] P2-5. Brain hygiene signals
- [ ] P2-6. Multilingual model
- [ ] P2-7. Index access for other bots

## Validation and release record

- 0.1.38: native picker interactions, canvas storage tests, icon checks, production build, in-app picker check, and Apple notarization passed.
- 0.1.39: published 2026-10-13. Both the app and DMG passed Apple notarization,
  signatures, and bundled executable checks. The GitHub and download-service
  artifacts were verified against the local DMG; the public update feed matches.
  Native checks in the existing brain verified Canvas and Connectors opening,
  Claude and Codex typing, and Copy link feedback. No chat messages were sent.
  The first unpublished candidate exposed a workspace lease panic; the corrected
  candidate also fixes a concurrent directory-creation race. 95 native tests,
  Clippy, and all four Brainz CI jobs passed (one existing native test ignored).
  [Installer](https://github.com/tommy-muckstack/brainz/releases/tag/v0.1.39)
- Next update: template engine, native creation, Markdown slash insertion, and chat template insertion merged; folder defaults and live refresh are being validated.

## Work in progress

- Native note creation: searchable template picker, preview, inline inputs,
  calendar-event selection, shared folder hierarchy, atomic no-overwrite saves,
  daily-note reopening, and direct edit-mode opening at the template caret.
  100 native/engine tests and the CLI creation checks pass, along with Clippy
  and all four Brainz CI jobs. Empty-note default banners and skill integration remain open;
  this UI has not shipped in an installer yet.
- Markdown slash insertion: templates, dated callouts, tasks, dates, tables,
  Mermaid, and brain file/person links. Required-input forms preserve the source
  note, reject changed or read-only targets, and apply as one undo step. URL,
  code-fence, inline-code, Unicode, link-escaping, caret, and native insertion
  checks pass. AI actions and skills remain separate work.
- Chat template insertion: `//` searches built-in/local templates by name,
  description, and tags in a Brainz group alongside the existing agent commands.
  The shared picker prepares an editable draft without sending; cancellation
  preserves the original draft. Twelve chat-completion tests and three shared
  native creation/insertion tests pass, as do all 47 message composer tests.
  Chat completion/composer checks now run in CI. This has not shipped in an installer.
- Template defaults: shared configuration and glob precedence, custom daily IDs,
  folder-first/recent-first native ordering, and per-brain recent IDs in local
  app data. Changing the folder preserves the selected template and typed inputs.
  CLI creation uses automatic rules and respects explicit overrides; suggestion
  rules keep the new note blank. 88 native tests and 19 engine tests pass (one
  existing ignored test), along with three CLI template tests.
- Live template refresh: the open picker watches its configured directory and
  configuration. Reloads preserve selection, typed inputs, and input focus.
  Invalid or deleted selected templates block creation until repaired or replaced.
  A FakeFs watcher test verifies edits, deletion, restoration, invalid configuration,
  and switching the watched directory; five native picker/insertion tests pass.
- Empty-note defaults: native editor bars offer Use, Blank, Change, and Undo.
  Newly created notes can apply an automatic rule when opened; existing empty
  notes only offer a suggestion. Initial worktree scans do not arm automatic
  insertion. Typing, external buffer edits, and read-only state prevent replacement;
  required inputs use the shared form. Links resolve from the actual note path.
  90 native tests pass (one existing ignored), plus the expanded interaction
  check for Undo/reopen and read-only buffers; Clippy passes. Closed-file filling
  remains open. This has not shipped in an installer.

- Sync safety: post-merge updates stop if new local work prevents a fast-forward;
  the destructive fallback is removed. Pulls no longer rebase or autostash in the
  live checkout, even if the user's Git configuration enables autostash. Manual
  sync can propose diverged histories through its existing PR flow. A direct
  pull of diverged commits pauses with guidance until isolated integration is
  implemented. Five local bare-origin tests cover clean pulls, staged/unstaged
  and untracked text, CRLF preservation, divergence, concurrent edits, and branch
  changes. No real brain repositories are used. Not yet released.
- Canvas menu layering: native whiteboards yield while a GPUI popover is open,
  keeping the pane's + and split menus visible. Tracking is scoped per window
  and clears on dismissal, replacement, or release. Native regression coverage
  includes all four lifecycles and runs in Brainz CI. Not yet released.
- Brainz launcher: Cmd+K opens grouped, searchable note templates and the shared
  navigation tools. It uses local template overrides, recent ordering, live
  catalog refresh, and the existing creation form. Escape restores the draft;
  Cmd+Enter explicitly asks Claude in a new conversation. Inherited Cmd+K chord
  prefixes are removed so the launcher opens immediately. Keyboard tests use
  the shipped macOS keymap and full workspace/modal host, with AI actions captured
  locally instead of sent. This has not shipped in an installer.

- Delta source check (2026-10-13): Zed's official
  [Delta packaging repository](https://github.com/zed-industries/delta-nix)
  explicitly identifies Delta as proprietary. Its public packaging code is not
  the application source. Rich note editing, comments, history, and collaboration
  remain implementation work in the existing Brainz Markdown viewer.
- Template foundation: ten built-ins, YAML metadata, Markdown helpers, AI
  placeholders, local overrides, configurable folder loading, and read-only
  `templates list/show/render` commands. Watcher integration,
  skill templates, live context providers, and AI review remain open within P0-1
  and the following epics. The engine's 15 local tests (including ten reviewed
  render fixtures), five CLI tests, and Clippy pass. A separate CLI smoke test
  verifies overrides, strict rendering, missing-input errors, and zero writes.

- Release foundations PR #58 adds CI, verified updates, stable/preview settings,
  offline notes, private defaults, local Labs, and a shared release pipeline.
- Local foundation checks: 22 updater tests (including wrong-signature rejection),
  three update notification tests, 22 client tests, nine flag tests, and three
  release-tool tests pass. The published 0.1.38 app and DMG pass the new smoke check.
- Remote CI: canvas, Linux, macOS, and workflow lint pass. macOS uses Xcode 16.4
  after Xcode 15.4 failed to compile the upstream WebRTC dependency.
  Repository protection,
  inherited-workflow disabling, release secrets, and a clean signing rehearsal
  remain owner setup and validation tasks.
- Canvas agent preview: local CLI/MCP list/read/apply/status/events/screenshot,
  per-brain connector setup, schema-backed batches, staged Apply/Discard,
  one-step undo, conflict checks, external Yjs merge, and durable recovery copies.
  The full Canvas P0-1 epic remains open for the remaining operation vocabulary
  and streamed protocol. The initial preview shipped in 0.1.39.
- CLI list/read/status/apply and stdio MCP smoke passed using a temporary brain.
- Canvas planner and existing helpers: 26 unit tests pass. Native canvas/Brainz
  tests: 83 passed, one existing ignored test. Clippy passes. Browser coverage
  includes staged edits, one-step undo/redo, rollback, screenshots, external
  merge/recovery, styling conflicts, frames, and grouping alongside the existing
  interaction suite.
- Persistence follow-up: local viewport storage, zero document writes while
  panning, one-time font migration, and live assistant reads with a labeled
  saved-context fallback.
- Format-v2 storage preview: in-place migration with exact backups, append-only
  updates, guarded compaction, external images, seven-day image trash, per-board
  locks, and independent recovery copies. Browser coverage verifies migration
  and reopening without redundant saves. Worker serialization, the incremental
  scene cache, thumbnails, and the native large-board open-time benchmark remain
  open, so the full P0-3 epic is not complete.
- Drawing tools follow-up: keyboard shortcuts, repeat-tool lock, grouped command
  search, immediate text entry, sticky and mind-map keyboard navigation, laser,
  image paste, and local macOS OCR. The native OCR helper is separate from the
  optional AI providers. Full P0-4 remains open for contextual styling,
  quick-create, and connector polish.
- Drawing validation: 34 browser checks, 29 unit checks, 81 native checks (one
  existing ignored test), the production web build, and native Clippy pass.
- Interaction follow-up: four quick-create dots for shapes and stickies,
  keyboard choices after dragging a connection, rounded elbows, live connection
  rerouting, interior endpoint binding, and Command to disable snapping.
  Selection actions now share one toolbar, with object filtering and sticky
  Auto/Small/Medium/Large text. Link entry floats without blocking the canvas;
  commands, right-click actions, and the shortcut sheet share one registry.
  Curved-line midpoint controls, sketch recognition, and full arrangement
  controls remain open within P0-4.
- Interaction validation: 39 browser scenarios pass (38 in the full run, plus
  the updated file-menu test), 29 unit tests pass, and the production build
  passes. Coverage includes saved quick-create labels, creation undo, sticky
  sizing after reopening, interior bindings, and rerouting during a drag.
