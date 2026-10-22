//! Brainz: a note's past versions, read from the brain's git history. Every
//! sync commits the brain, so each version is a sync that changed the note;
//! edits since the last sync show as "Now". Restoring writes the old text into
//! the note's buffer as one edit, so Undo in the note takes it back.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use chrono::{DateTime, FixedOffset, Local};
use gpui::{App, Entity, EventEmitter, FocusHandle, Focusable, Task, WeakEntity, Window, actions};
use markdown::{Markdown, MarkdownElement};
use ui::prelude::*;
use util::ResultExt as _;
use workspace::{Item, Workspace};

actions!(
    brainz_note,
    [
        /// Show the open note's past versions.
        ShowHistory,
    ]
);

const MAX_VERSIONS: usize = 200;

#[derive(Clone, Debug, PartialEq)]
struct Version {
    commit: String,
    date: DateTime<FixedOffset>,
    message: String,
    /// The note's path in that commit, which differs after a rename.
    path: String,
}

/// `git log --follow` output: records start with \x1e, fields split by \x1f,
/// and the note's path at that commit follows on its own line.
fn parse_log(output: &str) -> Vec<Version> {
    output
        .split('\u{1e}')
        .filter_map(|record| {
            let mut lines = record.lines().filter(|line| !line.trim().is_empty());
            let mut fields = lines.next()?.split('\u{1f}');
            let commit = fields.next()?.trim().to_owned();
            let date = DateTime::parse_from_rfc3339(fields.next()?.trim()).ok()?;
            let message = fields.next().unwrap_or("").trim().to_owned();
            let path = lines.next()?.trim().to_owned();
            Some(Version {
                commit,
                date,
                message,
                path,
            })
        })
        .collect()
}

#[allow(clippy::disallowed_methods, reason = "runs on the background executor")]
fn git(brain: &Path, args: &[&str]) -> Result<String> {
    let output = crate::github_sync::command(brain, "git")
        .args(args)
        .output()
        .context("running git")?;
    anyhow::ensure!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn versions(brain: &Path, relative: &str) -> Result<Vec<Version>> {
    let log = git(
        brain,
        &[
            "log",
            "--follow",
            "--name-only",
            &format!("-n{MAX_VERSIONS}"),
            "--format=%x1e%H%x1f%aI%x1f%s",
            "--",
            relative,
        ],
    )?;
    Ok(parse_log(&log))
}

fn text_at(brain: &Path, version: &Version) -> Result<String> {
    git(
        brain,
        &["show", &format!("{}:{}", version.commit, version.path)],
    )
}

fn when(date: &DateTime<FixedOffset>) -> String {
    date.with_timezone(&Local)
        .format("%b %-d, %Y · %-I:%M %p")
        .to_string()
}

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &ShowHistory, window, cx| {
            let note = workspace.active_item(cx).and_then(|item| {
                let path = item.project_path(cx)?;
                let project = workspace.project().read(cx);
                let worktree = project.worktree_for_id(path.worktree_id, cx)?.read(cx);
                Some(worktree.absolutize(&path.path))
            });
            match note {
                Some(note) => open(workspace, note, window, cx),
                None => workspace.show_toast(
                    workspace::Toast::new(
                        workspace::notifications::NotificationId::unique::<ShowHistory>(),
                        "Open a note to see its history.",
                    )
                    .autohide(),
                    cx,
                ),
            }
        });
    })
    .detach();
}

/// Shows the history of `note`, reusing its tab when one is open.
pub fn open(
    workspace: &mut Workspace,
    note: PathBuf,
    window: &mut Window,
    cx: &mut gpui::Context<Workspace>,
) {
    let existing = workspace
        .items_of_type::<NoteHistoryView>(cx)
        .find(|view| view.read(cx).note == note);
    if let Some(existing) = existing {
        workspace.activate_item(&existing, true, true, window, cx);
        existing.update(cx, |view, cx| view.load(cx));
        return;
    }
    let Some(brain) = workspace
        .project()
        .read(cx)
        .find_worktree(&note, cx)
        .map(|(worktree, _)| worktree.read(cx).abs_path().to_path_buf())
    else {
        return;
    };
    let weak = cx.entity().downgrade();
    let view = cx.new(|cx| NoteHistoryView::new(weak, brain, note, cx));
    workspace.add_item_to_active_pane(Box::new(view), None, true, window, cx);
}

pub struct NoteHistoryView {
    workspace: WeakEntity<Workspace>,
    brain: PathBuf,
    note: PathBuf,
    focus_handle: FocusHandle,
    versions: Vec<Version>,
    /// The note on disk now, shown first when it differs from the last version.
    current: Option<String>,
    /// None selects "Now".
    selected: Option<usize>,
    preview: Entity<Markdown>,
    preview_text: Option<String>,
    loading: bool,
    error: Option<String>,
    _task: Option<Task<()>>,
}

