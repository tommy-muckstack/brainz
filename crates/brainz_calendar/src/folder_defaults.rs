use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};

use brainz_templates::{InsertMode, Registry, RenderRequest, Rendered, Template, TemplatesConfig};
use collections::HashSet;
use editor::{
    Addon, Editor,
    display_map::{BlockPlacement, BlockProperties, BlockStyle, CustomBlockId},
};
use gpui::{
    App, Entity, Focusable, KeyBinding, KeyContext, Subscription, Task, WeakEntity, Window, actions,
};
use serde_json::Value;
use ui::prelude::*;
use util::ResultExt;
use workspace::Workspace;

use super::{new_note, slash::NoteInsertion};

actions!(brainz_calendar, [UseFolderTemplate, DismissFolderTemplate]);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new(
            "enter",
            UseFolderTemplate,
            Some("Editor && BrainzFolderTemplate && !menu"),
        ),
        KeyBinding::new(
            "escape",
            DismissFolderTemplate,
            Some("Editor && BrainzFolderTemplate && !menu"),
        ),
    ]);
    cx.observe_new(|workspace: &mut Workspace, window, cx| {
        let Some(window) = window else { return };
        let created = Rc::new(RefCell::new(HashSet::<PathBuf>::default()));
        let armed = Rc::new(RefCell::new(HashSet::default()));
        let project = workspace.project().clone();
        if !project.read(cx).is_local() {
            return;
        }
        let worktrees = project
            .read(cx)
            .visible_worktrees(cx)
            .map(|worktree| worktree.read(cx).id())
            .collect::<Vec<_>>();
        for worktree_id in worktrees {
            arm_worktree(&project, worktree_id, armed.clone(), cx);
        }
        cx.subscribe_in(&project, window, {
            let created = created.clone();
            move |workspace, project, event, window, cx| {
                if let project::Event::WorktreeAdded(worktree_id) = event {
                    arm_worktree(&project, *worktree_id, armed.clone(), cx);
                    return;
                }
                let project::Event::WorktreeUpdatedEntries(worktree_id, entries) = event else {
                    return;
                };
                if !armed.borrow().contains(worktree_id) {
                    return;
                }
                let paths = project
                    .read(cx)
                    .worktree_for_id(*worktree_id, cx)
                    .map(|worktree| {
                        let worktree = worktree.read(cx);
                        entries
                            .iter()
                            .filter(|(_, _, change)| *change == project::PathChange::Added)
                            .map(|(path, _, _)| worktree.absolutize(path))
                            .filter(|path| is_note(path))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                {
                    let mut created = created.borrow_mut();
                    // Bound session state even when a large folder is imported.
                    if created.len() + paths.len() > 4096 {
                        created.clear();
                    }
                    created.extend(paths.iter().cloned());
                }
                let editors = workspace.items_of_type::<Editor>(cx).collect::<Vec<_>>();
                for editor in editors {
                    if let Some(path) = note_path(&editor, cx)
                        && paths.contains(&path)
                    {
                        attach(workspace, &editor, true, window, cx);
                        created.borrow_mut().remove(&path);
                    }
                }
            }
        })
        .detach();
        let owner = cx.entity();
        cx.subscribe_in(&owner, window, move |workspace, _, event, window, cx| {
            let workspace::Event::ItemAdded { item } = event else {
                return;
            };
            let Some(editor) = item.downcast::<Editor>() else {
                return;
            };
            let automatic =
                note_path(&editor, cx).is_some_and(|path| created.borrow_mut().remove(&path));
            attach(workspace, &editor, automatic, window, cx);
        })
        .detach();
    })
    .detach();
}

fn arm_worktree(
    project: &Entity<project::Project>,
    worktree_id: project::WorktreeId,
    armed: Rc<RefCell<HashSet<project::WorktreeId>>>,
    cx: &mut Context<Workspace>,
) {
    let Some(worktree) = project.read(cx).worktree_for_id(worktree_id, cx) else {
        return;
    };
    let Some(local) = worktree.read(cx).as_local() else {
        return;
    };
    let scanned = local.scan_complete();
    cx.spawn(async move |_, _| {
        scanned.await;
        armed.borrow_mut().insert(worktree_id);
    })
    .detach();
}

fn is_note(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("md" | "markdown")
    )
}

