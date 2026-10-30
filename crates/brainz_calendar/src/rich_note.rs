//! Brainz: Markdown notes open in a rich text editor, BlockSuite's doc mode
//! running in a web view. The page receives the note as top-level segments
//! and sends back the whole file, keeping the bytes of every block the person
//! did not touch. Saves go through the project buffer, so undo, the file
//! watcher and agents editing the same note all see one copy of the text.

use gpui::{App, actions};

actions!(
    brainz_note,
    [
        /// Open the active Markdown file in the rich text editor.
        OpenRichNote,
        /// Show the Markdown source of the open note.
        ShowMarkdownSource,
        /// Switch the active note between rich text and Markdown; anywhere
        /// else, show the project's diagnostics as before.
        ToggleNoteView,
    ]
);

pub fn init(cx: &mut App) {
    #[cfg(target_os = "macos")]
    native::init(cx);
    #[cfg(not(target_os = "macos"))]
    let _ = cx;
}

/// Whether `href` leaves Brainz: web pages and mail open in the browser.
fn is_external_link(href: &str) -> bool {
    let lower = href.trim().to_ascii_lowercase();
    ["http://", "https://", "mailto:", "tel:"]
        .iter()
        .any(|scheme| lower.starts_with(scheme))
}

/// A relative link such as `../people/sam.md#notes` resolved against the
/// note's folder, without its fragment or query. None for anchors alone.
fn resolve_note_link(note: &std::path::Path, href: &str) -> Option<std::path::PathBuf> {
    let path = href.split(['#', '?']).next()?.trim();
    if path.is_empty() {
        return None;
    }
    let path = percent_decode(path);
    let candidate = std::path::Path::new(&path);
    Some(if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        note.parent()?.join(candidate)
    })
}

const MAX_NOTE_IMAGE_BYTES: u64 = 25 * 1024 * 1024;

fn image_mime(path: &std::path::Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "heic" => "image/heic",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        _ => return None,
    })
}

/// An image the note refers to, read only when it is an image file inside
/// the brain: the page may ask for any path, so nothing else is served.
pub(crate) fn read_brain_image(
    note: &std::path::Path,
    brain: &std::path::Path,
    src: &str,
) -> anyhow::Result<(Vec<u8>, &'static str)> {
    use anyhow::Context as _;
    let path = resolve_note_link(note, src).context("no image path")?;
    let path = path.canonicalize().context("image not found")?;
    anyhow::ensure!(
        path.starts_with(brain.canonicalize()?),
        "image is outside the brain"
    );
    let mime = image_mime(&path).context("not an image")?;
    anyhow::ensure!(
        std::fs::metadata(&path)?.len() <= MAX_NOTE_IMAGE_BYTES,
        "image is too large"
    );
    Ok((std::fs::read(&path)?, mime))
}

/// Saves a pasted or dropped image in an `attachments` folder beside the
/// note and returns the Markdown path to it, relative to the note.
fn save_note_image(
    note: &std::path::Path,
    mime: &str,
    bytes: &[u8],
    now: chrono::DateTime<chrono::Local>,
) -> anyhow::Result<String> {
    use anyhow::Context as _;
    use std::io::Write as _;
    anyhow::ensure!(
        (bytes.len() as u64) <= MAX_NOTE_IMAGE_BYTES,
        "image is too large"
    );
    let extension = match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/avif" => "avif",
        "image/heic" => "heic",
        "image/bmp" => "bmp",
        other => anyhow::bail!("{other} is not an image Brainz saves"),
    };
    let folder = note
        .parent()
        .context("note has no folder")?
        .join("attachments");
    std::fs::create_dir_all(&folder)?;
    let stem: String = note
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_lowercase())
        .unwrap_or_default()
        .chars()
        .map(|character| {
            if character.is_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect();
    let stem = stem.trim_matches('-');
    let base = format!(
        "{}{}",
        if stem.is_empty() {
            "image".to_owned()
        } else {
            format!("{stem}-")
        },
        now.format("%Y%m%d-%H%M%S")
    );
    for number in 1..1000 {
        let name = if number == 1 {
            format!("{base}.{extension}")
        } else {
            format!("{base}-{number}.{extension}")
        };
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(folder.join(&name))
        {
            Ok(mut file) => {
                file.write_all(bytes)?;
                return Ok(format!("attachments/{name}"));
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error).context("saving the image"),
        }
    }
    anyhow::bail!("too many images saved this second")
}

/// A note the `[[` menu offers, by its path from the brain's folder.
#[derive(serde::Serialize, Debug, PartialEq)]
struct NoteEntry {
    path: String,
    title: String,
}

/// A note that links to the open one, with the line that holds the link.
#[derive(serde::Serialize, Debug, PartialEq)]
struct Backlink {
    path: String,
    title: String,
    snippet: String,
}

/// The note's first `# ` heading, else its file name.
fn note_title(path: &str, text: &str) -> String {
    text.lines()
        .take(40)
        .find_map(|line| line.strip_prefix("# "))
        .map(|heading| heading.trim().to_owned())
        .filter(|heading| !heading.is_empty())
        .unwrap_or_else(|| {
            std::path::Path::new(path)
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.to_owned())
        })
}

fn brain_notes(brain: &std::path::Path) -> anyhow::Result<Vec<NoteEntry>> {
    use std::io::Read as _;
    crate::backup::list_notes(brain).map(|paths| {
        paths
            .into_iter()
            .map(|path| {
                // The heading is near the top; a large note is not read whole.
                let mut head = Vec::new();
                if let Ok(file) = std::fs::File::open(brain.join(&path)) {
                    file.take(8 * 1024).read_to_end(&mut head).ok();
                }
                let title = note_title(&path, &String::from_utf8_lossy(&head));
                NoteEntry { path, title }
            })
            .collect()
    })
}

/// Markdown link targets in a line: the part inside `](...)`.
fn link_targets(line: &str) -> impl Iterator<Item = &str> {
    line.match_indices("](").filter_map(move |(start, _)| {
        let rest = &line[start + 2..];
        match rest.strip_prefix('<') {
            Some(inner) => inner.find('>').map(|end| &inner[..end]),
            None => rest.find([')', ' ']).map(|end| &rest[..end]),
        }
    })
}

