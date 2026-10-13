# Brainz templates

Local Markdown template parsing and phase-one rendering. This crate has no GPUI,
model-provider, or HTTP dependency. AI blocks become placeholders and separate
instructions; rendering does not call a model or write a note.
The separate creation API publishes a complete Markdown file atomically without
overwriting existing notes. Daily-note creation reuses an existing path.

The ten built-ins live in `assets/brainz/templates`. Files in the brain's
`.brainz/templates` folder override matching IDs. Invalid files are reported
individually. Opening the native picker reloads current files; live refresh while
the picker remains open is separate integration work.

Set `[templates] dir = "Templates"` in `brainz.toml` to use another folder inside
the brain, or pass `--directory` for a single CLI invocation.

```sh
brainz templates list --workspace /path/to/brain --json
brainz templates show decision --workspace /path/to/brain
brainz templates render decision --workspace /path/to/brain \
  --folder projects/demo --input 'title=Choose a storage format' --json
brainz new --workspace /path/to/brain --template decision \
  --folder projects/demo --input 'title=Choose a storage format'
brainz new --workspace /path/to/brain --daily
```

`render` only prints the result. `--strict` rejects missing values. `--context`
accepts a JSON file (or `-` for stdin) for calendar, people, selection, and other
read-only context. `--mode insert` and `--mode append` omit the leading H1 and
note frontmatter. The JSON result includes the caret byte offset, proposed
relative path, missing inputs, AI placeholders, and the user-content guard.

Template paths reject absolute paths, parent traversal, and symlinks outside the
brain. Proposed names use numeric suffixes for existing notes. `create_note`
atomically reserves and publishes the file; proposing a name does not reserve it.

Native creation uses a searchable picker with inline inputs, a Markdown preview,
calendar-event choices, and the existing folder hierarchy. New notes open in edit
mode at the template caret; daily-note reopening leaves previous edits intact.
The CLI prints the created path and does not launch a model or open a window.

Folder defaults, the full context-provider registry, slash completions,
skill-bundled templates, and AI review remain open. AI sections stay as
placeholders. This is not the complete templates-and-skills feature.