fn note_path(editor: &Entity<Editor>, cx: &App) -> Option<PathBuf> {
    let buffer = editor.read(cx).buffer().read(cx).as_singleton()?;
    let path = buffer.read(cx).file()?.as_local()?.abs_path(cx);
    is_note(&path).then_some(path)
}

fn note_insertion(editor: &Entity<Editor>, expected: String, cx: &App) -> Option<NoteInsertion> {
    let buffer = editor.read(cx).buffer().read(cx).as_singleton()?;
    let snapshot = buffer.read(cx).snapshot();
    NoteInsertion::new(
        editor,
        snapshot.anchor_before(0)..snapshot.anchor_after(snapshot.len()),
        expected,
        cx,
    )
}

fn attach(
    workspace: &mut Workspace,
    editor: &Entity<Editor>,
    newly_created: bool,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    if let Some(state) = editor
        .read(cx)
        .addon::<FolderAddon>()
        .map(|addon| addon.0.clone())
    {
        state.update(cx, |state, cx| {
            state.newly_created |= newly_created;
            state.maybe_apply_automatically(window, cx);
        });
        return;
    }
    if editor.read(cx).read_only(cx)
        || editor.read(cx).buffer().read(cx).len(cx) != editor::MultiBufferOffset(0)
    {
        return;
    }
    let Some(path) = note_path(editor, cx) else {
        return;
    };
    let Some(root) = workspace
        .root_paths(cx)
        .into_iter()
        .find(|root| path.starts_with(root))
    else {
        return;
    };
    if path.strip_prefix(&root).is_ok_and(|relative| {
        relative
            .components()
            .any(|component| component.as_os_str().to_string_lossy().starts_with('.'))
    }) {
        return;
    }
    let owner = cx.weak_entity();
    let state = cx.new(|cx| {
        FolderTemplate::new(
            editor,
            owner,
            root.to_path_buf(),
            path,
            newly_created,
            window,
            cx,
        )
    });
    editor.update(cx, |editor, _| editor.register_addon(FolderAddon(state)));
}

