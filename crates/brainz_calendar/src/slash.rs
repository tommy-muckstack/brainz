use std::{cell::RefCell, ops::Range, path::PathBuf, rc::Rc, sync::Arc};

use anyhow::Result;
use brainz_templates::{InsertMode, Registry};
use collections::HashSet;
use editor::{CompletionProvider, Editor, MultiBufferOffset, scroll::Autoscroll};
use gpui::{App, AppContext, Context, Entity, Task, WeakEntity, Window};
use language::{Buffer, CodeLabel};
use lsp::CompletionContext;
use project::lsp_store::CompletionDocumentation;
use project::{Completion, CompletionGroup, CompletionResponse, CompletionSource};
use text::{Anchor, ToOffset, ToPoint};
use util::ResultExt;
use workspace::Workspace;

pub fn init(cx: &mut App) {
    cx.observe_new(|_: &mut Workspace, window, cx| {
        let Some(window) = window else { return };
        let workspace = cx.entity();
        let mut attached = HashSet::default();
        cx.subscribe_in(&workspace, window, move |workspace, _, event, _, cx| {
            let workspace::Event::ItemAdded { item } = event else {
                return;
            };
            let Some(editor) = item.downcast::<Editor>() else {
                return;
            };
            if !workspace.project().read(cx).is_local() || !attached.insert(editor.entity_id()) {
                return;
            }
            let roots = workspace.root_paths(cx);
            let Some(buffer) = editor.read(cx).buffer().read(cx).as_singleton() else {
                return;
            };
            let Some(path) = buffer
                .read(cx)
                .file()
                .and_then(|file| file.as_local())
                .map(|file| file.abs_path(cx))
            else {
                return;
            };
            let Some(root) = roots.into_iter().find(|root| path.starts_with(root)) else {
                return;
            };
            let owner = cx.weak_entity();
            editor.update(cx, |editor, cx| {
                let fallback = editor.completion_provider();
                editor.set_completion_provider(Some(Rc::new(NoteCompletionProvider {
                    root: root.to_path_buf(),
                    workspace: owner,
                    editor: cx.weak_entity(),
                    fallback,
                })));
            });
        })
        .detach();
    })
    .detach();
}

struct NoteCompletionProvider {
    root: PathBuf,
    workspace: WeakEntity<Workspace>,
    editor: WeakEntity<Editor>,
    fallback: Option<Rc<dyn CompletionProvider>>,
}

fn slash_token_range(text: &str, position: usize) -> Option<Range<usize>> {
    let prefix = text.get(..position)?;
    let line_start = prefix.rfind('\n').map_or(0, |offset| offset + 1);
    let reference = ["/link ", "/person "]
        .into_iter()
        .filter_map(|command| {
            let offset = prefix.get(line_start..)?.rfind(command)? + line_start;
            (offset == 0 || prefix.get(..offset)?.chars().next_back()?.is_whitespace())
                .then_some(offset)
        })
        .max();
    let start = reference.or_else(|| prefix.rfind('/'))?;
    if start > 0 && !prefix.get(..start)?.chars().next_back()?.is_whitespace() {
        return None;
    }
    if reference.is_none()
        && !prefix
            .get(start + 1..)?
            .chars()
            .all(|character| character.is_alphanumeric() || character == '-' || character == '_')
    {
        return None;
    }
    Some(start..position)
}

// Parse the Markdown prefix as well as the token: an unfinished fence or backtick
// span must keep ordinary typing literal, even before the syntax tree catches up.
fn slash_range(text: &str, position: usize) -> Option<Range<usize>> {
    let token = slash_token_range(text, position)?;
    let prefix = text.get(..position)?;
    let start = token.start;
    for (event, range) in pulldown_cmark::Parser::new(prefix).into_offset_iter() {
        if range.start <= start
            && start < range.end
            && matches!(
                event,
                pulldown_cmark::Event::Code(_)
                    | pulldown_cmark::Event::Start(pulldown_cmark::Tag::CodeBlock(_))
            )
        {
            return None;
        }
    }
    let paragraph = prefix.get(..start)?.rsplit("\n\n").next()?;
    let mut characters = paragraph.chars().peekable();
    let mut backticks = 0;
    while let Some(character) = characters.next() {
        if character == '\\' {
            characters.next();
        } else if character == '`' {
            let mut count = 1;
            while characters.peek() == Some(&'`') {
                characters.next();
                count += 1;
            }
            if backticks == count {
                backticks = 0;
            } else if backticks == 0 {
                backticks = count;
            }
        }
    }
    (backticks == 0).then_some(start..position)
}

