//! Brainz: a folder picker over the brain's tree, used by the composer's
//! "File in folder…" chip to choose where a pasted screenshot lands.

use std::{
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};

use fuzzy::{StringMatch, StringMatchCandidate, match_strings};
use gpui::{App, Context, DismissEvent, Entity, EventEmitter, Focusable, Render, Task, Window};
use picker::{Picker, PickerDelegate};
use ui::{HighlightedLabel, ListItem, ListItemSpacing, prelude::*};
use workspace::ModalView;

const MAX_DEPTH: usize = 6;
const SKIPPED: &[&str] = &["node_modules", "target", "__pycache__", ".git"];

/// Every folder under `root`, as a relative path, sorted. Blocking file
/// I/O: run it on the background executor.
pub fn list_folders(root: &Path) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, depth: usize, out: &mut Vec<String>) {
        if depth > MAX_DEPTH {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut children: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        children.sort();
        for child in children {
            let Some(name) = child.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if name.starts_with('.') || SKIPPED.contains(&name) {
                continue;
            }
            if let Ok(relative) = child.strip_prefix(root) {
                out.push(relative.to_string_lossy().into_owned());
            }
            walk(root, &child, depth + 1, out);
        }
    }
    let mut folders = Vec::new();
    walk(root, root, 0, &mut folders);
    folders
}

pub struct FolderPicker {
    picker: Entity<Picker<FolderPickerDelegate>>,
}

impl FolderPicker {
    pub fn new(
        folders: Vec<String>,
        on_pick: Box<dyn Fn(String, &mut Window, &mut App) + 'static>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let delegate = FolderPickerDelegate {
            picker: cx.entity().downgrade(),
            folders: folders.into_iter().map(Arc::from).collect(),
            matches: Vec::new(),
            selected_index: 0,
            on_pick: Rc::new(on_pick),
        };
        let picker = cx.new(|cx| Picker::uniform_list(delegate, window, cx));
        Self { picker }
    }
}

impl ModalView for FolderPicker {}
impl EventEmitter<DismissEvent> for FolderPicker {}

impl Focusable for FolderPicker {
    fn focus_handle(&self, cx: &App) -> gpui::FocusHandle {
        self.picker.focus_handle(cx)
    }
}

impl Render for FolderPicker {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .key_context("BrainzFolderPicker")
            .w(rems(34.))
            .child(self.picker.clone())
    }
}

pub struct FolderPickerDelegate {
    picker: gpui::WeakEntity<FolderPicker>,
    folders: Vec<Arc<str>>,
    matches: Vec<StringMatch>,
    selected_index: usize,
    on_pick: Rc<dyn Fn(String, &mut Window, &mut App) + 'static>,
}

impl PickerDelegate for FolderPickerDelegate {
    type ListItem = ListItem;

    fn name() -> &'static str {
        "BrainzFolderPicker"
    }

    fn placeholder_text(&self, _window: &mut Window, _cx: &mut App) -> Arc<str> {
        "File in folder…".into()
    }

    fn match_count(&self) -> usize {
        self.matches.len()
    }

    fn selected_index(&self) -> usize {
        self.selected_index
    }

    fn set_selected_index(&mut self, ix: usize, _: &mut Window, _: &mut Context<Picker<Self>>) {
        self.selected_index = ix;
    }

    fn update_matches(
        &mut self,
        query: String,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Task<()> {
        let background = cx.background_executor().clone();
        let candidates = self
            .folders
            .iter()
            .enumerate()
            .map(|(id, folder)| StringMatchCandidate::new(id, folder))
            .collect::<Vec<_>>();
        cx.spawn_in(window, async move |this, cx| {
            let matches = if query.is_empty() {
                candidates
                    .into_iter()
                    .enumerate()
                    .map(|(index, candidate)| StringMatch {
                        candidate_id: index,
                        string: candidate.string,
                        positions: Vec::new(),
                        score: 0.0,
                    })
                    .collect()
            } else {
                match_strings(
                    &candidates,
                    &query,
                    false,
                    true,
                    200,
                    &Default::default(),
                    background,
                )
                .await
            };
            this.update_in(cx, |this, _, cx| {
                this.delegate.matches = matches;
                this.delegate.selected_index = this
                    .delegate
                    .selected_index
                    .min(this.delegate.matches.len().saturating_sub(1));
                cx.notify();
            })
            .ok();
        })
    }

    fn confirm(&mut self, _secondary: bool, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        if let Some(found) = self.matches.get(self.selected_index) {
            let folder = found.string.clone();
            (self.on_pick)(folder, window, cx);
        }
        self.picker
            .update(cx, |_, cx| cx.emit(DismissEvent))
            .ok();
    }

    fn dismissed(&mut self, _: &mut Window, cx: &mut Context<Picker<Self>>) {
        self.picker
            .update(cx, |_, cx| cx.emit(DismissEvent))
            .ok();
    }

    fn render_match(
        &self,
        ix: usize,
        selected: bool,
        _window: &mut Window,
        _cx: &mut Context<Picker<Self>>,
    ) -> Option<Self::ListItem> {
        let found = self.matches.get(ix)?;
        Some(
            ListItem::new(ix)
                .inset(true)
                .spacing(ListItemSpacing::Sparse)
                .toggle_state(selected)
                .start_slot(Icon::new(IconName::Folder).size(IconSize::Small).color(Color::Muted))
                .child(HighlightedLabel::new(found.string.clone(), found.positions.clone())),
        )
    }
}
