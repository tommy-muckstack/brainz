# Changelog

## Unreleased

### Improved

- Canvas: the collapsed drawing menu keeps the last drawing tool available as a one-click action beside its expand button.

### Fixed

- Canvas: Command-C and Command-V copy selected objects through the native clipboard, including connected objects pasted into another canvas.
- Canvas: typing into a new text object keeps the first character and no longer triggers drawing shortcuts.
- Canvas: closing the command menu restores keyboard focus, and leaving a shape label no longer overwrites a subsequent selection.

## 0.1.57 (2026-10-29)

### Improved

- The app icon leaves more room around the brain, so it no longer runs into the edges.

### Fixed

- A new note from Create → Note that you close without writing anything is no longer left behind as an empty "Untitled" file.

## 0.1.56 (2026-10-28)

### Added

- To-Do: drag items to reorder them or move them between sections.
- To-Do: Show done / Hide done in the header hides checked items everywhere (hidden by default).
- To-Do: set a High, Medium or Low priority on any item; it is saved as `[P1]`/`[P2]`/`[P3]` in the board file.
- To-Do: click an item's title to edit it in place (Enter or click away saves, Esc cancels).
- To-Do: New to-do in the header and an Add a to-do row under each section add items without leaving the tab.

### Fixed

- A new brain with no to-do file now opens the To-Do tab on an empty Today / Waiting / Done list instead of an error; the file is created when you add the first item.

## 0.1.55 (2026-10-23)

### Improved

- Sidebar notices have a quiet divider above Create.
- The canvas drawing tray fits all tools on shorter windows without internal scrolling.
- Create menu icons briefly animate as you hover each option.
- Canvas Undo and Redo are smaller and sit beside Add file in the header.

## 0.1.54 (2026-10-23)

### Improved

- MyMan meeting notes show the MyMan logo and update their pill title when the Markdown heading becomes available. Wispr Flow note pills now show the Wispr Flow logo.

## 0.1.53 (2026-10-23)

### Fixed

- GitHub sync no longer mistakes AWS-shaped text inside encoded canvas assets for a standalone access key ID.

## 0.1.52 (2026-10-23)

### Improved

- Create canvases in any brain folder. Editing a canvas title now renames its file in the sidebar, and an existing untitled canvas with a saved title is corrected when opened.
- The canvas tool tray starts compact, expands on hover or click, and shows shortcut letters with each tool's name on hover.
- Shapes briefly settle and glow when their edges align during a drag; dragging farther still moves past the snap point.
- Canvases now use incremental storage by default. Existing canvases are backed up during migration, and typing saves small updates instead of repeatedly serializing the entire board. View position saves wait until scrolling settles.

### Fixed

- Copying and pasting selected canvas objects with Command-C and Command-V duplicates them instead of invoking the image paste fallback.
- Normal external canvas merges no longer show distracting save and merge toasts.
- GitHub sync checks for a complete AWS access key ID, avoiding false alarms from incidental `AKIA` text in encoded canvas data.
- The macOS app icon now has a light brain mark on a dark background in dark appearance. The legacy icon remains visible on both light and dark desktops.

## 0.1.50 (2026-10-22)

### New

- Create a new local brain from the welcome screen, File menu, or an empty project panel. Brainz creates a folder with an Inbox, a Canvases folder, and a local Git repository on `main`; connecting GitHub is optional.
- Open Claude from the welcome screen, an empty agent panel, or the launcher without opening or cloning a repository first.

## 0.1.49 (2026-10-22)

### Fixed

- Check for Updates now explains when Brainz is running from a development build and offers the current public installer instead of silently doing nothing.

## 0.1.48 (2026-10-22)

### Fixed

- When asked to open a note or file, Claude and Codex in Brainz use Brainz directly on macOS instead of relying on the system's default app for Markdown files.
- Calendar access is requested only when you click Allow Calendar Access in the Calendar tab; background refreshes no longer reopen the macOS permission prompt.

## 0.1.47 (2026-10-22)

### New