struct FolderAddon(Entity<FolderTemplate>);
impl Addon for FolderAddon {
    fn extend_key_context(&self, context: &mut KeyContext, cx: &App) {
        if matches!(self.0.read(cx).phase, Phase::Suggested) {
            context.add("BrainzFolderTemplate");
        }
    }
    fn to_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[derive(PartialEq)]
enum Phase {
    Loading,
    Suggested,
    Applied(String),
    Dismissed,
}

struct FolderTemplate {
    editor: WeakEntity<Editor>,
    workspace: WeakEntity<Workspace>,
    path: PathBuf,
    newly_created: bool,
    automatic: bool,
    phase: Phase,
    template: Option<Template>,
    rendered: Option<Rendered>,
    error: Option<String>,
    block: Option<CustomBlockId>,
    _subscriptions: Vec<Subscription>,
    _load: Task<()>,
}

impl FolderTemplate {
    fn new(
        editor: &Entity<Editor>,
        workspace: WeakEntity<Workspace>,
        root: PathBuf,
        path: PathBuf,
        newly_created: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut subscriptions = vec![cx.subscribe_in(
            editor,
            window,
            |state, editor, event: &editor::EditorEvent, _, cx| {
                if matches!(event, editor::EditorEvent::BufferEdited) {
                    if state.phase == Phase::Dismissed {
                        return;
                    }
                    let text = editor.read(cx).text(cx);
                    if match &state.phase {
                        Phase::Loading | Phase::Suggested => !text.is_empty(),
                        Phase::Applied(applied) => text != *applied,
                        Phase::Dismissed => false,
                    } {
                        state.dismiss(cx);
                    }
                }
            },
        )];
        let owner = cx.weak_entity();
        subscriptions.push(editor.update(cx, |editor, _| {
            editor.register_action(move |_: &UseFolderTemplate, window, cx| {
                owner
                    .update(cx, |state, cx| state.apply(window, cx))
                    .log_err();
            })
        }));
        let owner = cx.weak_entity();
        subscriptions.push(editor.update(cx, |editor, _| {
            editor.register_action(move |_: &DismissFolderTemplate, _, cx| {
                owner.update(cx, |state, cx| state.dismiss(cx)).log_err();
            })
        }));
        let load_root = root;
        let load_path = path.clone();
        let use_calendar = editor
            .read(cx)
            .project()
            .is_none_or(|project| !project.read(cx).fs().is_fake());
        let load = cx.spawn_in(window, async move |state, cx| {
            let result = cx
                .background_spawn(async move {
                    let registry = Registry::load_brain(&load_root, None)?;
                    let config = TemplatesConfig::load(&load_root)?;
                    let relative = load_path.strip_prefix(&load_root)?;
                    let Some(choice) = registry.default_for_path(&config, relative)? else {
                        return Ok::<_, anyhow::Error>(None);
                    };
                    let template = registry
                        .templates
                        .get(&choice.template)
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!("Folder template is unavailable"))?;
                    let title = relative
                        .file_stem()
                        .map(|title| title.to_string_lossy().replace(['-', '_'], " "))
                        .unwrap_or_default();
                    let mut request = RenderRequest {
                        title: title.clone(),
                        folder: relative
                            .parent()
                            .filter(|path| !path.as_os_str().is_empty())
                            .map(|path| path.to_string_lossy().into_owned())
                            .unwrap_or_else(|| ".".into()),
                        context: if use_calendar {
                            new_note::local_context(&load_root)
                        } else {
                            Default::default()
                        },
                        ..Default::default()
                    };
                    request.input.insert("title".into(), Value::String(title));
                    let rendered = new_note::render_in_note(&template, &request, relative)?;
                    Ok(Some((template, rendered, choice.auto)))
                })
                .await;
            state
                .update_in(cx, |state, window, cx| {
                    if state.phase == Phase::Dismissed {
                        return;
                    }
                    match result {
                        Ok(Some((template, rendered, automatic))) => {
                            state.template = Some(template);
                            state.rendered = Some(rendered);
                            state.automatic = automatic;
                            state.phase = Phase::Suggested;
                            state.show_bar(cx);
                            state.maybe_apply_automatically(window, cx);
                        }
                        Ok(None) => state.dismiss(cx),
                        Err(error) => {
                            state.error = Some(format!("Could not load folder template: {error}"));
                            state.phase = Phase::Suggested;
                            state.show_bar(cx);
                        }
                    }
                })
                .log_err();
        });
        Self {
            editor: editor.downgrade(),
            workspace,
            path,
            newly_created,
            automatic: false,
            phase: Phase::Loading,
            template: None,
            rendered: None,
            error: None,
            block: None,
            _subscriptions: subscriptions,
            _load: load,
        }
    }

    fn needs_input(&self) -> bool {
        self.template
            .as_ref()
            .zip(self.rendered.as_ref())
            .is_some_and(|(template, rendered)| {
                template.metadata.inputs.iter().any(|input| {
                    input.required
                        && rendered
                            .report
                            .missing
                            .contains(&format!("input.{}", input.id))
                })
            })
    }

    fn maybe_apply_automatically(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.newly_created
            && self.automatic
            && self.phase == Phase::Suggested
            && !self.needs_input()
            && self.error.is_none()
        {
            self.apply(window, cx);
        }
    }

    fn valid_editor(&self, cx: &App) -> Option<Entity<Editor>> {
        let editor = self.editor.upgrade()?;
        (!editor.read(cx).read_only(cx) && note_path(&editor, cx).as_ref() == Some(&self.path))
            .then_some(editor)
    }

    fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.phase != Phase::Suggested {
            return;
        }
        let Some(editor) = self.valid_editor(cx) else {
            self.dismiss(cx);
            return;
        };
        if editor.read(cx).buffer().read(cx).len(cx) != editor::MultiBufferOffset(0) {
            self.dismiss(cx);
            return;
        }
        if self.needs_input() {
            self.change(window, cx);
            return;
        }
        let Some(rendered) = self.rendered.as_ref() else {
            return;
        };
        let Some(target) = note_insertion(&editor, String::new(), cx) else {
            return;
        };
        match target.apply(rendered, window, cx) {
            Ok(()) => {
                self.phase = Phase::Applied(rendered.body.clone());
                self.error = None;
                window.focus(&editor.focus_handle(cx), cx);
            }
            Err(error) => self.error = Some(error.to_string()),
        }
        cx.notify();
    }

    fn undo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.valid_editor(cx) else {
            return;
        };
        if let Phase::Applied(body) = &self.phase
            && editor.read(cx).text(cx) == *body
        {
            editor.update(cx, |editor, cx| {
                editor.undo(&editor::actions::Undo, window, cx)
            });
            self.dismiss(cx);
        }
    }

    fn change(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.valid_editor(cx) else {
            self.dismiss(cx);
            return;
        };
        let expected = match &self.phase {
            Phase::Suggested => String::new(),
            Phase::Applied(body) => body.clone(),
            _ => return,
        };
        if editor.read(cx).text(cx) != expected {
            self.dismiss(cx);
            return;
        }
        let Some(target) = note_insertion(&editor, expected, cx) else {
            return;
        };
        let id = self
            .template
            .as_ref()
            .map(|template| template.metadata.id.clone())
            .unwrap_or_default();
        self.workspace
            .update(cx, |workspace, cx| {
                new_note::insert_template_with_mode(
                    workspace,
                    target,
                    id,
                    InsertMode::New,
                    window,
                    cx,
                )
            })
            .log_err();
    }

    fn dismiss(&mut self, cx: &mut Context<Self>) {
        self.phase = Phase::Dismissed;
        if let Some(block) = self.block.take() {
            self.editor
                .update(cx, |editor, cx| {
                    editor.remove_blocks(HashSet::from_iter([block]), None, cx)
                })
                .log_err();
        }
        cx.notify();
    }

    fn show_bar(&mut self, cx: &mut Context<Self>) {
        if self.block.is_some() {
            cx.notify();
            return;
        }
        let owner = cx.weak_entity();
        self.block = self
            .editor
            .update(cx, |editor, cx| {
                editor
                    .insert_blocks(
                        [BlockProperties {
                            placement: BlockPlacement::Above(editor::Anchor::Min),
                            height: Some(2),
                            style: BlockStyle::Sticky,
                            priority: 0,
                            render: Arc::new(move |_| {
                                owner.upgrade().map_or_else(
                                    || div().into_any_element(),
                                    |owner| owner.into_any_element(),
                                )
                            }),
                        }],
                        None,
                        cx,
                    )
                    .into_iter()
                    .next()
            })
            .log_err()
            .flatten();
        cx.notify();
    }
}