/// A line with Markdown marks removed, for showing as a snippet.
fn snippet(line: &str) -> String {
    let mut text = String::new();
    let mut rest = line.trim();
    while let Some(start) = rest.find('[') {
        text.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match (after.find("]("), after.find(')')) {
            (Some(close), Some(end)) if close < end => {
                text.push_str(&after[..close]);
                rest = &after[end + 1..];
            }
            _ => {
                text.push('[');
                rest = after;
            }
        }
    }
    text.push_str(rest);
    let text: String = text
        .trim_start_matches(['#', '>', '-', '*', ' '])
        .replace(['*', '`'], "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if text.chars().count() > 160 {
        format!("{}…", text.chars().take(159).collect::<String>())
    } else {
        text
    }
}

/// Notes in the brain with a Markdown link that resolves to `note`.
fn backlinks(brain: &std::path::Path, note: &std::path::Path) -> anyhow::Result<Vec<Backlink>> {
    backlinks_to(brain, note, None)
}

/// Backlinks to `note`, which may already have moved to `renamed`: links are
/// then compared by path rather than by the file they reach.
#[allow(clippy::disallowed_methods, reason = "runs on the background executor")]
fn backlinks_to(
    brain: &std::path::Path,
    note: &std::path::Path,
    renamed: Option<&std::path::Path>,
) -> anyhow::Result<Vec<Backlink>> {
    let brain_canonical = brain.canonicalize()?;
    let note = match renamed {
        Some(_) => normalize(&brain_canonical.join(note.strip_prefix(brain).unwrap_or(note))),
        None => note.canonicalize()?,
    };
    let Some(name) = note
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
    else {
        return Ok(Vec::new());
    };
    let encoded = name.replace(' ', "%20");
    // git grep narrows the brain to files naming this note; each match is then
    // resolved, so "plan.md" in another folder does not count.
    let output = crate::github_sync::command(brain, "git")
        .args(["grep", "-l", "-I", "--untracked", "-F", "-e"])
        .arg(&name)
        .args(["-e", &encoded, "--", "*.md", "*.markdown"])
        .output()?;
    let mut found = Vec::new();
    for path in String::from_utf8_lossy(&output.stdout).lines() {
        let file = brain.join(path);
        if file.canonicalize().ok().as_ref() == Some(&note)
            || renamed.is_some_and(|renamed| file == renamed)
        {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let line = text.lines().find(|line| {
            link_targets(line).any(|target| {
                !is_external_link(target)
                    && resolve_note_link(&brain_canonical.join(path), target)
                        .map(|target| match renamed {
                            Some(_) => normalize(&target),
                            None => target.canonicalize().unwrap_or(target),
                        })
                        .as_ref()
                        == Some(&note)
            })
        });
        if let Some(line) = line {
            found.push(Backlink {
                path: path.to_owned(),
                title: note_title(path, &text),
                snippet: snippet(line),
            });
        }
    }
    found.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(found)
}

/// What a note's Claude menu asks for, as the message Claude receives, and
/// whether it is sent straight away (Ask leaves it for the user to finish).
fn claude_request(
    action: &str,
    note: &str,
    text: &str,
    todo_board: &str,
) -> Option<(String, bool)> {
    // A fence longer than any backtick run in the passage keeps it whole.
    let longest = text
        .split(|character| character != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    let passage = format!("{fence}markdown\n{}\n{fence}", text.trim());
    let request = match action {
        "rewrite" => (
            format!(
                "In the note `{note}`, rewrite this passage so it reads more clearly. Keep its meaning, facts, names and Markdown formatting. Replace exactly that passage in the file and change nothing else.\n\n{passage}"
            ),
            true,
        ),
        "summarize" => (
            format!(
                "In the note `{note}`, add a short summary (two to four sentences) of this passage directly below it, as a quote block that starts with **Summary:**. Change nothing else in the note.\n\n{passage}"
            ),
            true,
        ),
        "todos" => (
            format!(
                "Turn the action items in this passage from `{note}` into to-dos on my To-Do board (`{todo_board}`), under Today, one line per action, each ending with a link back to the note. Don't edit the note itself.\n\n{passage}"
            ),
            true,
        ),
        "ask" => (
            format!("About this passage in `{note}`:\n\n{passage}\n\n"),
            false,
        ),
        _ => return None,
    };
    Some(request)
}

/// The file name a note's heading suggests: characters a file name cannot
/// hold become dashes, runs of spaces collapse, and `.md` is added.
fn file_name_for_heading(heading: &str) -> Option<String> {
    let cleaned: String = heading
        .chars()
        .map(|character| match character {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            character if character.is_control() => ' ',
            character => character,
        })
        .collect();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let cleaned: String = cleaned.trim_matches(['.', ' ']).chars().take(100).collect();
    let cleaned = cleaned.trim_end();
    (!cleaned.is_empty()).then(|| format!("{cleaned}.md"))
}

/// The Markdown link from `from` (a note) to `to`, both absolute, with
/// spaces encoded the way Brainz writes links.
fn relative_link(from: &std::path::Path, to: &std::path::Path) -> Option<String> {
    let from_folder = from.parent()?;
    let mut common = 0;
    let from_parts: Vec<_> = from_folder.components().collect();
    let to_parts: Vec<_> = to.components().collect();
    while common < from_parts.len()
        && common + 1 < to_parts.len()
        && from_parts[common] == to_parts[common]
    {
        common += 1;
    }
    let mut parts: Vec<String> = from_parts[common..]
        .iter()
        .map(|_| "..".to_owned())
        .collect();
    parts.extend(
        to_parts[common..]
            .iter()
            .map(|part| part.as_os_str().to_string_lossy().replace(' ', "%20")),
    );
    Some(parts.join("/"))
}

/// Points every Markdown link to `old` in the brain at `new` instead (both
/// relative to the brain), so a renamed note keeps its backlinks. Returns how
/// many notes changed.
fn rewrite_links(
    brain: &std::path::Path,
    old: &std::path::Path,
    new: &std::path::Path,
) -> anyhow::Result<usize> {
    let brain = &brain.canonicalize()?;
    let old = &normalize(&brain.join(old));
    let new = &brain.join(new);
    let mut changed = 0;
    for link in backlinks_to(brain, old, Some(new))? {
        let file = brain.join(&link.path);
        let text = std::fs::read_to_string(&file)?;
        let Some(replacement) = relative_link(&file, new) else {
            continue;
        };
        let mut rewritten = String::with_capacity(text.len());
        let mut rest = text.as_str();
        while let Some(start) = rest.find("](") {
            let (before, after) = rest.split_at(start + 2);
            rewritten.push_str(before);
            let (target, tail) = match after.strip_prefix('<') {
                Some(inner) => match inner.find('>') {
                    Some(end) => (&inner[..end], &inner[end + 1..]),
                    None => ("", after),
                },
                None => match after.find([')', ' ']) {
                    Some(end) => (&after[..end], &after[end..]),
                    None => ("", after),
                },
            };
            let fragment = target.find('#').map(|index| &target[index..]).unwrap_or("");
            let points_here = !target.is_empty()
                && !is_external_link(target)
                && resolve_note_link(&file, target)
                    .map(|path| normalize(&path))
                    .as_deref()
                    == Some(old);
            if points_here {
                rewritten.push_str(&replacement);
                rewritten.push_str(fragment);
                rest = tail;
            } else {
                rest = after;
            }
        }
        rewritten.push_str(rest);
        if rewritten != text {
            std::fs::write(&file, rewritten)?;
            changed += 1;
        }
    }
    Ok(changed)
}

/// `a/./b/../c` as `a/c`, without touching the disk (the old path is gone).
fn normalize(path: &std::path::Path) -> std::path::PathBuf {
    let mut normal = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normal.pop();
            }
            component => normal.push(component),
        }
    }
    normal
}

