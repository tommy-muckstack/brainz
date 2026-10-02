> [!IMPORTANT]
> Remove this line to confirm you've reviewed this PR before submitting.

# Brainz

A personal desktop workspace for a Markdown notes repo (your "brain"), built from Zed. Files on the left,
Markdown reading in the center, and a terminal below, with Gellix typography and
a charcoal-and-amber theme.

**Install:** download the signed, notarized build from
[muckstack.com/download/brainz](https://muckstack.com/download/brainz) (or the
DMG on the [releases page](https://github.com/tommy-muckstack/brainz/releases)),
drag Brainz to Applications, and open it. Packaged builds check
`brainz-latest.json` on the download host once an hour and install updates in
place, the same mechanism Zed uses, so end users never rebuild anything.
When an update is ready, a persistent notification in the bottom-right of each
Brainz window offers **Update & Restart** or **Later** (remind me in one hour).
Restart uses the normal unsaved-work prompts; cancelling keeps the update available.

**Build from source:** run `./script/brainz-local` on macOS to build and open
`~/Applications/Brainz.app`. Source builds never auto-update.
The first build requires Rust 1.98.1, Xcode, and CMake. The bundled UI font is
DM Sans (SIL Open Font License, in `assets/fonts/dm-sans/`). Gellix is a
commercial font, so its files are excluded from Git; drop them into
`assets/fonts/gellix/` and set `"ui_font_family": "Gellix"` in
`~/.config/brainz/settings.json` to use it.

Brainz uses `~/.config/brainz` and `~/Library/Application Support/Brainz`. Edit
`~/.config/brainz/themes/brainz.json` to adjust colors live, or
`~/.config/brainz/settings.json` to change the layout and terminal preferences.

The bottom panel's **Launch** button (rocket) opens a Shell or a native Claude or
Codex conversation in the current project. Everything you open becomes a tab in the
strip under the toolbar, shells and conversations side by side. Click a tab to
switch, use its close button to drop it, and click its icon (or right-click the tab) to give it a color
(a filled rounded square replaces the tab icon and the provider icon in thread lists; twelve
colors from Amber to Black, and message bubbles pick dark or light text to match).
Tabs and colors are remembered between launches. Conversations and the message
box use Gellix with natural spacing. Shell content uses Lilex for a fixed
character grid. The message box starts one line tall, including in a new chat,
and grows as you type.
Paste or attach multiple screenshots to collect them in one horizontally scrollable
thumbnail strip above the text before you send.
Pasted Google Doc, Granola, Wispr Flow, and GitHub links become clickable title
pills in the composer and sent messages (Granola shows its logo; GitHub pills read
`owner/repo #33`, `owner/repo@abc1234`, or `owner/repo · path`, with the pull
request or issue title once it loads). Titles load in the
background from the page; when a service exposes none, the pill stays labeled
Google Doc, Granola notes, or Wispr Flow notes, and the original link is preserved
for the agent and browser. A pasted path to a file on this Mac becomes a pill with
the file name that opens it on click; the agent still gets the path.
Named links in chat replies also render as rounded, clickable pills, preserving
their displayed titles and original file or web destinations.

The sun button at the far left of the status bar opens the **Brief** tab: today's
calendar events with who they match in the brain and an **Open prep** or **Open
folder** button, every folder whose status callout is older than its newest note,
and the ten oldest open loops, with a link to the full Open loops tab. It refreshes
on open and every five minutes. **⌘⌃V** anywhere in the window pastes the
clipboard (a screenshot, a link, a path) into the active conversation's message box
and focuses it, opening a Claude conversation first if none is up. Right-click an
amber folder in the tree for **Refresh status from newest notes**, which opens a
Claude tab pre-filled to rewrite the callout. The first Claude conversation offers
**Set standing permissions**, which writes Brainz's own Claude rules (edits in the
brain accepted; read, search, git, and connectors never ask; destructive shapes
still prompt) so routine work stops asking.

The calendar button in the status bar (bottom left) opens a **Calendar** tab
with the next seven days, read from every account macOS Calendar knows about.
It uses a small bundled helper built from `script/brainz-calendar.swift`, the
standard EventKit approach. macOS asks for Calendar access the first
time; the tab refreshes every five minutes and on demand. Sent messages sit on
the right in the tab's colour; replies sit on the left. Hovering a reply or a
quoted draft shows a copy button that rides along the top of the visible part
of the text while you scroll, so a long email never needs scrolling back up.
Plain-text and code blocks use the same sticky copy control and keep the
"Copied" confirmation visible briefly even after the pointer moves away.
Hovering one of your own messages shows a send-again button that posts the
same text as a new message. The options drawer behind the sliders button has
a reset button that starts a fresh conversation with the same agent in the
same tab (position and colour kept) without reconnecting; the old
conversation stays in the sidebar. When an agent is waiting for permission
while its tab is out of view, the corner popup has a **Yes** button that
approves that one tool call in place, next to View and Dismiss. Routine popups
dismiss after five seconds; permission and input requests stay visible.
While an agent is working, the send button's hover menu has a clickable
**Send Immediately** action. The message box says
"Type message…", grows as you type, and keeps model and mode options behind the
gear button. The minus and plus beside Send shrink or grow the chat text one
pixel at a time and save the size in settings.

The checkbox button next to the calendar opens a **To-Do** tab over the
brain's check-off board (`TODO.md` at the root, or the `todo` path in
`brainz.toml`). Sections are `##` headings, items are `- [ ]` lines, and Done
folds away; it refreshes every 30 seconds. Ticking an item checks it in place
with today's date and shows it struck through in amber, so nothing vanishes;
un-ticking clears the box. Whoever maintains the board tidies Done later.

