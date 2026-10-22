use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use brainz_templates::{InsertMode, Registry, TemplatesConfig};
use futures::StreamExt as _;
use fuzzy::StringMatchCandidate;
use gpui::{
    App, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, KeyBinding, Subscription,
    Task, WeakEntity, Window, actions,
};
use picker::{Picker, PickerDelegate};
use ui::{ListItem, ListItemSpacing, prelude::*};
use util::ResultExt;
use workspace::{ModalView, Workspace};

actions!(brainz_calendar, [OpenPalette, AskFromPalette]);

#[derive(Clone, PartialEq, Debug, serde::Deserialize, schemars::JsonSchema, gpui::Action)]
#[action(namespace = brainz_calendar)]
pub struct AskClaude {
    pub text: String,
}

pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new(
        "cmd-enter",
        AskFromPalette,
        Some("BrainzPalette"),
    )]);
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &OpenPalette, window, cx| {
            let root = workspace
                .root_paths(cx)
                .first()
                .map(|path| path.to_path_buf());
            let fs = workspace.project().read(cx).fs().clone();
            let owner = cx.weak_entity();
            workspace.toggle_modal(window, cx, move |window, cx| {
                Palette::new(owner, root, fs, window, cx)
            });
        });
    })
    .detach();
}

#[derive(Clone)]
enum Target {
    NewNote,
    Daily,
    SaveAsTemplate,
    Template(String),
    Navigate(usize),
}

#[derive(Clone)]
struct Row {
    name: String,
    description: String,
    search: String,
    group: &'static str,
    icon: IconName,
    target: Target,
}

fn rows(root: Option<&Path>, use_recents: bool) -> (Vec<Row>, PathBuf, Option<String>) {
    let mut rows = vec![
        Row {
            name: "New note".into(),
            description: "Choose one of your templates or start blank".into(),
            search: "new note blank create".into(),
            group: "Notes",
            icon: IconName::File,
            target: Target::NewNote,
        },
        Row {
            name: "Daily note".into(),
            description: "Open today's note, creating it if needed".into(),
            search: "daily today journal".into(),
            group: "Notes",
            icon: IconName::BrainzCalendar,
            target: Target::Daily,
        },
        Row {
            name: "Save note as template".into(),
            description: "Use the open note as a starting point for new ones".into(),
            search: "save template reuse copy".into(),
            group: "Notes",
            icon: IconName::FileTextOutlined,
            target: Target::SaveAsTemplate,
        },
    ];
    let mut directory = root.unwrap_or(Path::new("")).join(".brainz/templates");
    let mut error = None;
    if let Some(root) = root {
        let loaded = (|| {
            let mut registry = Registry::load_brain(root, None)?;
            registry.retain_saved();
            let config = TemplatesConfig::load(root)?;
            directory = root.join(
                config
                    .dir
                    .as_deref()
                    .unwrap_or(Path::new(".brainz/templates")),
            );
            let recents = if use_recents {
                match brainz_templates::recent_templates_path(paths::data_dir(), root)
                    .and_then(|path| brainz_templates::read_recents(&path))
                {
                    Ok(recents) => recents,
                    Err(failure) => {
                        error = Some(format!("Could not load recent templates: {failure}"));
                        Vec::new()
                    }
                }
            } else {
                Vec::new()
            };
            let mut templates = registry
                .templates
                .into_values()
                .filter(|template| template.metadata.modes.contains(&InsertMode::New))
                .collect::<Vec<_>>();
            templates.sort_by_cached_key(|template| {
                (
                    recents
                        .iter()
                        .position(|id| *id == template.metadata.id)
                        .unwrap_or(usize::MAX),
                    template.metadata.name.to_lowercase(),
                )
            });
            rows.extend(templates.into_iter().map(|template| {
                let metadata = template.metadata;
                let group = if recents.contains(&metadata.id) {
                    "Recent"
                } else {
                    "Templates"
                };
                Row {
                    search: format!(
                        "{} {} {} {}",
                        metadata.name,
                        metadata.id,
                        metadata.description,
                        metadata.tags.join(" ")
                    ),
                    name: metadata.name,
                    description: format!("Create note · {}", metadata.description),
                    group,
                    icon: IconName::File,
                    target: Target::Template(metadata.id),
                }
            }));
            if !registry.errors.is_empty() {
                error = Some(format!(
                    "{} template files could not be loaded",
                    registry.errors.len()
                ));
            }
            Ok::<_, anyhow::Error>(())
        })();
        if let Err(failure) = loaded {
            error = Some(format!("Could not load templates: {failure}"));
        }
    }
    rows.extend(
        super::nav::ITEMS
            .iter()
            .enumerate()
            .map(|(index, item)| Row {
                name: item.label.into(),
                description: format!("Open {}", item.label),
                search: item.label.into(),
                group: "Navigate",
                icon: item.icon,
                target: Target::Navigate(index),
            }),
    );
    (rows, directory, error)
}

