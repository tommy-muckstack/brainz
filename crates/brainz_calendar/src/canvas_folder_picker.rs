use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
};

use file_icons::FileIcons;
use fuzzy::{StringMatch, StringMatchCandidate};
use gpui::{
    App, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, Subscription, Task, Window,
};
use picker::{Picker, PickerDelegate};
use project::{Project, ProjectEntryId, WorktreeId};
use settings::{FolderIndicator, Settings};
use ui::{ListItem, ListItemSpacing, prelude::*};
use util::{ResultExt, paths::compare_paths};
use workspace::ModalView;

pub(super) struct FolderSettings {
    indicator: FolderIndicator,
    indent: f32,
    hide_hidden: bool,
    hide_ignored: bool,
}

impl Settings for FolderSettings {
    fn from_settings(content: &settings::SettingsContent) -> Self {
        let panel = content.project_panel.as_ref();
        Self {
            indicator: panel
                .and_then(|panel| panel.folder_indicator)
                .unwrap_or_default(),
            indent: panel
                .and_then(|panel| panel.indent_size.as_ref())
                .map(|size| **size)
                .unwrap_or(12.),
            hide_hidden: panel.and_then(|panel| panel.hide_hidden).unwrap_or(true),
            hide_ignored: panel
                .and_then(|panel| panel.hide_gitignore)
                .unwrap_or(false),
        }
    }
}

#[derive(Clone)]
struct Folder {
    path: PathBuf,
    label: String,
    search: String,
    depth: usize,
    worktree: WorktreeId,
    entry: ProjectEntryId,
}

fn folders(project: &Project, cx: &App) -> Vec<Folder> {
    let settings = FolderSettings::get_global(cx);
    let mut folders = Vec::new();
    for worktree in project.visible_worktrees(cx) {
        let worktree = worktree.read(cx);
        if !worktree.is_local() {
            continue;
        }
        let root = worktree.root_name_str();
        let mut tree = worktree
            .directories(!settings.hide_ignored, 0)
            .filter(|entry| {
                !settings.hide_hidden
                    || !entry
                        .path
                        .as_std_path()
                        .components()
                        .any(|part| part.as_os_str().to_string_lossy().starts_with('.'))
            })
            .map(|entry| {
                let relative = entry.path.as_std_path();
                Folder {
                    path: worktree.absolutize(&entry.path),
                    label: relative
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| root.to_owned()),
                    search: if relative.as_os_str().is_empty() {
                        root.to_owned()
                    } else {
                        format!("{root}/{}", relative.display())
                    },
                    depth: relative.components().count(),
                    worktree: worktree.id(),
                    entry: entry.id,
                }
            })
            .collect::<Vec<_>>();
        tree.sort_by(|a, b| compare_paths((&a.path, false), (&b.path, false)));
        folders.extend(tree);
    }
    folders
}

fn visible_indices(
    folders: &[Folder],
    expanded: &HashSet<PathBuf>,
    matches: Option<&[StringMatch]>,
) -> Vec<usize> {
    let shown = matches.map(|matches| {
        matches
            .iter()
            .filter_map(|matched| folders.get(matched.candidate_id))
            .flat_map(|folder| {
                folder
                    .path
                    .ancestors()
                    .take(folder.depth + 1)
                    .map(Path::to_path_buf)
            })
            .collect::<HashSet<_>>()
    });
    folders
        .iter()
        .enumerate()
        .filter_map(|(index, folder)| {
            let visible = match &shown {
                Some(shown) => shown.contains(&folder.path),
                None => folder
                    .path
                    .ancestors()
                    .skip(1)
                    .take(folder.depth)
                    .all(|parent| expanded.contains(parent)),
            };
            visible.then_some(index)
        })
        .collect()
}

pub(super) struct CanvasFolderPicker {
    picker: Entity<Picker<FolderDelegate>>,
    title: SharedString,
    icon: IconName,
    _subscriptions: Vec<Subscription>,
}

