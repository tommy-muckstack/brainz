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
- [x] K3: meaningful area/file commit subjects and session identifiers without a model.
- [ ] K4–K7/K9: path-scoped commits, automatic syncing, branch support, event-driven refresh, and review before every commit.
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
- 0.1.40: published 2026-10-13. Templates, native creation, Markdown/chat
  insertion, folder defaults, live refresh, and the launcher are included, with
  safe fast-forward updates, readable commit subjects, canvas popover visibility,
  composer padding focus, and responsive template forms. All four CI jobs passed.
  The app and DMG passed Apple notarization and signature/executable checks.
  Native checks against the signed app verified the plus menu, unsent chat typing,
  template creation, and Connectors in a synthetic workspace. The GitHub asset
  digest, versioned download, stable download, and public feed were verified.
  [Installer](https://github.com/tommy-muckstack/brainz/releases/tag/v0.1.40)
- 0.1.41: published 2026-10-13. Markdown viewer safeguards (stale task
  rejection, viewer Undo/Redo and dirty state, untouched-file saves,
  whitespace-preserving Markdown defaults, Alt-click to source) and sync review
  of every outgoing commit. All four CI jobs passed; the app and DMG passed
  notarization. [Installer](https://github.com/tommy-muckstack/brainz/releases/tag/v0.1.41)
- 0.1.42: published 2026-10-14. A six-second OCR deadline so a pasted
  screenshot never holds a send in "Loading Added Context", Connect instead of
  Reconnect for connectors the current agent has not configured, queued-message
  Send Now / Edit / Remove controls kept on the row, a canvas header that hugs
  the title and reports saving only on failure, and Dropbox in the connector
  catalog. All four CI jobs passed on each PR; the app and DMG passed
  notarization, Gatekeeper accepts the DMG, and the GitHub asset, stable
  download, and public feed were verified.
  [Installer](https://github.com/tommy-muckstack/brainz/releases/tag/v0.1.42)
- 0.1.43: published 2026-10-14. Sync to GitHub merges GitHub's newer commits
  into the local brain before opening its pull request (overlapping notes are
  named, files left untouched, superseded sync pull requests closed), and a
  long paste in the chat composer becomes one pasted-text card with open,
  edit-as-text, and remove controls. All four CI jobs passed on each PR; the
  app and DMG passed notarization and the signature smoke check, and the
  GitHub asset, stable download, and public feed were verified. The card's
  look in the running app is not yet checked by eye.
  [Installer](https://github.com/tommy-muckstack/brainz/releases/tag/v0.1.43)
- 0.1.44: published 2026-10-15. Sync to GitHub keeps the pull request it just
  opened (0.1.43 closed it as superseded, so every sync stopped at the
  mergeable check). Covered by a unit test on the supersede predicate. All
  four CI jobs passed on the PR; the app and DMG passed notarization and the
  signature smoke check, and the GitHub asset, stable download, and public
  feed were uploaded. The first real sync on 0.1.44 is not yet confirmed.
  [Installer](https://github.com/tommy-muckstack/brainz/releases/tag/v0.1.44)
- 0.1.45: published 2026-10-22. Markdown notes open in a rich text editor
  (BlockSuite doc mode in a web view, saving only changed blocks back to
  Markdown), a Create menu in the sidebar, a Back up notes card in Connectors
  (Obsidian folder or Notion), and chat activity-row and attachment-indent
  fixes. Signing, notarization and stapling were verified; the DMG, stable
  download and feed were uploaded.
  [Installer](https://github.com/tommy-muckstack/brainz/releases/tag/v0.1.45)
- 0.1.46: published 2026-10-22. Save as template, Create > Note opening a
  blank note in the rich editor, New note listing only saved templates, and a
  spinning Create button. Signing, notarization and stapling were verified;
  the DMG, stable download and feed were uploaded.
  [Installer](https://github.com/tommy-muckstack/brainz/releases/tag/v0.1.46)
- 0.1.47: published 2026-10-22. Note images, tables, links, and history; agent canvas
  tools; Claude actions in notes; quick capture; and automatic naming for
  untitled notes. Chat turns Claude artifact and Brainz object links into pills;
  canvas titles have more room, and macOS clipboard screenshots paste onto
  canvases. Connectors offers Miro's hosted MCP server. Signing, notarization,
  stapling, bundled executable checks, and live download verification passed.
  [Installer](https://github.com/tommy-muckstack/brainz/releases/tag/v0.1.47)
- 0.1.48 candidate: Agent requests to open a file use Brainz directly on macOS.
  Publication and signed-artifact verification are pending.

## Work in progress

- Rich editor source-map foundation: transient block ranges preserve original
  Markdown bytes, including gaps and reference definitions. Validated edit
  plans change only requested ranges and reject stale snapshots or partial
  Unicode selections. Seven core tests cover 40 synthetic fixtures, exact prose,
  To-Do title and metadata edits, bold delimiters, nesting, and a 5,000-line
  document. The live Buffer, anchored selections, rendered editing surface,
  autosave, and native round-trip corpus gate are still required. E1 remains open.
- Native note creation: searchable template picker, preview, inline inputs,
  calendar-event selection, shared folder hierarchy, atomic no-overwrite saves,
  daily-note reopening, and direct edit-mode opening at the template caret.
  100 native/engine tests and the CLI creation checks pass, along with Clippy
  and all four Brainz CI jobs. Empty-note defaults are implemented below; skill integration remains open;
  this UI shipped in 0.1.40.
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
  Chat completion/composer checks now run in CI. Shipped in 0.1.40.
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
  remains open. Shipped in 0.1.40.

- Sync messages: manual commits describe the affected area and note, retain the
  configured author, and add a session trailer. Single-commit PRs reuse that
  subject. Optional AI summaries and agent attribution remain open. Shipped in 0.1.40.
- Sync safety: post-merge updates stop if new local work prevents a fast-forward;
  the destructive fallback is removed. Pulls no longer rebase or autostash in the
  live checkout, even if the user's Git configuration enables autostash. Manual
  sync can propose diverged histories through its existing PR flow. A direct
  pull of diverged commits pauses with guidance until isolated integration is
  implemented. Five local bare-origin tests cover clean pulls, staged/unstaged
  and untracked text, CRLF preservation, divergence, concurrent edits, and branch
  changes. No real brain repositories are used. Shipped in 0.1.40.
- Sync review: every outgoing commit is reviewed before publishing. The
  staged tree is captured with `write-tree` and reviewed from Git objects, not
  the working files, so edits made during review cannot slip into the commit.
  Each unpublished commit on main is reviewed too, including content removed
  by a later commit. Credential filenames, secret markers, and files over 25 MB
  stop the sync. Reviewed changes are committed from a private index, and the
  sync stops if the branch, HEAD, or a Git hook changes the reviewed tree. Six
  local fixture-repository tests cover nested names, binary and large content,
  staged-versus-working content, secrets removed later, renames, and branch
  changes. Shipping in 0.1.41.
- Canvas menu layering: native whiteboards yield while a GPUI popover is open,
  keeping the pane's + and split menus visible. Tracking is scoped per window
  and clears on dismissal, replacement, or release. Native regression coverage
  includes all four lifecycles and runs in Brainz CI. Shipped in 0.1.40.
- Native verification for 0.1.40 used a separate app data directory
  and synthetic workspace. The pane's + menu displays over a canvas; Escape
  restores it. Connectors opens and Railway has its logo. The launcher opens a
  template form and creates a note in the chosen folder. Template results no
  longer overlap inputs; geometry tests cover normal and narrow layouts.
  Composer padding accepts physical typing while a canvas is open. All 48 chat
  editor tests pass, including padding focus and controls; no test messages were
  sent and no personal notes or drafts were changed by these checks.
- Brainz launcher: Cmd+K opens grouped, searchable note templates and the shared
  navigation tools. It uses local template overrides, recent ordering, live
  catalog refresh, and the existing creation form. Escape restores the draft;
  Cmd+Enter explicitly asks Claude in a new conversation. Inherited Cmd+K chord
  prefixes are removed so the launcher opens immediately. Keyboard tests use
  the shipped macOS keymap and full workspace/modal host, with AI actions captured
  locally instead of sent. Shipped in 0.1.40.
- Follow-on Markdown safeguards: source-snapshot validation rejects stale task
  positions and read-only edits; accepted edits use one undo transaction. The
  viewer reports dirty state, supports Undo/Redo, and preserves untouched files
  on save. Default Markdown saving preserves whitespace and the final-newline
  choice. Alt-click reopens the source editor without automatic preview taking
  over. All 32 viewer tests pass, including 40 synthetic round-trip fixtures,
  CRLF save/undo, stale tasks, uppercase task markers, and existing navigation.
  These safeguards ship in 0.1.41; full inline editing remains open.

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