impl NoteCompletionProvider {
    fn reference_completions(
        &self,
        buffer: &Entity<Buffer>,
        range: Range<Anchor>,
        people_only: bool,
        cx: &mut Context<Editor>,
    ) -> Task<Result<Vec<CompletionResponse>>> {
        let Some(workspace) = self.workspace.upgrade() else {
            return Task::ready(Ok(Vec::new()));
        };
        let Some(note) = buffer
            .read(cx)
            .file()
            .and_then(|file| file.as_local())
            .map(|file| file.abs_path(cx))
        else {
            return Task::ready(Ok(Vec::new()));
        };
        let project = workspace.read(cx).project().read(cx);
        let mut files = Vec::new();
        for worktree in project.visible_worktrees(cx) {
            let worktree = worktree.read(cx);
            for entry in worktree.entries(false, 0).filter(|entry| !entry.is_dir()) {
                let absolute = worktree.absolutize(&entry.path);
                let Ok(relative) = absolute.strip_prefix(&self.root) else {
                    continue;
                };
                if absolute == note
                    || relative
                        .components()
                        .any(|part| part.as_os_str().to_string_lossy().starts_with('.'))
                {
                    continue;
                }
                if matches!(
                    relative
                        .extension()
                        .and_then(|extension| extension.to_str()),
                    Some("md" | "markdown" | "pdf" | "canvas")
                ) {
                    files.push(relative.to_path_buf());
                }
            }
        }
        let root = self.root.clone();
        cx.background_spawn(async move {
            let note = note.strip_prefix(&root)?;
            let people_dir = super::brain_config::BrainConfig::load(&root).people_dir;
            let shortcut = if people_only { "person" } else { "link" };
            let mut completions = Vec::new();
            for path in files {
                if people_only
                    && (!path.starts_with(&people_dir)
                        || path.extension().and_then(|extension| extension.to_str()) != Some("md"))
                {
                    continue;
                }
                let destination = path.to_string_lossy();
                let title = path
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().replace(['-', '_'], " "))
                    .unwrap_or_default();
                let label = title
                    .replace('\\', "\\\\")
                    .replace('[', "\\[")
                    .replace(']', "\\]");
                let url = brainz_templates::link_destination(note, &destination)?;
                completions.push(item(
                    format!("/{shortcut} {title} · {destination}"),
                    destination.into_owned(),
                    if people_only { "People" } else { "Files" },
                    range.clone(),
                    format!("[{label}]({url})"),
                ));
            }
            Ok(vec![CompletionResponse {
                completions,
                display_options: Default::default(),
                is_incomplete: true,
            }])
        })
    }

    fn state(
        &self,
        buffer: &Entity<Buffer>,
        position: Anchor,
        cx: &App,
    ) -> Option<(Range<Anchor>, String)> {
        let buffer = buffer.read(cx);
        let path = buffer.file()?.as_local()?.abs_path(cx);
        if !path.starts_with(&self.root)
            || !buffer
                .language()
                .is_some_and(|language| language.name().as_ref() == "Markdown")
        {
            return None;
        }
        let snapshot = buffer.snapshot();
        let point = position.to_point(&snapshot);
        let line = snapshot
            .text_for_range(text::Point::new(point.row, 0)..point)
            .collect::<String>();
        slash_token_range(&line, line.len())?;
        let position = position.to_offset(&snapshot);
        // Keep completion work bounded for generated Markdown dumps.
        if position > 2 * 1024 * 1024 {
            return None;
        }
        let text = snapshot.text_for_range(0..position).collect::<String>();
        let range = slash_range(&text, position)?;
        Some((
            snapshot.anchor_before(range.start)..snapshot.anchor_after(range.end),
            text.get(range)?.to_owned(),
        ))
    }
}

