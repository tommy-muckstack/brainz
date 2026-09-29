# Brainz icons

Put custom SVGs here. `crates/icons/src/icons.rs`, in `IconName::path`, is the
central mapping from app icons to these files. Unmapped icons use upstream art.

Use a `24 24` viewBox and `currentColor` so icons follow the active theme.
The supplied search artwork is `search.svg`, mapped to `MagnifyingGlass`.
The supplied folder artwork is `folder.svg`, mapped to `Folder` and `FolderOpen`.
The default file-tree theme in `crates/theme/src/icon_theme.rs` also uses it for
expanded and collapsed folders.
The supplied terminal artwork is `terminal.svg`, mapped to `Terminal` and
`TerminalAlt` for the tabs and panel button.
The supplied arrows are `back.svg` and `forward.svg`, mapped to `ArrowLeft` and
`ArrowRight`. Disclosure chevrons keep their upstream artwork.
The supplied check mark is `check.svg`, mapped to `Check`.
The supplied file artwork is `file.svg`, mapped to the generic file, document,
Markdown, and book icons. The default file-tree theme uses it for generic files
and Markdown documents, including preview tabs.
The supplied copy artwork is `copy.svg`, mapped to `Copy`. The supplied
expand and collapse artwork is `expand.svg` and `collapse.svg`, mapped to
`Maximize` and `Minimize` (message editor and panel full-screen toggles).
The supplied rocket artwork is `launch.svg`, mapped to `Launch`, used by the
bottom panel's Launch button (Shell, Claude, Codex).
The supplied paper plane is `send.svg`, mapped to `Send` (the composer's send
button). `ellipsis.svg` (vertical dots) maps to `Ellipsis`, and `sliders.svg` maps to
`BrainzSliders`, the composer's model/mode options toggle next to the plus. `trash.svg` maps to `Trash` (sidebar delete), `plus.svg`/`close.svg`
to `Plus`/`Close`, `files.svg` to `FileTree`, `split.svg` to `Split`,
`calendar.svg`/`mcp.svg`/`launch.svg` to the Brainz status-bar buttons, and
`claude.svg`/`codex.svg` (from the ACP registry) to `BrainzClaude`/`BrainzCodex`.
The supplied rounded square, filled, is `swatch.svg`, mapped to `Swatch`: the
tab colour swatch in the bottom panel's tab strip and its right-click menu.