impl CanvasFolderPicker {
    pub(super) fn new(
        project: Entity<Project>,
        current: PathBuf,
        filename: String,
        sender: smol::channel::Sender<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let all_folders = folders(project.read(cx), cx);
        let expanded = all_folders
            .iter()
            .filter(|folder| folder.depth == 0 || current.starts_with(&folder.path))
            .map(|folder| folder.path.clone())
            .collect();
        let delegate = FolderDelegate {
            project: project.clone(),
            folders: all_folders,
            expanded,
            visible: Vec::new(),
            selected: 0,
            selected_path: Some(current),
            query: String::new(),
            filename,
            sender: Some(sender),
        };
        let picker = cx.new(|cx| {
            Picker::uniform_list(delegate, window, cx)
                .embedded()
                .max_height(rems(22.))
        });
        let dismiss = cx.subscribe(&picker, |_, _, _: &DismissEvent, cx| cx.emit(DismissEvent));
        let updates = cx.subscribe_in(
            &project,
            window,
            |this, _, event: &project::Event, window, cx| {
                if matches!(
                    event,
                    project::Event::WorktreeUpdatedEntries(..)
                        | project::Event::WorktreeAdded(..)
                        | project::Event::WorktreeRemoved(..)
                ) {
                    this.picker
                        .update(cx, |picker, cx| picker.refresh(window, cx));
                }
            },
        );
        Self {
            picker,
            title: "Move canvas".into(),
            icon: IconName::BrainzWhiteboard,
            _subscriptions: vec![dismiss, updates],
        }
    }

    pub(super) fn with_title(mut self, title: impl Into<SharedString>, icon: IconName) -> Self {
        self.title = title.into();
        self.icon = icon;
        self
    }
}

impl ModalView for CanvasFolderPicker {}
impl EventEmitter<DismissEvent> for CanvasFolderPicker {}
impl Focusable for CanvasFolderPicker {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.picker.focus_handle(cx)
    }
}
impl Render for CanvasFolderPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .w(px(540.))
            .rounded_lg()
            .overflow_hidden()
            .border_1()
            .border_color(cx.theme().colors().border)
            .bg(cx.theme().colors().elevated_surface_background)
            .child(
                h_flex()
                    .p_3()
                    .gap_2()
                    .child(Icon::new(self.icon).color(Color::Muted))
                    .child(Label::new(self.title.clone()).weight(gpui::FontWeight::SEMIBOLD)),
            )
            .child(self.picker.clone())
    }
}

struct FolderDelegate {
    project: Entity<Project>,
    folders: Vec<Folder>,
    expanded: HashSet<PathBuf>,
    visible: Vec<usize>,
    selected: usize,
    selected_path: Option<PathBuf>,
    query: String,
    filename: String,
    sender: Option<smol::channel::Sender<PathBuf>>,
}

impl FolderDelegate {
    fn selected_folder(&self) -> Option<&Folder> {
        self.visible
            .get(self.selected)
            .and_then(|index| self.folders.get(*index))
    }

    fn toggle_folder(&mut self, folder: &Folder, cx: &mut Context<Picker<Self>>) {
        if !self.expanded.remove(&folder.path) {
            self.expanded.insert(folder.path.clone());
            if let Some(task) = self.project.update(cx, |project, cx| {
                project.expand_entry(folder.worktree, folder.entry, cx)
            }) {
                task.detach_and_log_err(cx);
            }
        }
    }
}

