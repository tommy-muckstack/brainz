# Brainz implementation tracker

Requested scope: all four specifications and the outstanding product requests.
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
- [ ] Investigate reusable Delta source and licensing; document any unavailable upstream source.

## Release basics

- [ ] P0-1. CI for Brainz: PR and `main` checks, required before merge
- [ ] P0-2. One version source and a changelog
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
- Next release: pending implementation and validation.