fn percent_decode(text: &str) -> String {
    url::form_urlencoded::parse(format!("p={}", text.replace('+', "%2B")).as_bytes())
        .next()
        .map(|(_, value)| value.into_owned())
        .unwrap_or_else(|| text.to_owned())
}

#[cfg(target_os = "macos")]
mod native {
    use std::{borrow::Cow, cell::Cell, path::PathBuf, rc::Rc};

    use anyhow::{Context as _, Result};
    use base64::Engine as _;
    use editor::Editor;
    use gpui::{
        App, Bounds, Context, Entity, EventEmitter, FocusHandle, Focusable, Pixels, Subscription,
        Task, WeakEntity, Window,
    };
    use language::{Buffer, BufferEvent};
    use markdown_preview::markdown_preview_view::{MarkdownPreviewView, RichNoteOpener};
    use project::Project;
    use serde::Deserialize;
    use serde_json::{Value, json};
    use ui::prelude::*;
    use util::ResultExt as _;
    use workspace::{
        Item, ItemHandle as _, OpenOptions, Pane, Workspace,
        item::{ItemBufferKind, ItemEvent},
    };

    use super::{OpenRichNote, ShowMarkdownSource, ToggleNoteView};

    pub(super) fn init(cx: &mut App) {
        cx.set_global(RichNoteOpener(Rc::new(
            |workspace, editor, _pane, focus, window, cx| {
                let view = RichNoteView::for_editor(workspace, &editor, focus, window, cx)?;
                Some(Box::new(view))
            },
        )));
        cx.observe_new(|workspace: &mut Workspace, _, _| {
            workspace.register_action(|workspace, _: &OpenRichNote, window, cx| {
                if let Some(editor) = workspace
                    .active_item(cx)
                    .and_then(|item| item.downcast::<Editor>())
                {
                    open_rich_note(workspace, &editor, window, cx);
                }
            });
            // Cmd-Shift-M from the sidebar or a panel still means the open
            // note; it only falls back to diagnostics when no note is active.
            workspace.register_action(|workspace, _: &ToggleNoteView, window, cx| {
                let active = workspace.active_item(cx);
                if let Some(note) = active
                    .as_ref()
                    .and_then(|item| item.downcast::<RichNoteView>())
                {
                    note.update(cx, |note, cx| note.show_source(window, cx));
                    return;
                }
                if let Some(editor) = active
                    .and_then(|item| item.downcast::<Editor>())
                    .filter(|editor| is_markdown(editor, cx))
                {
                    open_rich_note(workspace, &editor, window, cx);
                    return;
                }
                if let Ok(deploy) = cx.build_action("diagnostics::Deploy", None) {
                    window.dispatch_action(deploy, cx);
                }
            });
        })
        .detach();
    }

    fn is_markdown(editor: &Entity<Editor>, cx: &App) -> bool {
        editor
            .read(cx)
            .buffer()
            .read(cx)
            .as_singleton()
            .and_then(|buffer| buffer.read(cx).file().map(|file| file.path().clone()))
            .and_then(|path| {
                path.extension()
                    .map(|extension| extension.to_ascii_lowercase())
            })
            .is_some_and(|extension| matches!(extension.as_str(), "md" | "markdown"))
    }

    fn open_rich_note(
        workspace: &mut Workspace,
        editor: &Entity<Editor>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        let Some(pane) = workspace.pane_for(editor) else {
            return;
        };
        let Some(view) = RichNoteView::for_editor(workspace, editor, true, window, cx) else {
            return;
        };
        // Both items show one file, and a pane only keeps one item per
        // file, so the editor leaves before the note is added.
        pane.update(cx, |pane, cx| {
            let index = pane.index_for_item(editor);
            pane.remove_item(editor.item_id(), false, false, window, cx);
            pane.add_item(Box::new(view), true, true, index, window, cx);
        });
    }

    #[derive(Deserialize)]
    #[serde(tag = "type", rename_all = "camelCase")]
    enum Message {
        Ready,
        Save {
            revision: u64,
            markdown: String,
        },
        Open {
            href: String,
        },
        Error {
            message: String,
        },
        Source,
        Image {
            request: u64,
            mime: String,
            data: String,
        },
        CopyImage {
            data: String,
        },
        Claude {
            action: String,
            text: String,
        },
        Rename {
            heading: String,
        },
        History,
        CopyPath,
    }