impl CompletionProvider for NoteCompletionProvider {
    fn completions(
        &self,
        buffer: &Entity<Buffer>,
        position: Anchor,
        trigger: CompletionContext,
        window: &mut Window,
        cx: &mut Context<Editor>,
    ) -> Task<Result<Vec<CompletionResponse>>> {
        let Some((range, query)) = self.state(buffer, position, cx) else {
            return self
                .fallback
                .as_ref()
                .map(|fallback| fallback.completions(buffer, position, trigger, window, cx))
                .unwrap_or_else(|| Task::ready(Ok(Vec::new())));
        };
        if query.starts_with("/link ") || query.starts_with("/person ") {
            return self.reference_completions(buffer, range, query.starts_with("/person "), cx);
        }
        let root = self.root.clone();
        let workspace = self.workspace.clone();
        let editor = self.editor.clone();
        let buffer = buffer.clone();
        let load = cx.background_spawn(async move { Registry::load_brain(&root, None) });
        cx.spawn(async move |_, _| {
            let registry = match load.await {
                Ok(registry) => registry,
                Err(error) => {
                    log::warn!("Could not load note template completions: {error:#}");
                    Registry::builtin()?
                }
            };
            let mut completions = Vec::new();
            for template in registry
                .templates
                .into_values()
                .filter(|template| template.metadata.modes.contains(&InsertMode::Insert))
            {
                let name = format!("/{} · {}", template.metadata.id, template.metadata.name);
                let mut completion = item(
                    name,
                    template.metadata.description,
                    "Templates",
                    range.clone(),
                    query.clone(),
                );
                let template_id = template.metadata.id;
                let workspace = workspace.clone();
                let editor = editor.clone();
                let buffer = buffer.clone();
                let range = range.clone();
                let expected = query.clone();
                completion.confirm = Some(Arc::new(move |_, window, cx| {
                    let workspace = workspace.clone();
                    let target = NoteInsertion {
                        editor: editor.clone(),
                        buffer: buffer.clone(),
                        range: range.clone(),
                        expected: expected.clone(),
                    };
                    let template_id = template_id.clone();
                    window.defer(cx, move |window, cx| {
                        workspace
                            .update(cx, |workspace, cx| {
                                super::new_note::insert_template(
                                    workspace,
                                    target,
                                    template_id,
                                    window,
                                    cx,
                                )
                            })
                            .log_err();
                    });
                    false
                }));
                completions.push(completion);
            }
            for (shortcut, description, body, cursor) in blocks() {
                let mut completion = item(
                    format!("/{shortcut}"),
                    description.into(),
                    "Blocks",
                    range.clone(),
                    body,
                );
                let editor = editor.clone();
                let buffer = buffer.clone();
                let start = range.start;
                completion.confirm = Some(Arc::new(move |_, window, cx| {
                    let editor = editor.clone();
                    let buffer = buffer.clone();
                    window.defer(cx, move |window, cx| {
                        editor
                            .update(cx, |editor, cx| {
                                if editor.buffer().read(cx).as_singleton().as_ref() != Some(&buffer)
                                {
                                    return;
                                }
                                let offset = start.to_offset(&buffer.read(cx).snapshot()) + cursor;
                                editor.change_selections(
                                    Some(Autoscroll::fit()).into(),
                                    window,
                                    cx,
                                    |selections| {
                                        selections
                                            .select_ranges([MultiBufferOffset(offset)
                                                ..MultiBufferOffset(offset)])
                                    },
                                );
                            })
                            .log_err();
                    });
                    false
                }));
                completions.push(completion);
            }
            for (shortcut, description) in [
                ("link", "Choose a file in this brain"),
                ("person", "Choose a person in this brain"),
            ] {
                let mut completion = item(
                    format!("/{shortcut}"),
                    description.into(),
                    "Blocks",
                    range.clone(),
                    format!("/{shortcut} "),
                );
                completion.confirm = Some(Arc::new(|_, _, _| true));
                completions.push(completion);
            }
            Ok(vec![CompletionResponse {
                completions,
                display_options: Default::default(),
                is_incomplete: true,
            }])
        })
    }

    fn is_completion_trigger(
        &self,
        buffer: &Entity<Buffer>,
        position: Anchor,
        text: &str,
        trigger_in_words: bool,
        cx: &mut Context<Editor>,
    ) -> bool {
        self.state(buffer, position, cx).is_some()
            || self.fallback.as_ref().is_some_and(|fallback| {
                fallback.is_completion_trigger(buffer, position, text, trigger_in_words, cx)
            })
    }

    fn resolve_completions(
        &self,
        buffer: Entity<Buffer>,
        indices: Vec<usize>,
        completions: Rc<RefCell<Box<[Completion]>>>,
        cx: &mut Context<Editor>,
    ) -> Task<Result<bool>> {
        if own_completions(&completions) {
            return Task::ready(Ok(false));
        }
        self.fallback
            .as_ref()
            .map(|fallback| fallback.resolve_completions(buffer, indices, completions, cx))
            .unwrap_or_else(|| Task::ready(Ok(false)))
    }

