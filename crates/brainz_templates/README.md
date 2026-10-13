# Brainz templates

Local Markdown template parsing and phase-one rendering. This crate has no GPUI,
model-provider, or HTTP dependency. AI blocks become placeholders and separate
instructions; rendering does not call a model or write a note.

The ten built-ins live in `assets/brainz/templates`. Files in the brain's
`.brainz/templates` folder override matching IDs. Invalid files are reported
individually. Reloading the registry reads current files; the app's live watcher
and picker are separate integration work.

Set `[templates] dir = "Templates"` in `brainz.toml` to use another folder inside
the brain, or pass `--directory` for a single CLI invocation.

```sh
brainz templates list --workspace /path/to/brain --json
brainz templates show decision --workspace /path/to/brain
brainz templates render decision --workspace /path/to/brain \
  --folder projects/demo --input 'title=Choose a storage format' --json
```

`render` only prints the result. `--strict` rejects missing values. `--context`
accepts a JSON file (or `-` for stdin) for calendar, people, selection, and other
read-only context. `--mode insert` and `--mode append` omit the leading H1 and
note frontmatter. The JSON result includes the caret byte offset, proposed
relative path, missing inputs, AI placeholders, and the user-content guard.

Template paths reject absolute paths, parent traversal, and symlinks outside the
brain. Proposed names use numeric suffixes for existing notes. The eventual
write operation must still use atomic no-overwrite creation; proposing a name
does not reserve it.

Native creation, folder defaults, live context providers, slash completions,
skill-bundled templates, and AI review are not connected yet. The CLI leaves
those AI sections as placeholders. This is the template engine foundation,
not the complete templates-and-skills feature.