struct Palette {
    picker: Entity<Picker<Delegate>>,
    _dismiss: Subscription,
    _watch: Task<()>,
}

impl Palette {
    fn new(
        workspace: WeakEntity<Workspace>,
        root: Option<PathBuf>,
        fs: Arc<dyn project::Fs>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let picker = cx.new(|cx| {
            Picker::list(
                Delegate {
                    workspace,
                    root: root.clone(),
                    directory: root
                        .as_ref()
                        .map(|root| root.join(".brainz/templates"))
                        .unwrap_or_default(),
                    use_recents: !fs.is_fake(),
                    rows: Vec::new(),
                    visible: Vec::new(),
                    selected: 0,
                    query: String::new(),
                    error: None,
                },
                window,
                cx,
            )
        });
        let dismiss = cx.subscribe(&picker, |_, _, _: &DismissEvent, cx| cx.emit(DismissEvent));
        let watched = picker.downgrade();
        let watch = cx.spawn_in(window, async move |_, cx| {
            let Some(root) = root else { return };
            let (mut events, _watcher) = fs.watch(&root, Duration::from_millis(250)).await;
            while let Some(events) = events.next().await {
                let Ok(directory) =
                    watched.read_with(cx, |picker, _| picker.delegate.directory.clone())
                else {
                    break;
                };
                if events.iter().any(|event| {
                    event.path.starts_with(&directory)
                        || directory.starts_with(&event.path)
                        || event.path == root.join("brainz.toml")
                }) {
                    if watched
                        .update_in(cx, |picker, window, cx| picker.refresh(window, cx))
                        .is_err()
                    {
                        break;
                    }
                }
            }
        });
        Self {
            picker,
            _dismiss: dismiss,
            _watch: watch,
        }
    }

    fn ask(&mut self, _: &AskFromPalette, window: &mut Window, cx: &mut Context<Self>) {
        self.picker
            .update(cx, |picker, cx| picker.delegate.ask(window, cx));
    }
}

impl ModalView for Palette {}
impl EventEmitter<DismissEvent> for Palette {}
impl Focusable for Palette {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.picker.focus_handle(cx)
    }
}
impl Render for Palette {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .key_context("BrainzPalette")
            .on_action(cx.listener(Self::ask))
            .w(px(600.))
            .rounded_lg()
            .overflow_hidden()
            .border_1()
            .border_color(cx.theme().colors().border)
            .bg(cx.theme().colors().elevated_surface_background)
            .child(
                h_flex()
                    .p_3()
                    .gap_2()
                    .child(Icon::new(IconName::MagnifyingGlass).color(Color::Muted))
                    .child(Label::new("Brainz").weight(gpui::FontWeight::SEMIBOLD)),
            )
            .child(self.picker.clone())
            .when_some(
                self.picker.read(cx).delegate.error.clone(),
                |view, error| {
                    view.child(
                        div().px_3().pb_2().child(
                            Label::new(error)
                                .size(LabelSize::Small)
                                .color(Color::Warning),
                        ),
                    )
                },
            )
            .child(
                div().px_3().py_2().child(
                    Label::new("Enter to open · ⌘Enter to ask Claude · Esc to close")
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                ),
            )
    }
}