impl NoteHistoryView {
    fn new(
        workspace: WeakEntity<Workspace>,
        brain: PathBuf,
        note: PathBuf,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        let mut view = Self {
            workspace,
            brain,
            note,
            focus_handle: cx.focus_handle(),
            versions: Vec::new(),
            current: None,
            selected: None,
            preview: cx.new(|cx| Markdown::new("".into(), None, None, cx)),
            preview_text: None,
            loading: true,
            error: None,
            _task: None,
        };
        view.load(cx);
        view
    }

    fn relative(&self) -> String {
        self.note
            .strip_prefix(&self.brain)
            .unwrap_or(&self.note)
            .to_string_lossy()
            .into_owned()
    }

    fn load(&mut self, cx: &mut gpui::Context<Self>) {
        self.loading = true;
        let brain = self.brain.clone();
        let note = self.note.clone();
        let relative = self.relative();
        self._task = Some(cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_spawn(async move {
                    let versions = versions(&brain, &relative)?;
                    let current = std::fs::read_to_string(&note).ok();
                    let latest = match versions.first() {
                        Some(version) => text_at(&brain, version).ok(),
                        None => None,
                    };
                    anyhow::Ok((
                        versions,
                        current.filter(|current| Some(current) != latest.as_ref()),
                    ))
                })
                .await;
            this.update(cx, |this, cx| {
                this.loading = false;
                match loaded {
                    Ok((versions, current)) => {
                        this.versions = versions;
                        this.current = current;
                        this.error = None;
                        let first = if this.current.is_some() || this.versions.is_empty() {
                            None
                        } else {
                            Some(0)
                        };
                        this.select(first, cx);
                    }
                    Err(error) => {
                        this.error = Some(format!("Couldn't read this note's history: {error:#}"))
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn select(&mut self, selected: Option<usize>, cx: &mut gpui::Context<Self>) {
        self.selected = selected;
        let Some(index) = selected else {
            let text = self.current.clone().unwrap_or_default();
            self.show(text, cx);
            return;
        };
        let Some(version) = self.versions.get(index).cloned() else {
            return;
        };
        let brain = self.brain.clone();
        self._task = Some(cx.spawn(async move |this, cx| {
            let text = cx
                .background_spawn(async move { text_at(&brain, &version) })
                .await;
            this.update(cx, |this, cx| match text {
                Ok(text) => this.show(text, cx),
                Err(error) => {
                    this.error = Some(format!("Couldn't read that version: {error:#}"));
                    cx.notify();
                }
            })
            .ok();
        }));
        cx.notify();
    }

    fn show(&mut self, text: String, cx: &mut gpui::Context<Self>) {
        self.preview
            .update(cx, |preview, cx| preview.replace(text.clone(), cx));
        self.preview_text = Some(text);
        cx.notify();
    }

    /// Writes the shown version into the note as one undoable edit and saves.
    fn restore(&mut self, cx: &mut gpui::Context<Self>) {
        let (Some(text), Some(index)) = (self.preview_text.clone(), self.selected) else {
            return;
        };
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        let label = self
            .versions
            .get(index)
            .map(|version| when(&version.date))
            .unwrap_or_default();
        let project = workspace.read(cx).project().clone();
        let open = project.update(cx, |project, cx| project.open_local_buffer(&self.note, cx));
        let weak_workspace = self.workspace.clone();
        cx.spawn(async move |this, cx| {
            let result = async {
                let buffer = open.await?;
                buffer.update(cx, |buffer, cx| {
                    let end = buffer.len();
                    buffer.edit([(0..end, text)], None, cx);
                });
                project
                    .update(cx, |project, cx| project.save_buffer(buffer, cx))
                    .await
            }
            .await;
            let message = match &result {
                Ok(()) => format!("Restored the {label} version. Undo in the note takes it back."),
                Err(error) => format!("Couldn't restore that version: {error:#}"),
            };
            weak_workspace
                .update(cx, |workspace, cx| {
                    workspace.show_toast(
                        workspace::Toast::new(
                            workspace::notifications::NotificationId::unique::<NoteHistoryView>(),
                            message,
                        )
                        .autohide(),
                        cx,
                    )
                })
                .log_err();
            this.update(cx, |this, cx| this.load(cx)).ok();
        })
        .detach();
    }

    fn title(&self) -> String {
        self.note
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Note".into())
    }

    fn render_row(
        &self,
        id: impl Into<ElementId>,
        selected: bool,
        heading: String,
        detail: String,
        on_click: impl Fn(&mut Self, &mut gpui::Context<Self>) + 'static,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        let colors = cx.theme().colors();
        v_flex()
            .id(id)
            .w_full()
            .px_3()
            .py_2()
            .gap_0p5()
            .rounded_md()
            .cursor_pointer()
            .when(selected, |row| row.bg(colors.element_selected))
            .hover(|row| row.bg(colors.element_hover))
            .child(Label::new(heading).size(LabelSize::Small))
            .child(
                Label::new(detail)
                    .size(LabelSize::Small)
                    .color(Color::Muted)
                    .truncate(),
            )
            .on_click(cx.listener(move |this, _, _, cx| on_click(this, cx)))
    }
}

impl EventEmitter<()> for NoteHistoryView {}

impl Focusable for NoteHistoryView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Item for NoteHistoryView {
    type Event = ();

    fn to_item_events(_: &Self::Event, _: &mut dyn FnMut(workspace::item::ItemEvent)) {}

    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        format!("History · {}", self.title()).into()
    }

    fn tab_icon(&self, _window: &Window, _cx: &App) -> Option<Icon> {
        Some(Icon::new(IconName::HistoryRerun))
    }

    fn telemetry_event_text(&self) -> Option<&'static str> {
        None
    }
}

impl Render for NoteHistoryView {
    fn render(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let editor_background = cx.theme().colors().editor_background;
        let border_variant = cx.theme().colors().border_variant;
        let mut list = v_flex().w(px(280.)).flex_none().gap_0p5().p_2();
        if self.current.is_some() {
            list = list.child(self.render_row(
                "note-history-now",
                self.selected.is_none(),
                "Now".into(),
                "Edits since the last sync".into(),
                |this, cx| this.select(None, cx),
                cx,
            ));
        }
        for (index, version) in self.versions.iter().enumerate() {
            list = list.child(self.render_row(
                ("note-history-version", index),
                self.selected == Some(index),
                when(&version.date),
                version.message.clone(),
                move |this, cx| this.select(Some(index), cx),
                cx,
            ));
        }
        if !self.loading && self.versions.is_empty() && self.current.is_none() {
            list = list.child(
                Label::new("No saved versions yet. Each sync to GitHub adds one.")
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            );
        }
        let can_restore = self.selected.is_some() && self.preview_text.is_some();
        let preview = v_flex()
            .id("note-history-preview")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .p_6()
            .gap_4()
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .child(
                        Label::new(
                            match self.selected.and_then(|index| self.versions.get(index)) {
                                Some(version) => {
                                    format!("{} as of {}", self.title(), when(&version.date))
                                }
                                None => format!("{} now", self.title()),
                            },
                        )
                        .weight(gpui::FontWeight::SEMIBOLD),
                    )
                    .child(
                        Button::new("note-history-restore", "Restore this version")
                            .style(ButtonStyle::Filled)
                            .label_size(LabelSize::Small)
                            .disabled(!can_restore)
                            .on_click(cx.listener(|this, _, _, cx| this.restore(cx))),
                    ),
            )
            .when_some(self.error.clone(), |preview, error| {
                preview.child(Label::new(error).color(Color::Error))
            })
            .child(MarkdownElement::new(
                self.preview.clone(),
                crate::brief::BriefView::narrative_style(window, cx),
            ));
        h_flex()
            .key_context("BrainzNoteHistory")
            .track_focus(&self.focus_handle)
            .size_full()
            .items_start()
            .bg(editor_background)
            .child(
                div()
                    .id("note-history-list")
                    .h_full()
                    .overflow_y_scroll()
                    .border_r_1()
                    .border_color(border_variant)
                    .child(list),
            )
            .child(preview)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_records_carry_each_versions_path() {
        let log = "\u{1e}abc123\u{1f}2026-10-22T15:04:05-04:00\u{1f}sync: notes\n\nwork/plan.md\n\u{1e}def456\u{1f}2026-10-20T09:00:00-04:00\u{1f}Rename\n\nold/plan.md\n";
        let versions = parse_log(log);
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[0].commit, "abc123");
        assert_eq!(versions[0].message, "sync: notes");
        assert_eq!(versions[0].path, "work/plan.md");
        assert_eq!(versions[1].path, "old/plan.md");
        assert!(parse_log("").is_empty());
    }

    #[test]
    #[allow(
        clippy::disallowed_methods,
        reason = "a test setting up a git repository"
    )]
    fn versions_follow_renames_and_read_old_text() {
        let brain = tempfile::tempdir().unwrap();
        let root = brain.path();
        let run = |args: &[&str]| {
            let status = crate::github_sync::command(root, "git")
                .args(["-c", "user.email=t@t", "-c", "user.name=t"])
                .args(args)
                .status()
                .unwrap();
            assert!(status.success());
        };
        run(&["init", "-q"]);
        std::fs::write(root.join("plan.md"), "# Plan\n\nFirst.\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-qm", "first"]);
        std::fs::create_dir_all(root.join("work")).unwrap();
        run(&["mv", "plan.md", "work/plan.md"]);
        std::fs::write(root.join("work/plan.md"), "# Plan\n\nSecond.\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-qm", "second"]);

        let found = versions(root, "work/plan.md").unwrap();
        assert_eq!(
            found
                .iter()
                .map(|version| version.message.as_str())
                .collect::<Vec<_>>(),
            ["second", "first"]
        );
        assert_eq!(text_at(root, &found[1]).unwrap(), "# Plan\n\nFirst.\n");
        assert_eq!(found[1].path, "plan.md");
    }
}