    fn apply_additional_edits_for_completion(
        &self,
        buffer: Entity<Buffer>,
        completions: Rc<RefCell<Box<[Completion]>>>,
        index: usize,
        push_to_history: bool,
        ranges: Vec<Range<Anchor>>,
        cx: &mut Context<Editor>,
    ) -> Task<Result<Option<language::Transaction>>> {
        if own_completions(&completions) {
            return Task::ready(Ok(None));
        }
        self.fallback
            .as_ref()
            .map(|fallback| {
                fallback.apply_additional_edits_for_completion(
                    buffer,
                    completions,
                    index,
                    push_to_history,
                    ranges,
                    cx,
                )
            })
            .unwrap_or_else(|| Task::ready(Ok(None)))
    }

    fn selection_changed(
        &self,
        matched: Option<&fuzzy::StringMatch>,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(fallback) = &self.fallback {
            fallback.selection_changed(matched, window, cx);
        }
    }
    fn show_snippets(&self) -> bool {
        self.fallback
            .as_ref()
            .is_some_and(|fallback| fallback.show_snippets())
    }
}

fn own_completions(completions: &RefCell<Box<[Completion]>>) -> bool {
    completions
        .borrow()
        .first()
        .and_then(|completion| completion.group.as_ref())
        .is_some_and(|group| group.key.starts_with("brainz-note-"))
}

fn item(
    name: String,
    description: String,
    group: &str,
    range: Range<Anchor>,
    body: String,
) -> Completion {
    Completion {
        label: CodeLabel {
            filter_range: 0..name.len(),
            text: name,
            runs: Vec::new(),
        },
        new_text: body,
        match_start: Some(range.start),
        replace_range: range,
        documentation: Some(CompletionDocumentation::MultiLinePlainText(
            description.into(),
        )),
        source: CompletionSource::Custom,
        icon_path: None,
        icon_color: None,
        snippet_deduplication_key: None,
        insert_text_mode: Some(lsp::InsertTextMode::AS_IS),
        confirm: None,
        group: Some(CompletionGroup {
            key: format!("brainz-note-{group}").into(),
            label: Some(group.to_owned().into()),
        }),
    }
}

fn blocks() -> Vec<(&'static str, &'static str, String, usize)> {
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let callout = format!("> [!NOTE]\n> {date} · ");
    vec![
        (
            "callout",
            "Dated note callout",
            callout.clone(),
            callout.len(),
        ),
        ("todo", "Checkbox task", "- [ ] ".into(), 6),
        ("date", "Today's date", date.clone(), date.len()),
        (
            "table",
            "Two-column Markdown table",
            "| Heading | Heading |\n| --- | --- |\n|  |  |\n".into(),
            2,
        ),
        (
            "mermaid",
            "Editable flow diagram",
            "```mermaid\nflowchart LR\n    A[Start] --> B[Next]\n```\n".into(),
            "```mermaid\nflowchart LR\n    A[".len(),
        ),
    ]
}

pub(super) struct NoteInsertion {
    editor: WeakEntity<Editor>,
    buffer: Entity<Buffer>,
    range: Range<Anchor>,
    expected: String,
}

impl NoteInsertion {
    pub(super) fn new(
        editor: &Entity<Editor>,
        range: Range<Anchor>,
        expected: String,
        cx: &App,
    ) -> Option<Self> {
        Some(Self {
            editor: editor.downgrade(),
            buffer: editor.read(cx).buffer().read(cx).as_singleton()?,
            range,
            expected,
        })
    }

    pub fn path(&self, cx: &App) -> Option<PathBuf> {
        Some(self.buffer.read(cx).file()?.as_local()?.abs_path(cx))
    }