- Notes show images: a picture on its own line (`![alt](path)`, or `![alt|300](path)` for a width) appears as an image. Paste or drop an image into a note and Brainz saves it in an `attachments` folder beside the note; resizing keeps the width.
- Hover a table or image in a note for Copy. Tables paste into Google Sheets or Excel as rows and columns with bold and italic kept; images copy as pictures.
- Type `[[` in a note to link another note from a list of the brain's notes. Each note shows a "Mentioned in" list of the notes that link to it.
- Claude and Codex in Brainz can work with canvases out of the box: create a canvas, add stickies, shapes, text, frames, cards, arrows and images from the brain, all as proposals you apply on the canvas. They also follow Brainz's conventions for note images, tables, links between notes and templates.

- Select text in a note for a Claude menu: Rewrite clearer, Summarize below, Turn into to-dos (added to your To-Do board) or Ask Claude. Claude works in the chat and edits the note itself, using your Claude sign-in.
- Quick capture: press Control-Option-N in any app to jot a thought; Enter adds it to Inbox.md in your brain under today's date.
- A new untitled note offers to take its first heading as its file name, and links to it from other notes are updated.
- Connectors includes Miro, using Miro's hosted MCP server and OAuth sign-in to work with boards, diagrams, documents, and comments.
- Notes made from a template open in the rich editor.
- Every note has a History button: see each synced version of the note, read it, and restore it in one click (Undo in the note takes the restore back).

### Changed

- The Canvas assistant setting is gone; canvas tools are always on and connected for every brain.
- Claude artifact links and Brainz note and canvas links appear as named pills in chat, while retaining their original destinations.

### Fixed

- Canvas names have more room in the title bar and show the full name on hover.
- Screenshots copied to the macOS clipboard paste onto canvases, including when the embedded web view omits image data from its paste event.

## 0.1.46 (2026-10-22)

### New

- Save as template (command palette) turns the open note into a template you can pick from New note. The note's own text, including any `{{` braces, is kept as written; New note asks for the new note's title.
- Create → Note opens a blank "Untitled" note in the current folder, straight in the rich text editor.

### Changed

- New note and the `/` menu list only your saved templates; the bundled examples (1:1, Bug report, Meeting notes and the rest) are gone. Open daily note still works.
- The Create button's plus spins into a cross while its menu is open and back when it closes.

## 0.1.45 (2026-10-22)

### New

- Markdown notes open in a rich text editor: type straight into headings, lists, checklists, quotes, code and tables, and Brainz saves Markdown back, rewriting only the blocks you changed. Cmd-Shift-M or the Markdown button switches to the source and back, and links to other notes open them in Brainz.
- A yellow Create button at the top of the sidebar makes a new canvas, note, folder, to-do (under Today) or Google Calendar invite.
- Connectors has a Back up notes card: choose Obsidian or Notion and Brainz keeps a second copy of every note there, updated after each sync or with Back up now. Obsidian gets the Markdown files in a vault folder (Obsidian's iCloud folder by default); Notion gets a private "Brainz backup" page with a page per folder and per note, through a personal access token you paste once.

### Fixed

- Chat activity rows ("Ran N commands") line up with the agent's reply bubble, and composer text no longer starts indented after attaching many images or files.
- Arrow keys in the canvas's text no longer type stray characters.

## 0.1.44 (2026-10-15)

### Fixed

- Sync to GitHub no longer closes the pull request it has just opened. Since 0.1.43 every sync ended with "GitHub hasn't finished checking the pull request" after the superseded-request cleanup matched the new request itself; only older sync requests that the new one includes are closed now.

## 0.1.43 (2026-10-14)

### New

- A long paste in the chat composer becomes one pasted-text card above the text, like a screenshot, instead of filling the composer. The card shows the first lines; click it to read the whole thing in a tab, use the pencil to turn it back into editable text, or hover for Remove. The agent receives every line, and sent messages keep the card when the agent accepts embedded resources (Claude and Codex do).

### Fixed

- Sync to GitHub merges GitHub's newer commits into the local brain before opening its pull request, so a note changed in both places no longer leaves a conflicting pull request behind. An overlapping edit now names the note and leaves every file untouched, and pull requests from earlier attempts are closed instead of duplicated.

## 0.1.42 (2026-10-14)

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

