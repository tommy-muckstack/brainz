# Brainz note editing core

This crate maps Markdown block ranges against the original text and prepares
small, validated source edits. It does not serialize a Markdown syntax tree.
Reference definitions, gaps, line endings, and unsupported syntax stay in the
original source. Blocks include nested list items and structured content.

An edit plan belongs to one snapshot. `edits_for` rejects a changed source;
the caller must apply its returned edits in one Buffer transaction without
awaiting between validation and application. Convert byte ranges to Buffer
anchors before retaining selections across edits. Block indices are transient.

The crate is not yet connected to the rendered note editor. Its 40 synthetic
fixtures must also run through the native Buffer/UI before editing is enabled.
Cursor behavior, native undo, autosave, inline formatting, structured widgets,
and painting latency remain integration work.

Run `cargo test -p brainz_editor` for source preservation, nested ranges, exact
typing/title/property edits, bold delimiters, stale edits, and Unicode checks.