struct Delegate {
    workspace: WeakEntity<Workspace>,
    root: Option<PathBuf>,
    directory: PathBuf,
    use_recents: bool,
    rows: Vec<Row>,
    visible: Vec<usize>,
    selected: usize,
    query: String,
    error: Option<String>,
}

impl Delegate {
    fn ask(&self, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        let text = self.query.trim().to_owned();
        if text.is_empty() {
            return;
        }
        let workspace = self.workspace.clone();
        window.defer(cx, move |window, cx| {
            workspace
                .update(cx, |workspace, cx| workspace.hide_modal(window, cx))
                .log_err();
            window.defer(cx, move |window, cx| {
                window.dispatch_action(Box::new(AskClaude { text }), cx);
            });
        });
    }
}

impl PickerDelegate for Delegate {
    type ListItem = AnyElement;
    fn name() -> &'static str {
        "brainz palette"
    }
    fn match_count(&self) -> usize {
        self.visible.len()
    }
    fn selected_index(&self) -> usize {
        self.selected
    }
    fn set_selected_index(&mut self, index: usize, _: &mut Window, _: &mut Context<Picker<Self>>) {
        self.selected = index;
    }
    fn placeholder_text(&self, _: &mut Window, _: &mut App) -> Arc<str> {
        "Find a template, open a tool, or ask Claude…".into()
    }
    fn update_matches(
        &mut self,
        query: String,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Task<()> {
        self.query = query.clone();
        let root = self.root.clone();
        let use_recents = self.use_recents;
        cx.spawn_in(window, async move |picker, cx| {
            let (rows, directory, error) = cx
                .background_spawn(async move { rows(root.as_deref(), use_recents) })
                .await;
            let visible = if query.is_empty() {
                (0..rows.len()).collect()
            } else {
                let candidates = rows
                    .iter()
                    .enumerate()
                    .map(|(index, row)| StringMatchCandidate::new(index, &row.search))
                    .collect::<Vec<_>>();
                fuzzy::match_strings(
                    &candidates,
                    &query,
                    false,
                    true,
                    100,
                    &Default::default(),
                    cx.background_executor().clone(),
                )
                .await
                .into_iter()
                .map(|matched| matched.candidate_id)
                .collect()
            };
            picker
                .update(cx, |picker, cx| {
                    picker.delegate.rows = rows;
                    picker.delegate.visible = visible;
                    picker.delegate.directory = directory;
                    picker.delegate.error = error;
                    picker.delegate.selected = 0;
                    cx.notify();
                })
                .log_err();
        })
    }
    fn confirm(&mut self, secondary: bool, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        if secondary {
            self.ask(window, cx);
            return;
        }
        let Some(target) = self
            .visible
            .get(self.selected)
            .and_then(|index| self.rows.get(*index))
            .map(|row| row.target.clone())
        else {
            return;
        };
        let workspace = self.workspace.clone();
        window.defer(cx, move |window, cx| {
            workspace
                .update(cx, |workspace, cx| {
                    workspace.hide_modal(window, cx);
                    match target {
                        Target::Template(id) => super::new_note::open_with_template(
                            workspace,
                            false,
                            Some(id),
                            window,
                            cx,
                        ),
                        Target::NewNote => {
                            super::new_note::open_with_template(workspace, false, None, window, cx)
                        }
                        Target::Daily => {
                            super::new_note::open_with_template(workspace, true, None, window, cx)
                        }
                        Target::SaveAsTemplate => {
                            super::new_note::save_as_template(workspace, window, cx)
                        }
                        Target::Navigate(index) => {
                            if let Some(item) = super::nav::ITEMS.get(index) {
                                let action = (item.action)();
                                window.defer(cx, move |window, cx| {
                                    window.dispatch_action(action, cx)
                                });
                            }
                        }
                    }
                })
                .log_err();
        });
    }
    fn dismissed(&mut self, _: &mut Window, cx: &mut Context<Picker<Self>>) {
        cx.emit(DismissEvent);
    }
    fn render_match(
        &self,
        index: usize,
        selected: bool,
        _: &mut Window,
        _: &mut Context<Picker<Self>>,
    ) -> Option<AnyElement> {
        let row = self.rows.get(*self.visible.get(index)?)?;
        let header = index
            .checked_sub(1)
            .and_then(|index| self.visible.get(index))
            .and_then(|index| self.rows.get(*index))
            .is_none_or(|previous| previous.group != row.group);
        Some(
            v_flex()
                .when(header, |view| {
                    view.child(
                        div().px_3().pt_2().pb_1().child(
                            Label::new(row.group)
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                        ),
                    )
                })
                .child(
                    ListItem::new(index)
                        .spacing(ListItemSpacing::Sparse)
                        .toggle_state(selected)
                        .child(
                            h_flex()
                                .gap_2()
                                .child(Icon::new(row.icon).color(Color::Muted))
                                .child(
                                    v_flex()
                                        .min_w_0()
                                        .child(Label::new(row.name.clone()))
                                        .child(
                                            Label::new(row.description.clone())
                                                .size(LabelSize::XSmall)
                                                .color(Color::Muted)
                                                .truncate(),
                                        ),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };

    #[gpui::test]
    async fn palette_search_cancel_navigation_and_explicit_ask_preserve_the_note(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            cx.set_global(db::AppDatabase::test_new());
            workspace::AppState::test(cx);
            editor::init(cx);
            init(cx);
            let bindings = settings::KeymapFile::load_asset_allow_partial_failure(
                "keymaps/default-macos.json",
                cx,
            )
            .expect("default keymap");
            assert!(
                bindings
                    .iter()
                    .any(|binding| binding.action().as_any().is::<OpenPalette>()),
                "the default keymap includes the Brainz palette action"
            );
            cx.bind_keys(bindings);
        });
        let root = tempfile::tempdir().expect("temporary brain");
        let directory = root.path().join(".brainz/templates");
        std::fs::create_dir_all(&directory).expect("template directory");
        let path = directory.join("launch-plan.md");
        std::fs::write(
            &path,
            "---\nname: Launch plan\ntags: [spaceship]\n---\n# Launch\n",
        )
        .expect("fixture template");
        let fs = project::FakeFs::new(cx.executor());
        fs.insert_tree(root.path(), json!({"note.md":"Keep my draft"}))
            .await;
        let project = project::Project::test(fs.clone(), [root.path()], cx).await;
        let buffer = project
            .update(cx, |project, cx| {
                project.open_local_buffer(root.path().join("note.md"), cx)
            })
            .await
            .expect("buffer");
        let prompts = Rc::new(RefCell::new(Vec::new()));
        let navigations = Rc::new(Cell::new(0));
        let (host, cx) = cx.add_window_view(|window, cx| {
            workspace::MultiWorkspace::test_new(project.clone(), window, cx)
        });
        let workspace = host.read_with(cx, |host, _| host.workspace().clone());
        workspace.update(cx, |workspace, _| {
            workspace.register_action({
                let prompts = prompts.clone();
                move |_, action: &AskClaude, _, _| prompts.borrow_mut().push(action.text.clone())
            });
            workspace.register_action({
                let navigations = navigations.clone();
                move |_, _: &super::super::todo::OpenTodo, _, _| {
                    navigations.set(navigations.get() + 1)
                }
            });
        });
        let editor = workspace.update_in(cx, |workspace, window, cx| {
            let editor = cx.new(|cx| editor::Editor::for_buffer(buffer, Some(project), window, cx));
            workspace.add_item_to_active_pane(Box::new(editor.clone()), None, true, window, cx);
            window.focus(&editor.focus_handle(cx), cx);
            editor
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert!(
                editor.focus_handle(cx).contains_focused(window, cx),
                "the note has keyboard focus before opening the palette"
            )
        });
        cx.update(|window, cx| {
            assert!(
                window
                    .available_actions(cx)
                    .iter()
                    .any(|action| action.as_any().is::<OpenPalette>()),
                "palette action registered; contexts: {:?}",
                window.context_stack()
            );
            assert!(
                !window.bindings_for_action(&OpenPalette).is_empty(),
                "palette binding active; contexts: {:?}",
                window.context_stack()
            );
        });
        cx.simulate_keystrokes("cmd-k");
        cx.run_until_parked();
        let palette = workspace.update_in(cx, |workspace, _, cx| {
            workspace
                .active_modal::<Palette>(cx)
                .expect("palette opened immediately")
        });
        cx.simulate_input("spaceship");
        cx.run_until_parked();
        palette.read_with(cx, |palette, cx| {
            let delegate = &palette.picker.read(cx).delegate;
            assert_eq!(delegate.visible.len(), 1);
            assert!(matches!(&delegate.rows[delegate.visible[0]].target, Target::Template(id) if id == "launch-plan"));
        });
        std::fs::write(
            &path,
            "---\nname: Updated launch plan\ntags: [spaceship]\n---\n# Updated\n",
        )
        .expect("edit fixture template");
        fs.emit_fs_event(&path, Some(project::PathEventKind::Changed));
        cx.run_until_parked();
        palette.read_with(cx, |palette, cx| {
            let delegate = &palette.picker.read(cx).delegate;
            assert_eq!(delegate.query, "spaceship");
            assert_eq!(
                delegate.rows[delegate.visible[0]].name,
                "Updated launch plan"
            );
        });
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert!(!workspace.read(cx).has_active_modal(window, cx));
            assert!(editor.focus_handle(cx).contains_focused(window, cx));
            assert_eq!(editor.read(cx).text(cx), "Keep my draft");
        });
        cx.simulate_keystrokes("cmd-k");
        cx.run_until_parked();
        cx.simulate_input("To-Do");
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(navigations.get(), 1);
        assert!(prompts.borrow().is_empty());
        cx.simulate_keystrokes("cmd-k");
        cx.run_until_parked();
        cx.simulate_input("Explain my next steps");
        cx.run_until_parked();
        assert!(prompts.borrow().is_empty(), "typing never sends a request");
        cx.simulate_keystrokes("cmd-enter");
        cx.run_until_parked();
        assert_eq!(&*prompts.borrow(), &["Explain my next steps".to_owned()]);
        editor.read_with(cx, |editor, cx| {
            assert_eq!(editor.text(cx), "Keep my draft")
        });
        cx.simulate_keystrokes("cmd-k");
        cx.run_until_parked();
        cx.simulate_input("spaceship");
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert!(workspace.read(cx).has_active_modal(window, cx));
            assert!(workspace.read(cx).active_modal::<Palette>(cx).is_none());
        });
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(!root.path().join("launch.md").exists());
        editor.read_with(cx, |editor, cx| {
            assert_eq!(editor.text(cx), "Keep my draft")
        });
    }

    #[test]
    fn broken_templates_do_not_hide_navigation_or_valid_templates() {
        let root = tempfile::tempdir().expect("temporary brain");
        std::fs::create_dir_all(root.path().join(".brainz/templates")).expect("templates");
        std::fs::write(
            root.path().join(".brainz/templates/broken.md"),
            "---\nname: [invalid\n---",
        )
        .expect("invalid fixture");
        std::fs::write(
            root.path().join(".brainz/templates/standup.md"),
            "---\nname: Standup\n---\nYesterday, today, blockers",
        )
        .expect("saved fixture");
        let (rows, _, error) = rows(Some(root.path()), false);
        assert!(error.is_some());
        assert!(
            rows.iter()
                .any(|row| matches!(&row.target, Target::Template(id) if id == "standup"))
        );
        // Bundled templates stay out of the list.
        assert!(
            !rows
                .iter()
                .any(|row| matches!(&row.target, Target::Template(id) if id == "decision"))
        );
        assert!(rows.iter().any(|row| row.name == "Connectors"));
    }
}