impl PickerDelegate for FolderDelegate {
    type ListItem = ListItem;
    fn name() -> &'static str {
        "brainz canvas folders"
    }
    fn match_count(&self) -> usize {
        self.visible.len()
    }
    fn selected_index(&self) -> usize {
        self.selected
    }
    fn select_on_hover(&self) -> bool {
        false
    }
    fn set_selected_index(&mut self, index: usize, _: &mut Window, cx: &mut Context<Picker<Self>>) {
        self.selected = index;
        self.selected_path = self.selected_folder().map(|folder| folder.path.clone());
        cx.notify();
    }
    fn placeholder_text(&self, _: &mut Window, _: &mut App) -> Arc<str> {
        "Find a folder…".into()
    }
    fn no_matches_text(&self, _: &mut Window, _: &mut App) -> Option<SharedString> {
        Some("No matching folders".into())
    }
    fn update_matches(
        &mut self,
        query: String,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Task<()> {
        let query_changed = query != self.query;
        self.query = query.clone();
        let folders = folders(self.project.read(cx), cx);
        let candidates = folders
            .iter()
            .enumerate()
            .map(|(index, folder)| StringMatchCandidate::new(index, &folder.search))
            .collect::<Vec<_>>();
        cx.spawn_in(window, async move |picker, cx| {
            let matches = if query.is_empty() {
                None
            } else {
                Some(
                    fuzzy::match_strings(
                        &candidates,
                        &query,
                        false,
                        true,
                        100,
                        &Default::default(),
                        cx.background_executor().clone(),
                    )
                    .await,
                )
            };
            picker
                .update(cx, |picker, cx| {
                    let delegate = &mut picker.delegate;
                    delegate.folders = folders;
                    delegate.visible =
                        visible_indices(&delegate.folders, &delegate.expanded, matches.as_deref());
                    let preferred = if query_changed {
                        matches
                            .as_ref()
                            .and_then(|matches| matches.first())
                            .and_then(|matched| delegate.folders.get(matched.candidate_id))
                            .map(|folder| &folder.path)
                    } else {
                        delegate.selected_path.as_ref()
                    };
                    delegate.selected = preferred
                        .and_then(|path| {
                            delegate.visible.iter().position(|index| {
                                delegate
                                    .folders
                                    .get(*index)
                                    .is_some_and(|folder| &folder.path == path)
                            })
                        })
                        .unwrap_or(0);
                    delegate.selected_path =
                        delegate.selected_folder().map(|folder| folder.path.clone());
                    cx.notify();
                })
                .log_err();
        })
    }
    fn confirm(&mut self, _: bool, _: &mut Window, cx: &mut Context<Picker<Self>>) {
        let Some(folder) = self.selected_folder() else {
            return;
        };
        let path = folder.path.clone();
        if let Some(sender) = self.sender.take() {
            sender.try_send(path).log_err();
        }
        cx.emit(DismissEvent);
    }
    fn dismissed(&mut self, _: &mut Window, cx: &mut Context<Picker<Self>>) {
        self.sender.take();
        cx.emit(DismissEvent);
    }
    fn render_match(
        &self,
        index: usize,
        selected: bool,
        _: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Option<ListItem> {
        let folder = self.folders.get(*self.visible.get(index)?)?;
        let settings = FolderSettings::get_global(cx);
        let expanded = if self.query.is_empty() {
            self.expanded.contains(&folder.path)
        } else {
            self.visible
                .get(index + 1)
                .and_then(|index| self.folders.get(*index))
                .is_some_and(|next| next.path.starts_with(&folder.path))
        };
        let indicators =
            FileIcons::get_folder_indicators(settings.indicator, expanded, &folder.path, cx);
        Some(
            ListItem::new(("canvas-folder", index))
                .spacing(ListItemSpacing::Dense)
                .indent_level(folder.depth)
                .indent_step_size(px(settings.indent))
                .toggle_state(selected)
                .inset(true)
                .start_slot(
                    h_flex()
                        .gap_0p5()
                        .when_some(indicators.chevron, |row, icon| {
                            row.child(Icon::from_path(icon).color(Color::Muted))
                        })
                        .when_some(indicators.icon, |row, icon| {
                            row.child(Icon::from_path(icon).color(Color::Muted))
                        }),
                )
                .child(Label::new(folder.label.clone()).truncate())
                .on_click(cx.listener(move |picker, _, window, cx| {
                    cx.stop_propagation();
                    picker.set_selected_index(index, None, true, window, cx);
                    if picker.delegate.query.is_empty() {
                        if let Some(folder) = picker.delegate.selected_folder().cloned() {
                            picker.delegate.toggle_folder(&folder, cx);
                            picker.refresh(window, cx);
                        }
                    }
                    picker.focus(window, cx);
                })),
        )
    }
    fn render_footer(&self, _: &mut Window, cx: &mut Context<Picker<Self>>) -> Option<AnyElement> {
        let destination = self
            .selected_folder()
            .map(|folder| format!("{}/{}", folder.search, self.filename));
        Some(
            v_flex()
                .p_3()
                .gap_2()
                .border_t_1()
                .border_color(cx.theme().colors().border)
                .child(
                    Label::new(
                        destination.unwrap_or_else(|| "Choose a folder in your brain".into()),
                    )
                    .size(LabelSize::Small)
                    .color(Color::Muted)
                    .truncate(),
                )
                .child(
                    h_flex()
                        .justify_between()
                        .gap_3()
                        .child(
                            Label::new("↑↓ Choose · Enter to move")
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .child(Button::new("cancel-canvas-move", "Cancel").on_click(
                                    cx.listener(|picker, _, window, cx| {
                                        picker.delegate.dismissed(window, cx)
                                    }),
                                ))
                                .child(
                                    Button::new("confirm-canvas-move", "Move here")
                                        .style(ButtonStyle::Filled)
                                        .disabled(self.selected_folder().is_none())
                                        .on_click(cx.listener(|picker, _, window, cx| {
                                            picker.focus(window, cx);
                                            window.dispatch_action(Box::new(menu::Confirm), cx);
                                        })),
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

    #[gpui::test]
    async fn typing_and_confirming_selects_a_folder_and_cancel_returns_nothing(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            let settings = settings::SettingsStore::test(cx);
            cx.set_global(settings);
            cx.set_global(db::AppDatabase::test_new());
            theme_settings::init(theme::LoadThemes::JustBase, cx);
            editor::init(cx);
            FolderSettings::register(cx);
        });
        let fs = project::FakeFs::new(cx.executor());
        fs.insert_tree("/brain", json!({"career": {}, "finances": {"Plans": {}}}))
            .await;
        let project = Project::test(fs, [Path::new("/brain")], cx).await;
        for cancel in [false, true] {
            let (sender, receiver) = smol::channel::bounded(1);
            let (modal, cx) = cx.add_window_view(|window, cx| {
                CanvasFolderPicker::new(
                    project.clone(),
                    PathBuf::from("/brain"),
                    "Ideas.brainzboard".into(),
                    sender,
                    window,
                    cx,
                )
            });
            let picker = modal.read_with(cx, |modal, _| modal.picker.clone());
            picker.update_in(cx, |picker, window, cx| picker.focus(window, cx));
            cx.run_until_parked();
            cx.simulate_input("finpl");
            cx.run_until_parked();
            picker.read_with(cx, |picker, _| {
                assert_eq!(
                    picker
                        .delegate
                        .selected_folder()
                        .expect("search result")
                        .path,
                    Path::new("/brain/finances/Plans")
                );
            });
            assert!(receiver.try_recv().is_err());
            if cancel {
                cx.dispatch_action(menu::Cancel);
                cx.run_until_parked();
                assert!(receiver.try_recv().is_err());
                assert!(receiver.is_closed());
            } else {
                cx.dispatch_action(menu::Confirm);
                cx.run_until_parked();
                assert_eq!(
                    receiver.try_recv().expect("confirmed folder"),
                    Path::new("/brain/finances/Plans")
                );
            }
        }
    }

    #[gpui::test]
    async fn typeahead_finds_nested_folders_and_preserves_their_ancestors(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            let settings = settings::SettingsStore::test(cx);
            cx.set_global(settings);
            FolderSettings::register(cx);
            FolderSettings::override_global(
                FolderSettings {
                    indicator: FolderIndicator::Icon,
                    indent: 12.,
                    hide_hidden: true,
                    hide_ignored: false,
                },
                cx,
            );
        });
        let fs = project::FakeFs::new(cx.executor());
        fs.insert_tree(
            "/brain",
            json!({
                ".brainz": {"private": {}},
                "career": {"Plans": {"note.md": "hello"}},
                "finances": {"Plans": {"budget.md": "budget"}},
                "README.md": "brain"
            }),
        )
        .await;
        let project = Project::test(fs, [Path::new("/brain")], cx).await;
        let folders = cx.read(|cx| folders(project.read(cx), cx));
        let expanded = HashSet::from([PathBuf::from("/brain")]);
        let labels = |indices: Vec<usize>| {
            indices
                .into_iter()
                .map(|index| folders[index].search.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            labels(visible_indices(&folders, &expanded, None)),
            ["brain", "brain/career", "brain/finances"]
        );
        let candidates = folders
            .iter()
            .enumerate()
            .map(|(index, folder)| StringMatchCandidate::new(index, &folder.search))
            .collect::<Vec<_>>();
        let matches = fuzzy::match_strings(
            &candidates,
            "FINpl",
            false,
            true,
            100,
            &Default::default(),
            cx.executor(),
        )
        .await;
        assert_eq!(
            labels(visible_indices(&folders, &expanded, Some(&matches))),
            ["brain", "brain/finances", "brain/finances/Plans"]
        );
        let matches = fuzzy::match_strings(
            &candidates,
            "plans",
            false,
            true,
            100,
            &Default::default(),
            cx.executor(),
        )
        .await;
        assert_eq!(
            labels(visible_indices(&folders, &expanded, Some(&matches))),
            [
                "brain",
                "brain/career",
                "brain/career/Plans",
                "brain/finances",
                "brain/finances/Plans"
            ]
        );
        assert!(visible_indices(&folders, &expanded, Some(&[])).is_empty());
        assert!(
            !folders
                .iter()
                .any(|folder| folder.search.contains(".brainz") || folder.search.ends_with(".md"))
        );
        assert_eq!(
            labels(visible_indices(&folders, &HashSet::new(), None)),
            ["brain"]
        );
    }
}