    pub struct RichNoteView {
        workspace: WeakEntity<Workspace>,
        project: Entity<Project>,
        buffer: Entity<Buffer>,
        path: PathBuf,
        focus_handle: FocusHandle,
        webview: Option<Rc<wry::WebView>>,
        bounds: Rc<Cell<Bounds<Pixels>>>,
        ready: bool,
        /// The text the page currently holds, so buffer edits that only echo
        /// the page's own save do not reload it.
        page_text: Option<String>,
        revision: u64,
        error: Option<SharedString>,
        save: Option<Task<()>>,
        messages: Option<Task<()>>,
        _subscriptions: Vec<Subscription>,
    }

    impl RichNoteView {
        fn for_editor(
            workspace: &mut Workspace,
            editor: &Entity<Editor>,
            focus: bool,
            window: &mut Window,
            cx: &mut Context<Workspace>,
        ) -> Option<Entity<Self>> {
            let project = workspace.project().clone();
            if !project.read(cx).is_local() {
                return None;
            }
            let buffer = editor.read(cx).buffer().read(cx).as_singleton()?;
            let path = buffer
                .read(cx)
                .file()
                .and_then(|file| file.as_local())
                .map(|file| file.abs_path(cx))?;
            let workspace_handle = cx.entity().downgrade();
            let view = cx.new(|cx| Self::new(workspace_handle, project, buffer, path, window, cx));
            if focus {
                let focus_handle = view.read(cx).focus_handle.clone();
                focus_handle.focus(window, cx);
            }
            Some(view)
        }

        fn new(
            workspace: WeakEntity<Workspace>,
            project: Entity<Project>,
            buffer: Entity<Buffer>,
            path: PathBuf,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) -> Self {
            let focus_handle = cx.focus_handle();
            let subscriptions = vec![
                cx.subscribe(&buffer, |this, _, event: &BufferEvent, cx| match event {
                    BufferEvent::Edited { .. } | BufferEvent::Reloaded => this.load_from_buffer(cx),
                    BufferEvent::FileHandleChanged => {
                        // The file moved (a rename here or elsewhere); follow it.
                        if let Some(path) = this
                            .buffer
                            .read(cx)
                            .file()
                            .and_then(|file| file.as_local())
                            .map(|file| file.abs_path(cx))
                        {
                            this.path = path;
                        }
                        cx.emit(ItemEvent::UpdateTab)
                    }
                    _ => {}
                }),
                cx.on_focus(&focus_handle, window, |this, _, _| {
                    if let Some(webview) = &this.webview {
                        webview.focus().log_err();
                    }
                    this.send(json!({"type": "focus"}));
                }),
            ];
            Self {
                workspace,
                project,
                buffer,
                path,
                focus_handle,
                webview: None,
                bounds: Rc::default(),
                ready: false,
                page_text: None,
                revision: 0,
                error: None,
                save: None,
                messages: None,
                _subscriptions: subscriptions,
            }
        }

        fn send(&self, message: Value) {
            if let Some(webview) = &self.webview {
                webview
                    .evaluate_script(&format!(
                        "window.brainzNote && window.brainzNote.receive({message});"
                    ))
                    .log_err();
            }
        }

        fn hide(&self) {
            if let Some(webview) = &self.webview {
                // Hiding WKWebView does not hand AppKit's first responder back
                // to GPUI; without this keys keep going to the hidden page.
                webview.focus_parent().log_err();
                webview.set_visible(false).log_err();
            }
        }

        fn load_from_buffer(&mut self, cx: &mut Context<Self>) {
            if !self.ready {
                return;
            }
            let text = self.buffer.read(cx).text();
            if self.page_text.as_deref() == Some(text.as_str()) {
                return;
            }
            let segments = brainz_editor::BlockMap::parse(text.as_str())
                .note_segments()
                .into_iter()
                .enumerate()
                .map(|(id, segment)| json!({"id": id, "kind": segment.kind, "text": segment.text}))
                .collect::<Vec<_>>();
            let read_only = self.buffer.read(cx).read_only();
            self.send(json!({
                "type": "load",
                "path": self.path,
                "readOnly": read_only,
                "segments": segments,
            }));
            self.page_text = Some(text);
        }

        fn apply_page_text(&mut self, revision: u64, markdown: String, cx: &mut Context<Self>) {
            if revision <= self.revision {
                return;
            }
            self.revision = revision;
            self.page_text = Some(markdown.clone());
            if self.buffer.read(cx).text() == markdown {
                self.send(json!({"type": "saved", "revision": revision}));
                return;
            }
            let diff = self.buffer.read(cx).diff(markdown, cx);
            let buffer = self.buffer.clone();
            let project = self.project.clone();
            self.save = Some(cx.spawn(async move |this, cx| {
                let diff = diff.await;
                buffer.update(cx, |buffer, cx| {
                    buffer.finalize_last_transaction();
                    buffer.apply_diff(diff, cx);
                    buffer.finalize_last_transaction();
                });
                let result = project
                    .update(cx, |project, cx| project.save_buffer(buffer, cx))
                    .await;
                this.update(cx, |this, cx| match result {
                    Ok(()) => this.send(json!({"type": "saved", "revision": revision})),
                    Err(error) => {
                        this.error = Some(format!("Could not save this note: {error}").into());
                        cx.notify();
                    }
                })
                .log_err();
            }));
        }

        fn open_link(&mut self, href: String, window: &mut Window, cx: &mut Context<Self>) {
            if super::is_external_link(&href) {
                cx.open_url(href.trim());
                return;
            }
            let Some(path) = super::resolve_note_link(&self.path, &href) else {
                return;
            };
            let Some(workspace) = self.workspace.upgrade() else {
                return;
            };
            workspace.update(cx, |workspace, cx| {
                workspace
                    .open_abs_path(path, OpenOptions::default(), window, cx)
                    .detach_and_log_err(cx);
            });
        }