    pub fn apply(
        &self,
        rendered: &brainz_templates::Rendered,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<()> {
        self.editor.update(cx, |editor, cx| {
            anyhow::ensure!(!editor.read_only(cx), "This note is read-only");
            anyhow::ensure!(
                editor.buffer().read(cx).as_singleton().as_ref() == Some(&self.buffer),
                "The note is no longer open"
            );
            let snapshot = self.buffer.read(cx).snapshot();
            let range = self.range.start.to_offset(&snapshot)..self.range.end.to_offset(&snapshot);
            anyhow::ensure!(
                snapshot.text_for_range(range.clone()).collect::<String>() == self.expected,
                "This part of the note changed. Close this picker and insert again."
            );
            let cursor = range.start + rendered.cursor.unwrap_or(rendered.body.len());
            editor.transact(window, cx, |editor, window, cx| {
                self.buffer.update(cx, |buffer, cx| {
                    buffer.edit([(range, rendered.body.as_str())], None, cx)
                });
                editor.change_selections(
                    Some(Autoscroll::fit()).into(),
                    window,
                    cx,
                    |selections| {
                        selections
                            .select_ranges([MultiBufferOffset(cursor)..MultiBufferOffset(cursor)])
                    },
                );
            });
            Ok(())
        })?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn insertion_tracks_prose_edits_and_rejects_conflicts(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            cx.set_global(db::AppDatabase::test_new());
            workspace::AppState::test(cx);
            editor::init(cx);
        });
        let buffer = cx.new(|cx| Buffer::local("before /dec after", cx));
        let (editor, cx) =
            cx.add_window_view(|window, cx| Editor::for_buffer(buffer.clone(), None, window, cx));
        let target = cx.update(|_, cx| {
            let snapshot = buffer.read(cx).snapshot();
            NoteInsertion {
                editor: editor.downgrade(),
                buffer: buffer.clone(),
                range: snapshot.anchor_before(7)..snapshot.anchor_after(11),
                expected: "/dec".into(),
            }
        });
        let rendered = brainz_templates::Rendered {
            path: None,
            body: "new block".into(),
            cursor: Some(4),
            ai_blocks: Vec::new(),
            report: Default::default(),
            content_guard: "test",
        };
        buffer.update(cx, |buffer, cx| {
            buffer.edit([(0..0, "Earlier. ")], None, cx)
        });
        cx.update(|window, cx| {
            target
                .apply(&rendered, window, cx)
                .expect("tracked insertion")
        });
        assert_eq!(
            editor.read_with(cx, |editor, cx| editor.text(cx)),
            "Earlier. before new block after"
        );
        editor.update_in(cx, |editor, window, cx| {
            editor.undo(&editor::actions::Undo, window, cx)
        });
        assert_eq!(
            editor.read_with(cx, |editor, cx| editor.text(cx)),
            "Earlier. before /dec after"
        );
        editor.update(cx, |editor, _| editor.set_read_only(true));
        cx.update(|window, cx| {
            assert!(
                target
                    .apply(&rendered, window, cx)
                    .expect_err("read only")
                    .to_string()
                    .contains("read-only")
            )
        });
        editor.update(cx, |editor, _| editor.set_read_only(false));
        buffer.update(cx, |buffer, cx| {
            let snapshot = buffer.snapshot();
            let range =
                target.range.start.to_offset(&snapshot)..target.range.end.to_offset(&snapshot);
            buffer.edit([(range, "replacement")], None, cx);
        });
        cx.update(|window, cx| {
            assert!(
                target
                    .apply(&rendered, window, cx)
                    .expect_err("conflict")
                    .to_string()
                    .contains("changed")
            )
        });
        assert_eq!(
            editor.read_with(cx, |editor, cx| editor.text(cx)),
            "Earlier. before replacement after"
        );
    }

    #[test]
    fn only_prose_slashes_offer_commands() {
        for text in [
            "/",
            "/dec",
            "# Note\n\n/meeting",
            "hello /todo",
            "- /table",
            "✅ /date",
            "`done` /todo",
            "\\`literal /todo",
            "/link folder/note",
            "hello /person Ada Lovelace",
        ] {
            assert!(slash_range(text, text.len()).is_some(), "{text:?}");
        }
        for text in [
            "https://",
            "hello/",
            "//",
            "/a/b",
            "/two words",
            "` /todo",
            "`` code ` /todo",
            "```md\n/todo",
            "~~~\n/todo",
            "    /todo",
            "\t/todo",
            "\\/todo",
            "https://link /not a command",
            "` /link Ada",
        ] {
            assert!(slash_range(text, text.len()).is_none(), "{text:?}");
        }
        let code = "before ` /todo ` after";
        assert!(slash_range(code, 14).is_none());
        assert!(slash_range("✅", 1).is_none());
        let after_fence = "```md\ncode\n```\n\n/todo";
        assert!(slash_range(after_fence, after_fence.len()).is_some());
    }
}