impl Render for FolderTemplate {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let applied = matches!(self.phase, Phase::Applied(_));
        let name = self
            .template
            .as_ref()
            .map(|template| template.metadata.name.as_str())
            .unwrap_or("note");
        h_flex()
            .h_full()
            .px_3()
            .gap_2()
            .bg(cx.theme().colors().editor_background)
            .child(
                Label::new(self.error.clone().unwrap_or_else(|| {
                    if applied {
                        format!("Used {name} template")
                    } else {
                        format!("Use {name} template?")
                    }
                }))
                .size(LabelSize::Small),
            )
            .when(self.error.is_none(), |row| {
                row.child(if applied {
                    Button::new("undo-folder-template", "Undo")
                        .on_click(cx.listener(|state, _, window, cx| state.undo(window, cx)))
                } else {
                    Button::new("use-folder-template", "Use ↵")
                        .on_click(cx.listener(|state, _, window, cx| state.apply(window, cx)))
                })
            })
            .child(
                Button::new("change-folder-template", "Change…")
                    .on_click(cx.listener(|state, _, window, cx| state.change(window, cx))),
            )
            .child(
                Button::new(
                    "dismiss-folder-template",
                    if applied { "Dismiss" } else { "Blank · Esc" },
                )
                .on_click(cx.listener(|state, _, _, cx| state.dismiss(cx))),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use project::Fs as _;
    use serde_json::json;

    async fn open_editor(
        workspace: &Entity<Workspace>,
        path: PathBuf,
        cx: &mut gpui::VisualTestContext,
    ) -> Entity<Editor> {
        let project = workspace.read_with(cx, |workspace, _| workspace.project().clone());
        let buffer = project
            .update(cx, |project, cx| project.open_local_buffer(path, cx))
            .await
            .expect("open fixture buffer");
        workspace.update_in(cx, |workspace, window, cx| {
            let editor = cx.new(|cx| Editor::for_buffer(buffer, Some(project), window, cx));
            workspace.add_item_to_active_pane(Box::new(editor.clone()), None, true, window, cx);
            window.focus(&editor.focus_handle(cx), cx);
            editor
        })
    }

    fn state(editor: &Entity<Editor>, cx: &App) -> Entity<FolderTemplate> {
        editor
            .read(cx)
            .addon::<FolderAddon>()
            .expect("folder template addon")
            .0
            .clone()
    }

    #[gpui::test]
    async fn empty_notes_suggest_or_apply_defaults_without_changing_existing_content(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            cx.set_global(db::AppDatabase::test_new());
            workspace::AppState::test(cx);
            editor::init(cx);
            init(cx);
        });
        let root = tempfile::tempdir().expect("temporary brain");
        std::fs::create_dir_all(root.path().join(".brainz/templates")).expect("fixture templates");
        let config = "[[templates.folder]]\npath = 'auto/*.md'\ntemplate = 'folder-note'\nauto = true\n[[templates.folder]]\npath = 'suggest/*.md'\ntemplate = 'folder-note'\n[[templates.folder]]\npath = 'needs/*.md'\ntemplate = 'needs-input'\nauto = true\n";
        std::fs::write(root.path().join("brainz.toml"), config).expect("configuration");
        std::fs::write(root.path().join(".brainz/templates/folder-note.md"), "---\nname: Folder note\npath: elsewhere/{{slug title}}.md\ninputs:\n  - id: title\n    required: true\n---\n# {{input.title}}\n\n[Reference]({{link \"reference.md\"}})\n\n{{cursor}}").expect("template");
        std::fs::write(root.path().join(".brainz/templates/needs-input.md"), "---\nname: Needs input\ninputs:\n  - id: person\n    required: true\n---\n# {{input.person}}\n").expect("required input template");
        let fs = project::FakeFs::new(cx.executor());
        fs.insert_tree(root.path(), json!({"brainz.toml":config,"auto":{"old.md":"","existing.md":"Preserve this note"},"suggest":{"blank.md":"","typing.md":"","external.md":""},"needs":{}})).await;
        let project = project::Project::test(fs.clone(), [root.path()], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        let existing = open_editor(&workspace, root.path().join("auto/existing.md"), cx).await;
        cx.run_until_parked();
        existing.read_with(cx, |editor, cx| {
            assert!(editor.addon::<FolderAddon>().is_none());
            assert_eq!(editor.text(cx), "Preserve this note");
        });
        let old = open_editor(&workspace, root.path().join("auto/old.md"), cx).await;
        cx.run_until_parked();
        old.read_with(cx, |editor, cx| {
            assert!(
                editor.text(cx).is_empty(),
                "opening an old empty note does not auto-fill it"
            );
            assert!(matches!(
                editor
                    .addon::<FolderAddon>()
                    .expect("suggestion")
                    .0
                    .read(cx)
                    .phase,
                Phase::Suggested
            ));
        });
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        cx.read(|cx| assert!(matches!(state(&old, cx).read(cx).phase, Phase::Dismissed)));

        let suggested = open_editor(&workspace, root.path().join("suggest/blank.md"), cx).await;
        cx.run_until_parked();
        suggested.read_with(cx, |editor, cx| assert!(editor.text(cx).is_empty()));
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        suggested.read_with(cx, |editor, cx| {
            assert_eq!(
                editor.text(cx),
                "# blank\n\n[Reference](../reference.md)\n\n"
            );
        });
        cx.dispatch_action(editor::actions::Undo);
        cx.run_until_parked();
        suggested.read_with(cx, |editor, cx| assert!(editor.text(cx).is_empty()));

        let path = root.path().join("auto/new-note.md");
        fs.atomic_write(path.clone(), String::new())
            .await
            .expect("new fixture note");
        cx.run_until_parked();
        let automatic = open_editor(&workspace, path, cx).await;
        cx.run_until_parked();
        automatic.read_with(cx, |editor, cx| {
            assert_eq!(
                editor.text(cx),
                "# new note\n\n[Reference](../reference.md)\n\n"
            );
            assert!(matches!(
                editor
                    .addon::<FolderAddon>()
                    .expect("used template bar")
                    .0
                    .read(cx)
                    .phase,
                Phase::Applied(_)
            ));
        });
        cx.update(|window, cx| {
            state(&automatic, cx).update(cx, |state, cx| state.undo(window, cx))
        });
        cx.run_until_parked();
        automatic.read_with(cx, |editor, cx| assert!(editor.text(cx).is_empty()));
        let reopened = open_editor(&workspace, root.path().join("auto/new-note.md"), cx).await;
        cx.run_until_parked();
        reopened.read_with(cx, |editor, cx| {
            assert!(
                editor.text(cx).is_empty(),
                "Undo and reopen must not reapply an automatic template"
            )
        });

        let typing = open_editor(&workspace, root.path().join("suggest/typing.md"), cx).await;
        typing.update_in(cx, |editor, window, cx| {
            editor.set_text("Already typing", window, cx)
        });
        cx.run_until_parked();
        typing.read_with(cx, |editor, cx| {
            assert_eq!(editor.text(cx), "Already typing")
        });
        let external = open_editor(&workspace, root.path().join("suggest/external.md"), cx).await;
        cx.run_until_parked();
        let external_state = cx.read(|cx| state(&external, cx));
        external.update(cx, |editor, cx| {
            editor
                .buffer()
                .read(cx)
                .as_singleton()
                .expect("buffer")
                .update(cx, |buffer, cx| {
                    buffer.edit([(0..0, "External edit")], None, cx)
                });
        });
        external_state.update_in(cx, |state, window, cx| state.apply(window, cx));
        cx.run_until_parked();
        external.read_with(cx, |editor, cx| {
            assert_eq!(editor.text(cx), "External edit")
        });
        let path = root.path().join("suggest/locked.md");
        fs.atomic_write(path.clone(), String::new())
            .await
            .expect("read-only fixture note");
        let locked = open_editor(&workspace, path, cx).await;
        cx.run_until_parked();
        locked.update(cx, |editor, _| editor.set_read_only(true));
        cx.update(|window, cx| state(&locked, cx).update(cx, |state, cx| state.apply(window, cx)));
        locked.read_with(cx, |editor, cx| assert!(editor.text(cx).is_empty()));
        let path = root.path().join("needs/person.md");
        fs.atomic_write(path.clone(), String::new())
            .await
            .expect("new note needing input");
        cx.run_until_parked();
        let required = open_editor(&workspace, path, cx).await;
        cx.run_until_parked();
        required.read_with(cx, |editor, cx| {
            assert!(editor.text(cx).is_empty());
            let state = editor
                .addon::<FolderAddon>()
                .expect("input suggestion")
                .0
                .read(cx);
            assert!(state.needs_input());
            assert!(matches!(state.phase, Phase::Suggested));
        });
        cx.update(|window, cx| {
            state(&required, cx).update(cx, |state, cx| state.apply(window, cx));
            assert!(workspace.read(cx).has_active_modal(window, cx));
        });
        assert!(
            !root.path().join("elsewhere").exists(),
            "applying in an existing note never creates the template's proposed path"
        );
    }
}