        /// Swaps this tab for a plain editor on the same buffer, so the raw
        /// Markdown is one keystroke away and its undo history is shared.
        fn show_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
            let Some(workspace) = self.workspace.upgrade() else {
                return;
            };
            let this = cx.entity();
            let buffer = self.buffer.clone();
            let project = self.project.clone();
            self.hide();
            window.defer(cx, move |window, cx| {
                workspace.update(cx, |workspace, cx| {
                    let Some(pane) = workspace.pane_for(&this) else {
                        return;
                    };
                    let editor = cx.new(|cx| Editor::for_buffer(buffer, Some(project), window, cx));
                    MarkdownPreviewView::suppress_auto_preview_once(&editor, cx);
                    pane.update(cx, |pane: &mut Pane, cx| {
                        let index = pane.index_for_item(&this);
                        pane.remove_item(this.item_id(), false, false, window, cx);
                        pane.add_item(Box::new(editor), true, true, index, window, cx);
                    });
                });
            });
        }

        fn create_webview(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Result<()> {
            let root = crate::whiteboard::native::asset_root()?;
            anyhow::ensure!(
                root.join("note.html").is_file(),
                "The note editor is missing from this build. Rebuild it with script/brainz-whiteboard-build."
            );
            let note = self.path.clone();
            let brain = self
                .project
                .read(cx)
                .find_worktree(&self.path, cx)
                .map(|(worktree, _)| worktree.read(cx).abs_path().to_path_buf())
                .context("The note is not in an open brain")?;
            let (sender, receiver) = smol::channel::unbounded::<Message>();
            let webview = wry::WebViewBuilder::new()
                .with_background_color((20, 20, 19, 255))
                .with_visible(false)
                .with_back_forward_navigation_gestures(false)
                .with_custom_protocol("brainz-note".into(), move |_, request| {
                    asset_response(&root, &note, &brain, request)
                })
                .with_navigation_handler(|url| url == "brainz-note://app/note.html")
                .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
                .with_ipc_handler(move |request| {
                    match serde_json::from_str::<Message>(request.body()) {
                        Ok(message) => {
                            if let Err(error) = sender.try_send(message) {
                                log::debug!("Note tab closed: {error}");
                            }
                        }
                        Err(error) => log::error!("Invalid note editor message: {error}"),
                    }
                })
                .with_url("brainz-note://app/note.html")
                .build_as_child(&*window)?;
            self.webview = Some(Rc::new(webview));
            self.messages = Some(cx.spawn_in(window, async move |this, cx| {
                while let Ok(message) = receiver.recv().await {
                    if this
                        .update_in(cx, |this, window, cx| this.handle(message, window, cx))
                        .is_err()
                    {
                        break;
                    }
                }
            }));
            Ok(())
        }

        /// The notes `[[` offers and the ones linking here, found off the main thread.
        fn send_links(&mut self, cx: &mut Context<Self>) {
            let note = self.path.clone();
            let Some(brain) = self
                .project
                .read(cx)
                .find_worktree(&self.path, cx)
                .map(|(worktree, _)| worktree.read(cx).abs_path().to_path_buf())
            else {
                return;
            };
            cx.spawn(async move |this, cx| {
                let (notes, links) = cx
                    .background_spawn(async move {
                        let current = note
                            .strip_prefix(&brain)
                            .map(|path| path.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        let notes = super::brain_notes(&brain).log_err().unwrap_or_default();
                        let links = super::backlinks(&brain, &note)
                            .log_err()
                            .unwrap_or_default();
                        (
                            json!({"type": "notes", "current": current, "notes": notes}),
                            links,
                        )
                    })
                    .await;
                this.update(cx, |this, _| {
                    this.send(notes);
                    this.send(json!({"type": "backlinks", "items": links}));
                })
                .ok();
            })
            .detach();
        }

        /// Hands the selected passage to Claude in the chat, which edits the
        /// note file itself, so the change arrives like any agent edit.
        fn ask_claude(
            &mut self,
            action: &str,
            text: &str,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) {
            let Some(brain) = self
                .project
                .read(cx)
                .find_worktree(&self.path, cx)
                .map(|(worktree, _)| worktree.read(cx).abs_path().to_path_buf())
            else {
                return;
            };
            let note = self
                .path
                .strip_prefix(&brain)
                .unwrap_or(&self.path)
                .to_string_lossy()
                .into_owned();
            let board = crate::brain_config::BrainConfig::load(&brain).todo;
            let Some((message, send)) = super::claude_request(action, &note, text, &board) else {
                return;
            };
            window.defer(cx, move |window, cx| {
                if send {
                    window
                        .dispatch_action(Box::new(crate::palette::AskClaude { text: message }), cx);
                } else {
                    window.dispatch_action(
                        Box::new(crate::prep::OpenClaudePrefilled { text: message }),
                        cx,
                    );
                }
            });
        }

        /// Renames the note's file after its heading and points the brain's
        /// links to it at the new name.
        fn rename_to_heading(&mut self, heading: &str, cx: &mut Context<Self>) {
            let Some(file_name) = super::file_name_for_heading(heading) else {
                return;
            };
            let old = self.path.clone();
            let new = old.with_file_name(&file_name);
            if new == old {
                return;
            }
            let project = self.project.clone();
            let (project_path, entry_id, brain) = {
                let project = project.read(cx);
                let Some(project_path) = project.project_path_for_absolute_path(&old, cx) else {
                    return;
                };
                let Some(entry) = project.entry_for_path(&project_path, cx) else {
                    return;
                };
                let Some(brain) = project
                    .find_worktree(&old, cx)
                    .map(|(worktree, _)| worktree.read(cx).abs_path().to_path_buf())
                else {
                    return;
                };
                (project_path, entry.id, brain)
            };
            let workspace = self.workspace.clone();
            let notice = move |message: String, cx: &mut gpui::AsyncApp| {
                workspace
                    .update(cx, |workspace, cx| {
                        workspace.show_toast(
                            workspace::Toast::new(
                                workspace::notifications::NotificationId::unique::<RichNoteView>(),
                                message,
                            )
                            .autohide(),
                            cx,
                        )
                    })
                    .log_err();
            };
            if new.exists() {
                cx.spawn(async move |_, cx| {
                    notice(
                        format!("A note called {file_name} is already in this folder."),
                        cx,
                    )
                })
                .detach();
                return;
            }
            let Some(new_relative) = project_path.path.parent().and_then(|folder| {
                util::rel_path::RelPath::from_unix_str(&file_name)
                    .ok()
                    .map(|name| folder.join(name))
            }) else {
                return;
            };
            let rename = project.update(cx, |project, cx| {
                project.rename_entry(
                    entry_id,
                    project::ProjectPath {
                        worktree_id: project_path.worktree_id,
                        path: new_relative.as_rel_path().into_arc(),
                    },
                    cx,
                )
            });
            cx.spawn(async move |this, cx| {
                if let Err(error) = rename.await {
                    notice(format!("Couldn't rename the note: {error:#}"), cx);
                    return;
                }
                let old_relative = old.strip_prefix(&brain).unwrap_or(&old).to_path_buf();
                let new_relative = new.strip_prefix(&brain).unwrap_or(&new).to_path_buf();
                let rewritten = cx
                    .background_spawn(async move {
                        super::rewrite_links(&brain, &old_relative, &new_relative)
                    })
                    .await;
                let message = match rewritten {
                    Ok(0) => format!("Renamed to {file_name}."),
                    Ok(1) => format!("Renamed to {file_name}; one note's link updated."),
                    Ok(count) => format!("Renamed to {file_name}; links in {count} notes updated."),
                    Err(error) => format!(
                        "Renamed to {file_name}, but links to it couldn't be updated: {error:#}"
                    ),
                };
                notice(message, cx);
                this.update(cx, |this, cx| {
                    this.path = new;
                    cx.emit(ItemEvent::UpdateTab);
                    this.send_links(cx);
                })
                .ok();
            })
            .detach();
        }

        fn save_image(&mut self, request: u64, mime: String, data: String, cx: &mut Context<Self>) {
            let note = self.path.clone();
            cx.spawn(async move |this, cx| {
                let result = cx
                    .background_spawn(async move {
                        let bytes = base64::engine::general_purpose::STANDARD.decode(data)?;
                        super::save_note_image(&note, &mime, &bytes, chrono::Local::now())
                    })
                    .await;
                this.update(cx, |this, _| match result {
                    Ok(src) => this.send(json!({"type": "imageSaved", "request": request, "src": src})),
                    Err(error) => this.send(json!({"type": "imageFailed", "request": request, "message": format!("{error:#}")})),
                })
                .ok();
            })
            .detach();
        }

        fn handle(&mut self, message: Message, window: &mut Window, cx: &mut Context<Self>) {
            match message {
                Message::Ready => {
                    self.ready = true;
                    self.page_text = None;
                    self.load_from_buffer(cx);
                    self.send_links(cx);
                    if self.focus_handle.is_focused(window) {
                        if let Some(webview) = &self.webview {
                            webview.focus().log_err();
                        }
                        self.send(json!({"type": "focus"}));
                    }
                }
                Message::Save { revision, markdown } => {
                    self.apply_page_text(revision, markdown, cx)
                }
                Message::Open { href } => self.open_link(href, window, cx),
                Message::Error { message } => {
                    log::error!(
                        "Note editor: {}",
                        message.chars().take(500).collect::<String>()
                    )
                }
                Message::Source => self.show_source(window, cx),
                Message::Claude { action, text } => self.ask_claude(&action, &text, window, cx),
                Message::Rename { heading } => self.rename_to_heading(&heading, cx),
                Message::CopyPath => {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                        self.path.to_string_lossy().into_owned(),
                    ));
                    self.send(json!({"type": "pathCopied"}));
                }
                Message::History => {
                    // Adding a tab reads every item in the pane, this one
                    // included, so it waits until this update has finished.
                    let note = self.path.clone();
                    let workspace = self.workspace.clone();
                    window.defer(cx, move |window, cx| {
                        workspace
                            .update(cx, |workspace, cx| {
                                crate::note_history::open(workspace, note, window, cx)
                            })
                            .log_err();
                    });
                }
                Message::Image {
                    request,
                    mime,
                    data,
                } => self.save_image(request, mime, data, cx),
                Message::CopyImage { data } => {
                    match base64::engine::general_purpose::STANDARD.decode(data) {
                        Ok(bytes) if bytes.starts_with(b"\x89PNG\r\n\x1a\n") => {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_image(
                                &gpui::Image::from_bytes(gpui::ImageFormat::Png, bytes),
                            ));
                        }
                        _ => log::warn!("Note editor sent an image that is not a PNG"),
                    }
                }
            }
        }
    }

    fn asset_response(
        root: &std::path::Path,
        note: &std::path::Path,
        brain: &std::path::Path,
        request: wry::http::Request<Vec<u8>>,
    ) -> wry::http::Response<Cow<'static, [u8]>> {
        let read = || -> Result<(Vec<u8>, String)> {
            anyhow::ensure!(
                request.method() == "GET" && request.uri().host() == Some("app"),
                "Invalid asset request"
            );
            let relative = request.uri().path().trim_start_matches('/');
            if relative == "brain-file" {
                let src =
                    url::form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes())
                        .find(|(key, _)| key == "path")
                        .map(|(_, value)| value.into_owned())
                        .context("no image path")?;
                let (bytes, mime) = super::read_brain_image(note, brain, &src)?;
                return Ok((bytes, mime.to_owned()));
            }
            crate::whiteboard::native::bundle_asset(
                root,
                if relative.is_empty() {
                    "note.html"
                } else {
                    relative
                },
            )
            .context("note editor asset")
        };
        let mut response = wry::http::Response::new(Cow::Owned(Vec::new()));
        match read() {
            Ok((bytes, mime)) => {
                *response.body_mut() = Cow::Owned(bytes);
                if let Ok(value) = wry::http::HeaderValue::from_str(&mime) {
                    response
                        .headers_mut()
                        .insert(wry::http::header::CONTENT_TYPE, value);
                }
            }
            Err(_) => *response.status_mut() = wry::http::StatusCode::NOT_FOUND,
        }
        response
    }

    impl EventEmitter<ItemEvent> for RichNoteView {}

    impl Focusable for RichNoteView {
        fn focus_handle(&self, _: &App) -> FocusHandle {
            self.focus_handle.clone()
        }
    }

    impl Item for RichNoteView {
        type Event = ItemEvent;

        fn to_item_events(event: &ItemEvent, emit: &mut dyn FnMut(ItemEvent)) {
            emit(*event);
        }

        fn tab_content_text(&self, _: usize, _: &App) -> SharedString {
            self.path
                .file_stem()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Note".into())
                .into()
        }

        fn tab_tooltip_text(&self, _: &App) -> Option<SharedString> {
            Some(self.path.to_string_lossy().into_owned().into())
        }

        fn tab_icon(&self, _: &Window, _: &App) -> Option<Icon> {
            Some(Icon::new(IconName::FileTextOutlined))
        }

        fn for_each_project_item(
            &self,
            cx: &App,
            visit: &mut dyn FnMut(gpui::EntityId, &dyn project::ProjectItem),
        ) {
            visit(self.buffer.entity_id(), self.buffer.read(cx));
        }

        fn buffer_kind(&self, _: &App) -> ItemBufferKind {
            ItemBufferKind::Singleton
        }

        fn show_toolbar(&self) -> bool {
            false
        }

        fn deactivated(&mut self, _: &mut Window, _: &mut Context<Self>) {
            self.hide();
        }

        fn workspace_deactivated(&mut self, _: &mut Window, _: &mut Context<Self>) {
            self.hide();
        }

        fn on_removed(&self, _: &mut Context<Self>) {
            self.hide();
        }
    }

    impl Render for RichNoteView {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            if self.webview.is_none() && self.error.is_none() {
                if let Err(error) = self.create_webview(window, cx) {
                    self.error = Some(format!("The note editor could not start: {error}").into());
                }
            }
            let mut body = div()
                .id("brainz-rich-note")
                .key_context("BrainzRichNote")
                .on_action(cx.listener(|this, _: &ShowMarkdownSource, window, cx| {
                    this.show_source(window, cx)
                }))
                .size_full()
                .overflow_hidden()
                .track_focus(&self.focus_handle)
                .bg(cx.theme().colors().editor_background);
            if let Some(error) = &self.error {
                self.hide();
                return body.child(
                    v_flex()
                        .p_8()
                        .gap_3()
                        .child(Label::new(error.clone()))
                        .child(
                            Button::new("brainz-note-source", "Open the Markdown instead")
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.show_source(window, cx)),
                                ),
                        ),
                );
            }
            let Some(webview) = self.webview.clone() else {
                return body;
            };
            let modal_open = self
                .workspace
                .upgrade()
                .is_some_and(|workspace| workspace.read(cx).has_active_modal(window, cx));
            // The page is a native view above GPUI, so popovers and modals
            // would sit behind it; step aside while one is open.
            if modal_open || ui::has_open_popover(window, cx) || cx.has_active_drag() {
                self.hide();
                return body;
            }
            let view_bounds = self.bounds.clone();
            body = body.child(
                gpui::canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        view_bounds.set(bounds);
                        use wry::dpi::{LogicalPosition, LogicalSize};
                        webview
                            .set_bounds(wry::Rect {
                                position: LogicalPosition::new(
                                    f64::from(bounds.origin.x),
                                    f64::from(bounds.origin.y),
                                )
                                .into(),
                                size: LogicalSize::new(
                                    f64::from(bounds.size.width),
                                    f64::from(bounds.size.height),
                                )
                                .into(),
                            })
                            .log_err();
                        webview.set_visible(true).log_err();
                        window.on_mouse_event(move |event: &gpui::MouseDownEvent, phase, _, _| {
                            if phase.capture() && !bounds.contains(&event.position) {
                                webview.focus_parent().log_err();
                            }
                        });
                    },
                )
                .size_full(),
            );
            body
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    #[allow(
        clippy::disallowed_methods,
        reason = "a test setting up a git repository"
    )]
    fn backlinks_resolve_each_link_to_this_note() {
        let brain = tempfile::tempdir().unwrap();
        let root = brain.path();
        let git = |args: &[&str]| {
            assert!(
                crate::github_sync::command(root, "git")
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            )
        };
        git(&["init", "-q"]);
        std::fs::create_dir_all(root.join("people")).unwrap();
        std::fs::create_dir_all(root.join("work")).unwrap();
        std::fs::create_dir_all(root.join("other")).unwrap();
        std::fs::write(root.join("people/Sam Lee.md"), "# Sam Lee\n").unwrap();
        std::fs::write(root.join("other/Sam Lee.md"), "# Another Sam\n").unwrap();
        std::fs::write(
            root.join("work/plan.md"),
            "# Q4 plan\n\nTalk to **[Sam](../people/Sam%20Lee.md)** about the budget.\n",
        )
        .unwrap();
        std::fs::write(
            root.join("work/other.md"),
            "See [Sam](../other/Sam Lee.md).\n",
        )
        .unwrap();
        std::fs::write(root.join("people/team.md"), "- [Sam](<Sam Lee.md>) leads\n").unwrap();

        let found = backlinks(root, &root.join("people/Sam Lee.md")).unwrap();
        assert_eq!(
            found,
            vec![
                Backlink {
                    path: "people/team.md".into(),
                    title: "team".into(),
                    snippet: "Sam leads".into(),
                },
                Backlink {
                    path: "work/plan.md".into(),
                    title: "Q4 plan".into(),
                    snippet: "Talk to Sam about the budget.".into(),
                },
            ]
        );
        let notes = brain_notes(root).unwrap();
        assert!(notes.contains(&NoteEntry {
            path: "work/plan.md".into(),
            title: "Q4 plan".into()
        }));
    }

    #[test]
    fn claude_requests_name_the_note_and_keep_the_passage_whole() {
        let (rewrite, send) =
            claude_request("rewrite", "work/plan.md", "Use ```code``` here", "TODO.md").unwrap();
        assert!(send);
        assert!(rewrite.contains("`work/plan.md`"));
        assert!(rewrite.contains("````markdown\nUse ```code``` here\n````"));
        let (todos, _) =
            claude_request("todos", "work/plan.md", "Call Sam", "ops/TODO.md").unwrap();
        assert!(todos.contains("`ops/TODO.md`"));
        assert!(todos.contains("Don't edit the note"));
        let (ask, send) = claude_request("ask", "work/plan.md", "Call Sam", "TODO.md").unwrap();
        assert!(!send);
        assert!(ask.ends_with("```\n\n"));
        assert!(claude_request("delete everything", "a.md", "x", "TODO.md").is_none());
    }

    #[test]
    fn headings_become_safe_file_names() {
        assert_eq!(
            file_name_for_heading("Q4 plan: budget / hiring").as_deref(),
            Some("Q4 plan- budget - hiring.md")
        );
        assert_eq!(
            file_name_for_heading("  Sam   Lee  ").as_deref(),
            Some("Sam Lee.md")
        );
        assert_eq!(file_name_for_heading("...").as_deref(), None);
        assert_eq!(file_name_for_heading("").as_deref(), None);
    }

    #[test]
    #[allow(
        clippy::disallowed_methods,
        reason = "a test setting up a git repository"
    )]
    fn renaming_a_note_rewrites_the_links_to_it() {
        let brain = tempfile::tempdir().unwrap();
        let root = brain.path();
        assert!(
            crate::github_sync::command(root, "git")
                .args(["init", "-q"])
                .status()
                .unwrap()
                .success()
        );
        std::fs::create_dir_all(root.join("people")).unwrap();
        std::fs::create_dir_all(root.join("work")).unwrap();
        std::fs::write(root.join("people/sam.md"), "# Sam Lee\n").unwrap();
        std::fs::write(root.join("people/other.md"), "# Other\n").unwrap();
        std::fs::write(
            root.join("work/plan.md"),
            "Talk to [Sam](../people/sam.md#notes) and [Other](../people/other.md).\n",
        )
        .unwrap();
        std::fs::write(root.join("people/team.md"), "- [Sam](<sam.md>) leads\n").unwrap();
        std::fs::rename(root.join("people/sam.md"), root.join("people/Sam Lee.md")).unwrap();

        let changed = rewrite_links(
            root,
            std::path::Path::new("people/sam.md"),
            std::path::Path::new("people/Sam Lee.md"),
        )
        .unwrap();
        assert_eq!(changed, 2);
        assert_eq!(
            std::fs::read_to_string(root.join("work/plan.md")).unwrap(),
            "Talk to [Sam](../people/Sam%20Lee.md#notes) and [Other](../people/other.md).\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("people/team.md")).unwrap(),
            "- [Sam](Sam%20Lee.md) leads\n"
        );
    }

    #[test]
    fn only_image_files_inside_the_brain_are_served() {
        let brain = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let note = brain.path().join("trips/rome.md");
        std::fs::create_dir_all(brain.path().join("trips/attachments")).unwrap();
        std::fs::write(&note, "# Rome").unwrap();
        std::fs::write(brain.path().join("trips/attachments/view one.png"), b"png").unwrap();
        std::fs::write(brain.path().join("secret.md"), b"text").unwrap();
        std::fs::write(outside.path().join("other.png"), b"png").unwrap();

        let (bytes, mime) =
            read_brain_image(&note, brain.path(), "attachments/view%20one.png").unwrap();
        assert_eq!((bytes.as_slice(), mime), (b"png".as_slice(), "image/png"));
        assert!(read_brain_image(&note, brain.path(), "attachments/view one.png").is_ok());
        assert!(read_brain_image(&note, brain.path(), "../secret.md").is_err());
        let escape = format!("{}", outside.path().join("other.png").display());
        assert!(read_brain_image(&note, brain.path(), &escape).is_err());
        assert!(read_brain_image(&note, brain.path(), "../../other.png").is_err());
        assert!(read_brain_image(&note, brain.path(), "attachments/missing.png").is_err());
    }

    #[test]
    fn pasted_images_land_in_attachments_with_unique_names() {
        use chrono::TimeZone as _;
        let brain = tempfile::tempdir().unwrap();
        let note = brain.path().join("Trip Plan.md");
        let now = chrono::Local
            .with_ymd_and_hms(2026, 10, 22, 17, 5, 9)
            .unwrap();
        let first = save_note_image(&note, "image/png", b"one", now).unwrap();
        let second = save_note_image(&note, "image/png", b"two", now).unwrap();
        assert_eq!(first, "attachments/trip-plan-20261022-170509.png");
        assert_eq!(second, "attachments/trip-plan-20261022-170509-2.png");
        assert_eq!(std::fs::read(brain.path().join(&second)).unwrap(), b"two");
        assert!(save_note_image(&note, "text/html", b"<b>", now).is_err());
    }

    #[test]
    fn links_resolve_against_the_note_folder() {
        let note = Path::new("/brain/people/sam.md");
        assert_eq!(
            resolve_note_link(note, "../projects/q4.md#goals"),
            Some(Path::new("/brain/people/../projects/q4.md").to_path_buf())
        );
        assert_eq!(
            resolve_note_link(note, "Coffee%20notes.md"),
            Some(Path::new("/brain/people/Coffee notes.md").to_path_buf())
        );
        assert_eq!(resolve_note_link(note, "#top"), None);
        assert_eq!(
            resolve_note_link(note, "/tmp/x.md"),
            Some(Path::new("/tmp/x.md").to_path_buf())
        );
    }

    #[test]
    fn web_and_mail_links_leave_brainz() {
        assert!(is_external_link("https://example.com"));
        assert!(is_external_link(" MAILTO:sam@example.com"));
        assert!(!is_external_link("notes/plan.md"));
    }
}