The trend-line button next to the MCP button opens a **Themes** tab: what the
brain has been about, computed from its git history. **Sync** (or a daily
run while Brainz is open) walks every commit, takes the added lines of `.md`
files, and turns bold spans, wiki links, the names of people and companies
in the repo, and capitalized phrases into weighted themes with a 12-week
line chart and momentum. The tab shows Rising, Fading, and Pinned themes.
The sliders button to the left of Sync filters People, Places, and Things;
only Things (topics, projects, and organizations) are shown by default.
People and places are identified from their configured folders and labeled
note context, including interview headings, attendees, and locations.
Filters are remembered on this computer. The generated files also retain
new themes, cross-folder threads, open loops, and the bot-written narrative.
Expand a theme for its top files and co-mentioned people; click either to
open it. Pin, Rename, Merge into…, and Hide append a line to
the themes folder's `pins.md` and re-run the pass, so nothing is ever
deleted from `signals.json`. Output lands in the themes folder of the working tree and the
Sync banner carries it to GitHub like any other change.

The hourglass button next to Themes opens an **Open loops** tab: every current
`⏳` and `⏰` line in the brain, grouped by owner (`⏰` is owed by you, `⏳` is
waiting on someone else), oldest first, with the counterparty when a known
person is named on the line and the age from `git blame`. Click a row or
**Mark done** to open the file at that line; Brainz never edits the line, you
strike it. Lines carrying those glyphs (or `⚠️`) are kept out of theme
extraction, and the Themes status callout carries the "N owed, M waiting,
oldest" summary. The same signals pass drops terms whose only sources are
itinerary, roster, logistics, or prompt folders (`themes.noise_dirs`) or a
single file, caps momentum at `x20+`, and, when a theme is expanded, shows an
all-time line bucketed by month under the windowed one (`themes.window_weeks`,
default 12).

**Calendar-aware prep.** Ten minutes before an event whose full attendee
names, attendee email domains, or title match a company, client, or project
folder (the children of `calendar.match_dirs`, matched against the folder's own
notes and the people files), an amber banner above the file tree says "Grace
Hopper, 3:00pm." with **Open prep** (opens the day's `*-prep.md` from the
matching dated subfolder and any Google Doc it links), **Open folder**, and
**Dismiss**. A first name alone never matches, and two folders tied on the same
evidence match nothing. The banner stays until the event ends. Logging the call
afterwards is the agent's job: paste the Granola, Wispr Flow, and My Man sources
into a conversation and the brain's granola-to-brain skill takes it from there.

**Screenshots are read for you.** Every image you attach is run through the
bundled Vision OCR helper (`script/brainz-ocr.swift`) on your Mac as soon as it
lands in the strip, and the recognized text travels with the image as an
attached resource when you send, with a note when it reads as an email and who
it is from. The agent works from the words, not the pixels, and your brain's own
rules decide what to do with them (log the correspondence, answer the question,
file the receipt). Nothing to click.

