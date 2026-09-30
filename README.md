> [!IMPORTANT]
> Remove this line to confirm you've reviewed this PR before submitting.

# Brainz

A personal desktop workspace for `tommy-brain`, built from Zed. Files on the left,
Markdown reading in the center, and a terminal below, with Gellix typography and
a charcoal-and-amber theme.

Run `./script/brainz-local` on macOS to build and open `~/Applications/Brainz.app`.
The first build requires Rust 1.98.1, Xcode, and CMake. Local Gellix font files live
in `assets/fonts/gellix/` and are excluded from Git.

Brainz uses `~/.config/brainz` and `~/Library/Application Support/Brainz`. Edit
`~/.config/brainz/themes/brainz.json` to adjust colors live, or
`~/.config/brainz/settings.json` to change the layout and terminal preferences.

The bottom panel's **Launch** button (rocket) opens a Shell or a native Claude or
Codex conversation in the current project. Everything you open becomes a tab in the
strip under the toolbar, shells and conversations side by side. Click a tab to
switch, use its close button to drop it, and right-click it to give it a color
(a filled rounded square replaces the tab icon).
Tabs and colors are remembered between launches. Conversations and the message
box use Gellix with natural spacing. Shell content uses Lilex for a fixed
character grid. The message box stays one line tall until you click into it.
Paste or attach a screenshot and it shows as a thumbnail above the text before
you send.

The calendar button in the status bar (bottom left) opens a **Calendar** tab
with the next seven days, read from every account macOS Calendar knows about.
It uses a small bundled helper built from `script/brainz-calendar.swift`, the
same EventKit approach as My Man. macOS asks for Calendar access the first
time; the tab refreshes every five minutes and on demand. Sent messages sit on
the right in the tab's colour; replies sit on the left. Hovering a reply or a
quoted draft shows a copy button that rides along the top of the visible part
of the text while you scroll, so a long email never needs scrolling back up.
Hovering one of your own messages shows a send-again button that posts the
same text as a new message. The options drawer behind the sliders button has
a reset button that starts a fresh conversation with the same agent in the
same tab (position and colour kept) without reconnecting; the old
conversation stays in the sidebar. The message box says
"Type message…", grows as you type, and keeps model and mode options behind the
gear button.

The checkbox button next to the calendar opens a **To-Do** tab over
`ops/desk/TODO.md`, the check-off board TodoBot maintains in the brain. It shows
Today, Urgent, and Delayed sections with Done folded away, and refreshes every
30 seconds. Ticking an item checks it in place with today's date and shows it
struck through in amber, so nothing vanishes; un-ticking clears the box.
TodoBot tidies checked items into Done on its next pass.

The MCP button next to it opens an **MCP Connectors** tab listing the servers
Brainz's Claude and Codex know about, with logos for the ones you use most.
The first time Claude runs in Brainz, your terminal Claude's MCP servers are
copied into Brainz's own config so both have the same connectors. Every minute
Brainz probes each connector (HTTP servers must answer, even with 401; stdio
commands must exist). Any that are down turn the MCP button red, and their row
in the tab gets a red dot with the reason. The tab bar's
split button is a plain toggle: on splits right, off joins everything back.

`script/brainz-local` signs the app with your Apple Development identity so
macOS remembers permission grants across rebuilds. Override it with
`BRAINZ_SIGNING_IDENTITY`. An ad-hoc signature would prompt every build.

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
`IconName::path` in `crates/icons/src/icons.rs`. Search shows the query and match
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