**Status decay.** Every folder under the match and vocabulary folders whose
`CLAUDE.md` opens with a dated callout (`> **Status 2026-09-22:**`, or the
older `> ## … 9/22` heading style) is compared with its newest sibling file or
dated subfolder. When a sibling is newer, the folder name turns amber in the
tree with a "Status 9/22, newest note 9/30." tooltip. Re-checked every five
minutes and shortly after any file change; fixing the callout date clears it.

**Shared Claude memory.** On the first launch for a brain, the memory folders
Claude Code keeps per project under `~/.claude` and under Brainz's own
`~/.config/brainz/claude` are merged into the brain at `.claude/memory/`
(newest file wins on a name collision, `MEMORY.md` entries unioned, the
originals kept beside them as `memory.pre-share-<stamp>`), and both locations
become symlinks to it, so a memory written in Brainz is there in a terminal
`claude` session and vice versa. Credentials never move. If the layout is not
one of the expected shapes, nothing changes and a notification says why.

The MCP button next to it opens an **MCP Connectors** tab listing the servers
Brainz's Claude and Codex know about, with logos for the ones you use most.
The first time Claude runs in Brainz, your terminal Claude's MCP servers are
copied into Brainz's own config so both have the same connectors. Every minute
Brainz probes each connector (HTTP servers must answer, even with 401; stdio
commands must exist). Any that are down turn the MCP button red, and their row
in the tab gets a red dot with the reason. An amber dot means Claude's sign-in to
that connector has lapsed (read from Claude's own auth cache); **Reconnect** on the
row, or "Reconnect in Claude / Codex" under the row's menu, runs the CLI's `mcp login`
in the terminal panel and brings your panel back when it finishes. When an agent
reply or a failed connector tool call says a connector needs reconnecting, the same
card appears in the conversation with a one-click Reconnect. Hosted connectors
Brainz knows how to set up (Notion, for now) appear under **Available** in the tab,
and when a conversation tries to connect one that isn't configured yet, a card
offers to add it and sign in, since the agent cannot do that itself. The tab bar's
split button is a plain toggle: on splits right, off joins everything back.

**Brainz → Install CLI** links `/usr/local/bin/brainz` to the bundled `cli` helper
(it asks for an administrator password once), so `brainz some/folder` or
`brainz note.md` from a terminal opens in the running Brainz. The helper reaches
the app through the `zed-cli://` URL scheme the bundle registers.

`script/brainz-local` signs the app with your Apple Development identity so
macOS remembers permission grants across rebuilds. Override it with
`BRAINZ_SIGNING_IDENTITY`. An ad-hoc signature would prompt every build.

**Cutting a release.** `script/brainz-release 0.2.0` makes a release build
with that version baked in, bundles and signs it with the Developer ID
certificate in the keychain, notarizes and staples the app and the DMG,
uploads `brainz-0.2.0.dmg`, the `brainz.dmg` alias, and `brainz-latest.json`
to the download host, and creates a GitHub release with the DMG attached.
It reads `BLOB_READ_WRITE_TOKEN` and `BRAINZ_NOTARY_PROFILE` from the
untracked `script/brainz-local.env`. Flags: `--skip-notarize`, `--no-upload`,
`--no-github`.

**Another machine or another brain.** Nothing about one machine is baked
into the code. The build script takes `BRAINZ_SIGNING_IDENTITY` (falls back
to ad-hoc signing with a warning) and `BRAINZ_WORKSPACE` (the brain to open;
default `~/brain`), both of which can live in an untracked
`script/brainz-local.env`. Gellix font files are not in Git, so copy
`assets/fonts/gellix/` over. Sign in to Claude and Codex once there; their
config lives under `~/.config/brainz/`. The brain's layout comes from an
optional `brainz.toml` at the brain's root, every key optional: `todo` (the
To-Do file), `themes_dir`, `people_dir`, `places_dir`, `vocabulary_folders`,
`exclude_prefixes`, `dated_exclude_dirs`, `stop_words`, `sync`, a `[calendar]`
table (`prep_lead_minutes`, `match_dirs`), and a
`[themes]` table (`noise_dirs`, `window_weeks`). The defaults and what each
key changes are listed at the top of `crates/brainz_calendar/src/brain_config.rs`;
a brain can keep its own copy of that reference next to its `brainz.toml`. A brain with no
`origin` remote, or with `sync = false`, never shows the Sync banner. The
Themes narrative is written by whatever bot you point at the prompt file in
`themes_dir`; Brainz only renders the block. MCP connectors come from that
machine's own Claude and Codex configs, with logos for the ones Brainz knows
and a generic icon for the rest.

An amber banner above the file tree appears when the brain and GitHub differ.
With local edits or commits it says "N changes not on GitHub" and **Sync to
GitHub** reviews them (no secrets, no huge files), commits, pushes a branch,
opens a pull request, waits for it to be mergeable, merges it into main, and
brings main back down. When GitHub has commits you don't, it says "N new on
GitHub" and the button becomes **Pull from GitHub**, a rebase with autostash so
local edits survive. With work on both sides, Sync pulls first. Errors open a
popup; success shows a toast that fades on its own.

Claude and Codex connect through the existing ACP integration, with their own
authentication and permissions. Their adapters install from the ACP registry on
first use. Brainz gives each one its own config directory
(`~/.config/brainz/claude` and `~/.config/brainz/codex`), so signing in inside
Brainz never signs out a `claude` or `codex` session running in another terminal
app, and vice versa. Expect one extra sign-in per machine. `settings.json`,
`CLAUDE.md`, and `config.toml` are copied over from the CLI directories the first
time; credentials never are. You can still run either CLI directly in a shell.

On macOS, the bundled zsh profile keeps your normal startup files, aliases, and
history, with a short amber prompt and no system login banner. Explicit custom
shell settings take precedence. The profile lives in `assets/brainz/shell/`.

Custom icons live in `assets/icons/brainz/`; their central mapping is
`IconName::path` in `crates/icons/src/icons.rs`. `IconName::BrainzTheme` (a
rising trend line) is reserved for a future Themes view. Search shows the query and match
navigation. Advanced controls appear only when their mode is already active,
so keyboard shortcuts cannot leave an invisible filter enabled.

## Upstream Zed

[![Zed](https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/zed-industries/zed/main/assets/badge/v0.json)](https://zed.dev)
[![CI](https://github.com/zed-industries/zed/actions/workflows/run_tests.yml/badge.svg)](https://github.com/zed-industries/zed/actions/workflows/run_tests.yml)

Welcome to Zed, a high-performance, multiplayer code editor from the creators of [Atom](https://github.com/atom/atom) and [Tree-sitter](https://github.com/tree-sitter/tree-sitter).

---

### Installation

On macOS, Linux, and Windows you can [download Zed directly](https://zed.dev/download) or install Zed via your local package manager ([macOS](https://zed.dev/docs/installation#macos)/[Linux](https://zed.dev/docs/linux#installing-via-a-package-manager)/[Windows](https://zed.dev/docs/windows#package-managers)).

Other platforms are not yet available:

- Web ([tracking discussion](https://github.com/zed-industries/zed/discussions/26195))

### Developing Zed

- [Building Zed for macOS](./docs/src/development/macos.md)
- [Building Zed for Linux](./docs/src/development/linux.md)
- [Building Zed for Windows](./docs/src/development/windows.md)

### Contributing

See [CONTRIBUTING.md](./CONTRIBUTING.md) for ways you can contribute to Zed.

Also... we're hiring! Check out our [jobs](https://zed.dev/jobs) page for open roles.

### Licensing

Zed source code is licensed primarily under GPL-3.0-or-later, with Apache-2.0 components where marked.

License information for third party dependencies must be correctly provided for CI to pass.

We use [`cargo-about`](https://github.com/EmbarkStudios/cargo-about) to automatically comply with open source licenses. If CI is failing, check the following:

- Is it showing a `no license specified` error for a crate you've created? If so, add `publish = false` under `[package]` in your crate's Cargo.toml.
- Is the error `failed to satisfy license requirements` for a dependency? If so, first determine what license the project has and whether this system is sufficient to comply with this license's requirements. If you're unsure, ask a lawyer. Once you've verified that this system is acceptable add the license's SPDX identifier to the `accepted` array in `script/licenses/zed-licenses.toml`.
- Is `cargo-about` unable to find the license for a dependency? If so, add a clarification field at the end of `script/licenses/zed-licenses.toml`, as specified in the [cargo-about book](https://embarkstudios.github.io/cargo-about/cli/generate/config.html#crate-configuration).

## Sponsorship

Zed is developed by **Zed Industries, Inc.**, a for-profit company.

If you’d like to financially support the project, you can do so via GitHub Sponsors.
Sponsorships go directly to Zed Industries and are used as general company revenue.
There are no perks or entitlements associated with sponsorship.
