//! Brainz: a To-Do tab over the brain's check-off board (`todo` in
//! `brainz.toml`, `TODO.md` by default). Brainz only flips checkboxes; a bot
//! or the person keeps the wording and sections in order. Below the board,
//! "Flagged in notes" lists the ⏳ and ⏰ lines the signals pass found in
//! notes, each with Add to To-Do (copies it onto the board and ticks the
//! note) or Dismiss (ticks the note).

use std::{path::PathBuf, time::Duration};

use anyhow::{Context as _, Result};
use chrono::Local;
use gpui::{App, EventEmitter, FocusHandle, Focusable, Task, WeakEntity, Window, actions};
use ui::{Tooltip, prelude::*};
use workspace::{
    HideStatusItem, Item, ItemHandle, OpenOptions, OpenVisible, StatusItemView, Workspace,
};

use crate::{
    brain_config::BrainConfig,
    myman, open_loops,
    themes::runner,
    themes_signals::{self as signals, OpenLoop},
};

actions!(
    brainz_todo,
    [
        /// Opens the To-Do tab.
        OpenTodo
    ]
);

const REFRESH_INTERVAL: Duration = Duration::from_secs(30);

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &OpenTodo, window, cx| {
            TodoView::open(workspace, window, cx);
        });
    })
    .detach();
}

#[derive(Debug, Clone)]
pub(crate) struct TodoItem {
    /// Line index in the file, so toggles edit the right line.
    pub(crate) line: usize,
    pub(crate) done: bool,
    pub(crate) title: String,
    pub(crate) tag: Option<String>,
    pub(crate) note: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct TodoSection {
    pub(crate) name: String,
    pub(crate) items: Vec<TodoItem>,
    pub(crate) empty_note: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct TodoBoard {
    pub(crate) sections: Vec<TodoSection>,
    pub(crate) updated: Option<String>,
}

/// `- [ ] Title with words `tag` (note)` → parts.
fn parse_item(line: &str, index: usize) -> Option<TodoItem> {
    let trimmed = line.trim_start();
    let rest = trimmed
        .strip_prefix("- [ ] ")
        .map(|rest| (false, rest))
        .or_else(|| trimmed.strip_prefix("- [x] ").map(|rest| (true, rest)))
        .or_else(|| trimmed.strip_prefix("- [X] ").map(|rest| (true, rest)))?;
    let (done, body) = rest;
    let mut body = body.trim().to_owned();
    let mut note = None;
    if body.ends_with(')')
        && let Some(open) = body.rfind(" (")
    {
        note = Some(body[open + 2..body.len() - 1].to_owned());
        body.truncate(open);
    }
    let mut tag = None;
    if body.ends_with('`')
        && let Some(open) = body[..body.len() - 1].rfind('`')
    {
        tag = Some(body[open + 1..body.len() - 1].to_owned());
        body.truncate(open);
    }
    Some(TodoItem {
        line: index,
        done,
        title: body.trim().to_owned(),
        tag,
        note,
    })
}

pub(crate) fn parse_board(text: &str) -> TodoBoard {
    let mut board = TodoBoard::default();
    for (index, line) in text.lines().enumerate() {
        if let Some(updated) = line.strip_prefix("**Last updated:**") {
            board.updated = Some(updated.trim().to_owned());
        } else if let Some(name) = line.strip_prefix("## ") {
            board.sections.push(TodoSection {
                name: name.trim().to_owned(),
                items: Vec::new(),
                empty_note: None,
            });
        } else if let Some(section) = board.sections.last_mut() {
            if let Some(item) = parse_item(line, index) {
                section.items.push(item);
            } else if let Some(note) = line
                .trim()
                .strip_prefix('_')
                .and_then(|s| s.strip_suffix('_'))
            {
                section.empty_note = Some(note.trim_matches(|c| c == '(' || c == ')').to_owned());
            }
        }
    }
    board
}

fn load_board(path: &PathBuf) -> Result<TodoBoard> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(parse_board(&text))
}

/// The open loops of the last signals pass, oldest first; none when the
/// pass has not run.
fn load_loops(repo: &PathBuf) -> Vec<OpenLoop> {
    let config = BrainConfig::load(repo);
    let mut loops = signals::load_signals(repo, &config)
        .map(|signals| signals.open_loops)
        .unwrap_or_default();
    loops.sort_by(|a, b| a.first_seen.cmp(&b.first_seen));
    loops
}

/// Flips one checkbox in place. Checking appends today's date; the item
/// stays in its section so it reads as done rather than vanishing; whoever
/// maintains the board tidies it into "## Done" later.
fn toggle_item(path: &PathBuf, line_index: usize) -> Result<()> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let Some(line) = lines.get(line_index).cloned() else {
        anyhow::bail!("the to-do list changed underneath; refresh and try again");
    };
    let trimmed = line.trim_start();
    let indent = &line[..line.len() - trimmed.len()];
    if let Some(rest) = trimmed.strip_prefix("- [ ] ") {
        let today = chrono::Local::now().format("%Y-%m-%d");
        lines[line_index] = format!("{indent}- [x] {} ({today})", rest.trim_end());
    } else if let Some(rest) = trimmed
        .strip_prefix("- [x] ")
        .or_else(|| trimmed.strip_prefix("- [X] "))
    {
        lines[line_index] = format!("{indent}- [ ] {rest}");
    } else {
        anyhow::bail!("that line is not a to-do item any more; refresh and try again");
    }
    let mut output = lines.join("\n");
    if text.ends_with('\n') {
        output.push('\n');
    }
    std::fs::write(path, output).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

pub struct TodoView {
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    repo: PathBuf,
    path: PathBuf,
    /// The configured path, for the error card.
    relative: String,
    owner: String,
    board: TodoBoard,
    /// ⏳ and ⏰ lines from notes, oldest first, from the last signals pass.
    loops: Vec<OpenLoop>,
    /// Open tasks My Man pulled out of meetings and notes, not yet on the
    /// board or dismissed.
    myman_tasks: Vec<myman::MyManTask>,
    error: Option<String>,
    show_done: bool,
    _load: Option<Task<()>>,
    _refresh_loop: Task<()>,
}

impl TodoView {
    pub fn open(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
        let existing = workspace.items_of_type::<TodoView>(cx).next();
        if let Some(existing) = existing {
            workspace.activate_item(&existing, true, true, window, cx);
            existing.update(cx, |view, cx| view.refresh(cx));
            return;
        }
        let Some(root) = workspace
            .root_paths(cx)
            .first()
            .map(|path| path.to_path_buf())
        else {
            return;
        };
        let config = BrainConfig::load(&root);
        let relative = config.todo.clone();
        let path = root.join(&relative);
        let owner = config.owner_label();
        let weak = cx.entity().downgrade();
        let view = cx.new(|cx| TodoView::new(weak, root, path, relative, owner, cx));
        workspace.add_item_to_active_pane(Box::new(view), None, true, window, cx);
    }

    fn new(
        workspace: WeakEntity<Workspace>,
        repo: PathBuf,
        path: PathBuf,
        relative: String,
        owner: String,
        cx: &mut Context<Self>,
    ) -> Self {
        let refresh_loop = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(REFRESH_INTERVAL).await;
                if this.update(cx, |this, cx| this.refresh(cx)).is_err() {
                    break;
                }
            }
        });
        let mut this = Self {
            focus_handle: cx.focus_handle(),
            workspace,
            repo,
            path,
            relative,
            owner,
            board: TodoBoard::default(),
            loops: Vec::new(),
            myman_tasks: Vec::new(),
            error: None,
            show_done: false,
            _load: None,
            _refresh_loop: refresh_loop,
        };
        this.refresh(cx);
        this
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let path = self.path.clone();
        let repo = self.repo.clone();
        self._load = Some(cx.spawn(async move |this, cx| {
            let (result, loops, myman_tasks) = cx
                .background_spawn(async move {
                    let myman_tasks = myman::MyMan::detect(&BrainConfig::load(&repo))
                        .map(|myman| myman::pending_tasks(&myman))
                        .unwrap_or_default();
                    (load_board(&path), load_loops(&repo), myman_tasks)
                })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(board) => {
                        this.board = board;
                        this.error = None;
                    }
                    Err(error) => this.error = Some(format!("{error:#}")),
                }
                this.loops = loops;
                this.myman_tasks = myman_tasks;
                cx.notify();
            })
            .ok();
        }));
    }

    /// Copies a My Man task onto the board under Today (or just remembers
    /// it as dismissed), so it stops showing here.
    fn resolve_myman_task(&mut self, ix: usize, add: bool, cx: &mut Context<Self>) {
        if ix >= self.myman_tasks.len() {
            return;
        }
        let task = self.myman_tasks.remove(ix);
        cx.notify();
        let board = self.path.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    if add {
                        let text = std::fs::read_to_string(&board)
                            .with_context(|| format!("reading {}", board.display()))?;
                        let output = open_loops::insert_board_line(&text, &task.board_line(), true);
                        std::fs::write(&board, output)
                            .with_context(|| format!("writing {}", board.display()))?;
                    }
                    myman::mark_task_handled(&task)
                })
                .await;
            this.update(cx, |this, cx| {
                if let Err(error) = result {
                    log::error!("brainz my man task: {error:#}");
                    this.error = Some(format!("Could not update the board: {error:#}"));
                }
                this.refresh(cx);
            })
            .ok();
        })
        .detach();
    }

    fn render_myman_tasks(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        if self.myman_tasks.is_empty() {
            return None;
        }
        let mut block = v_flex()
            .w_full()
            .gap_0p5()
            .child(
                h_flex()
                    .px_2()
                    .pt_5()
                    .pb_0p5()
                    .gap_2()
                    .child(
                        Label::new("From My Man")
                            .size(LabelSize::Small)
                            .weight(gpui::FontWeight::SEMIBOLD)
                            .color(Color::Accent),
                    )
                    .child(
                        Label::new(self.myman_tasks.len().to_string())
                            .size(LabelSize::Small)
                            .color(Color::Placeholder),
                    ),
            )
            .child(
                div().px_2().pb_1().child(
                    Label::new(
                        "Tasks My Man heard in meetings or found in notes. Add to To-Do puts one under Today; Dismiss hides it here and leaves My Man's own list alone.",
                    )
                    .size(LabelSize::Small)
                    .color(Color::Muted),
                ),
            );
        for (ix, task) in self.myman_tasks.iter().enumerate() {
            let when = task
                .date
                .map(|date| format!(" · {date}"))
                .unwrap_or_default();
            block = block.child(
                h_flex()
                    .id(("brainz-todo-myman", ix))
                    .w_full()
                    .items_center()
                    .gap_2p5()
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .hover(|this| this.bg(cx.theme().colors().element_hover))
                    .child(
                        v_flex()
                            .min_w_0()
                            .flex_1()
                            .child(Label::new(task.title.clone()).truncate())
                            .child(
                                Label::new(format!("My Man {}{when}", task.source))
                                    .size(LabelSize::Small)
                                    .color(Color::Placeholder),
                            ),
                    )
                    .child(
                        Button::new(("brainz-todo-myman-add", ix), "Add to To-Do")
                            .label_size(LabelSize::Small)
                            .style(ButtonStyle::Filled)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.resolve_myman_task(ix, true, cx);
                            })),
                    )
                    .child(
                        Button::new(("brainz-todo-myman-dismiss", ix), "Dismiss")
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.resolve_myman_task(ix, false, cx);
                            })),
                    ),
            );
        }
        Some(block.into_any_element())
    }

    /// Ticks the loop's marker in its note and, when `add` is set, copies
    /// it onto the board first. The signals pass re-runs so the row stays
    /// gone after the next refresh.
    fn resolve_loop(&mut self, ix: usize, add: bool, cx: &mut Context<Self>) {
        let Some(open_loop) = self.loops.get(ix).cloned() else {
            return;
        };
        self.loops.remove(ix);
        cx.notify();
        let note = self.repo.join(&open_loop.file);
        let board = self.path.clone();
        let owner = self.owner.clone();
        let repo = self.repo.clone();
        let workspace = self.workspace.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    if add {
                        open_loops::add_to_board(&board, &open_loop, &owner)?;
                    }
                    open_loops::strike_marker(&note, &open_loop)
                })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        if let Some(runner) = runner(cx) {
                            runner.update(cx, |runner, cx| runner.run(repo, cx));
                        }
                    }
                    Err(error) => {
                        log::error!("brainz flagged line: {error:#}");
                        workspace
                            .update(cx, |workspace, cx| {
                                let id = workspace::notifications::NotificationId::unique::<TodoView>();
                                workspace.show_notification(id, cx, |cx| {
                                    cx.new(|cx| {
                                        workspace::notifications::simple_message_notification::MessageNotification::new(
                                            format!("Could not update the note: {error:#}"),
                                            cx,
                                        )
                                    })
                                });
                            })
                            .ok();
                    }
                }
                this.refresh(cx);
            })
            .ok();
        })
        .detach();
    }

    fn open_relative(&self, relative: &str, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.repo.join(relative);
        if !target.exists() {
            return;
        }
        self.workspace
            .update(cx, |workspace, cx| {
                workspace
                    .open_abs_path(
                        target,
                        OpenOptions {
                            visible: Some(OpenVisible::None),
                            ..Default::default()
                        },
                        window,
                        cx,
                    )
                    .detach_and_log_err(cx);
            })
            .ok();
    }

    fn render_loops(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let today = Local::now().date_naive();
        let mut block = v_flex()
            .w_full()
            .gap_0p5()
            .child(
                h_flex()
                    .px_2()
                    .pt_5()
                    .pb_0p5()
                    .gap_2()
                    .child(
                        Label::new("Flagged in notes")
                            .size(LabelSize::Small)
                            .weight(gpui::FontWeight::SEMIBOLD),
                    )
                    .child(
                        Label::new(self.loops.len().to_string())
                            .size(LabelSize::Small)
                            .color(Color::Placeholder),
                    ),
            )
            .child(
                div().px_2().pb_1().child(
                    Label::new(
                        "Lines in your notes that start with ⏳ (waiting on someone) or ⏰ (you owe it) and never made it onto the board. Add to To-Do copies one here and ticks the note; Dismiss just ticks the note.",
                    )
                    .size(LabelSize::Small)
                    .color(Color::Muted),
                ),
            );
        if self.loops.is_empty() {
            return block
                .child(
                    div().px_2().py_1().child(
                        Label::new("Nothing flagged")
                            .size(LabelSize::Small)
                            .color(Color::Placeholder),
                    ),
                )
                .into_any_element();
        }
        for (ix, open_loop) in self.loops.iter().enumerate() {
            let text = markdown::markdown_to_plain_text(&open_loop.text)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let who = match (&open_loop.counterparty, open_loop.owed_by_owner()) {
                (_, true) => format!("{} owes", self.owner),
                (Some(person), false) => format!("Waiting on {person}"),
                (None, false) => "Waiting".to_owned(),
            };
            let file = open_loop.file.clone();
            let source = signals::file_label(&open_loop.file);
            let age = open_loop.age_days(today);
            block = block.child(
                h_flex()
                    .id(("brainz-todo-loop", ix))
                    .w_full()
                    .items_center()
                    .gap_2p5()
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .hover(|this| this.bg(cx.theme().colors().element_hover))
                    .child(
                        div()
                            .w(px(20.))
                            .flex_none()
                            .child(Label::new(open_loop.marker.clone()).size(LabelSize::Small)),
                    )
                    .child(
                        v_flex()
                            .min_w_0()
                            .flex_1()
                            .child(Label::new(text).truncate())
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(Label::new(who).size(LabelSize::Small).color(
                                        if open_loop.owed_by_owner() {
                                            Color::Warning
                                        } else {
                                            Color::Muted
                                        },
                                    ))
                                    .child(
                                        Label::new(format!("{age} days · {source}"))
                                            .size(LabelSize::Small)
                                            .color(Color::Placeholder),
                                    ),
                            ),
                    )
                    .child(
                        Button::new(("brainz-todo-loop-open", ix), "Open note")
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_relative(&file, window, cx);
                            })),
                    )
                    .child(
                        Button::new(("brainz-todo-loop-add", ix), "Add to To-Do")
                            .label_size(LabelSize::Small)
                            .style(ButtonStyle::Filled)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.resolve_loop(ix, true, cx);
                            })),
                    )
                    .child(
                        Button::new(("brainz-todo-loop-dismiss", ix), "Dismiss")
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.resolve_loop(ix, false, cx);
                            })),
                    ),
            );
        }
        block.into_any_element()
    }

    fn toggle(&mut self, line: usize, cx: &mut Context<Self>) {
        let path = self.path.clone();
        self._load = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    toggle_item(&path, line)?;
                    load_board(&path)
                })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(board) => {
                        this.board = board;
                        this.error = None;
                    }
                    Err(error) => this.error = Some(format!("{error:#}")),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn render_item(&self, ix: usize, item: &TodoItem, cx: &mut Context<Self>) -> impl IntoElement {
        let line = item.line;
        let icon = if item.done {
            IconName::BrainzCheckboxChecked
        } else {
            IconName::BrainzCheckboxEmpty
        };
        h_flex()
            .id(("brainz-todo-item", ix))
            .w_full()
            .items_start()
            .gap_2p5()
            .px_2()
            .py_1p5()
            .rounded_md()
            .hover(|this| this.bg(cx.theme().colors().element_hover))
            .child(
                IconButton::new(("brainz-todo-check", ix), icon)
                    .icon_size(IconSize::Small)
                    .icon_color(if item.done {
                        Color::Accent
                    } else {
                        Color::Muted
                    })
                    .tooltip(Tooltip::text(if item.done {
                        "Mark as not done"
                    } else {
                        "Mark done"
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle(line, cx))),
            )
            .child(
                v_flex()
                    .min_w_0()
                    .flex_1()
                    .child(
                        Label::new(item.title.clone())
                            .color(if item.done {
                                Color::Accent
                            } else {
                                Color::Default
                            })
                            .when(item.done, |label| label.strikethrough()),
                    )
                    .when(item.tag.is_some() || item.note.is_some(), |this| {
                        this.child(
                            h_flex()
                                .gap_2()
                                .when_some(item.note.clone(), |this, note| {
                                    this.child(
                                        Label::new(note).size(LabelSize::Small).color(Color::Muted),
                                    )
                                })
                                .when_some(item.tag.clone(), |this, tag| {
                                    this.child(
                                        Label::new(tag)
                                            .size(LabelSize::Small)
                                            .color(Color::Placeholder),
                                    )
                                }),
                        )
                    }),
            )
    }
}

impl EventEmitter<()> for TodoView {}

impl Focusable for TodoView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Item for TodoView {
    type Event = ();

    fn to_item_events(_: &Self::Event, _: &mut dyn FnMut(workspace::item::ItemEvent)) {}

    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        "To-Do".into()
    }

    fn tab_icon(&self, _window: &Window, _cx: &App) -> Option<Icon> {
        Some(Icon::new(IconName::BrainzCheckboxChecked))
    }

    fn telemetry_event_text(&self) -> Option<&'static str> {
        None
    }
}

impl Render for TodoView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let open_count: usize = self
            .board
            .sections
            .iter()
            .flat_map(|section| section.items.iter())
            .filter(|item| !item.done)
            .count();
        let header = h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .child(
                h_flex()
                    .gap_2()
                    .child(Icon::new(IconName::BrainzCheckboxChecked).color(Color::Muted))
                    .child(Label::new("To-Do").size(LabelSize::Large))
                    .child(
                        Label::new(format!("{open_count} open"))
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .when_some(self.board.updated.clone(), |this, updated| {
                        this.child(
                            Label::new(format!("Updated {updated}"))
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        )
                    })
                    .child(
                        IconButton::new("brainz-todo-refresh", IconName::ArrowCircle)
                            .icon_size(IconSize::Small)
                            .tooltip(Tooltip::text("Refresh"))
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                    ),
            );

        let mut body: Vec<gpui::AnyElement> = Vec::new();
        if let Some(error) = &self.error {
            body.push(
                v_flex()
                    .p_4()
                    .gap_2()
                    .rounded_lg()
                    .border_1()
                    .border_color(cx.theme().colors().border)
                    .bg(cx.theme().colors().surface_background)
                    .child(Label::new(error.clone()))
                    .child(
                        Label::new(format!(
                            "Brainz looks for {} in the open project (set `todo` in brainz.toml to change it).",
                            self.relative
                        ))
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                    )
                    .into_any_element(),
            );
        }
        let mut item_ix = 0usize;
        for section in &self.board.sections {
            let is_done = section.name.eq_ignore_ascii_case("done");
            let mut block = v_flex().w_full().gap_0p5().child(
                h_flex()
                    .px_2()
                    .pt_3()
                    .pb_1()
                    .gap_2()
                    .child(
                        Label::new(section.name.clone())
                            .size(LabelSize::Small)
                            .weight(gpui::FontWeight::SEMIBOLD)
                            .color(if is_done {
                                Color::Muted
                            } else {
                                Color::Default
                            }),
                    )
                    .child(
                        Label::new(section.items.len().to_string())
                            .size(LabelSize::Small)
                            .color(Color::Placeholder),
                    )
                    .when(is_done, |this| {
                        this.child(
                            Button::new(
                                "brainz-todo-toggle-done",
                                if self.show_done { "Hide" } else { "Show" },
                            )
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_done = !this.show_done;
                                cx.notify();
                            })),
                        )
                    }),
            );
            if is_done && !self.show_done {
                body.push(block.into_any_element());
                continue;
            }
            if section.items.is_empty() {
                block = block.child(
                    div().px_2().py_1().child(
                        Label::new(
                            section
                                .empty_note
                                .clone()
                                .unwrap_or_else(|| "Nothing here".into()),
                        )
                        .size(LabelSize::Small)
                        .color(Color::Placeholder),
                    ),
                );
            }
            for item in &section.items {
                block = block.child(ui::reveal(
                    ("brainz-todo-item-reveal", item_ix),
                    item_ix,
                    self.render_item(item_ix, item, cx),
                ));
                item_ix += 1;
            }
            body.push(block.into_any_element());
        }
        body.push(self.render_loops(cx));
        body.extend(self.render_myman_tasks(cx));

        v_flex()
            .id("brainz-todo")
            .key_context("BrainzTodo")
            .track_focus(&self.focus_handle)
            .size_full()
            .overflow_y_scroll()
            .bg(cx.theme().colors().editor_background)
            .child(
                v_flex()
                    .w_full()
                    .max_w(px(760.))
                    .mx_auto()
                    .pt_6()
                    .pb_10()
                    .px_4()
                    .gap_1()
                    .child(header)
                    .children(body),
            )
    }
}

/// Status bar button that opens the To-Do tab.
pub struct TodoButton {
    pane_item_focus_handle: Option<FocusHandle>,
    /// The To-Do tab is the active item, so the button lights up.
    active: bool,
}

impl TodoButton {
    pub fn new() -> Self {
        Self {
            pane_item_focus_handle: None,
            active: false,
        }
    }
}

impl Render for TodoButton {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let focus_handle = self.pane_item_focus_handle.clone();
        let active = self.active;
        div().child(
            IconButton::new("brainz-todo-button", IconName::BrainzCheckboxChecked)
                .icon_size(IconSize::Small)
                .toggle_state(active)
                .icon_color(if active {
                    Color::Accent
                } else {
                    Color::Default
                })
                .tooltip(move |_window, cx| {
                    if let Some(focus_handle) = &focus_handle {
                        Tooltip::for_action_in("To-Do", &OpenTodo, focus_handle, cx)
                    } else {
                        Tooltip::for_action("To-Do", &OpenTodo, cx)
                    }
                })
                .on_click(|_, window, cx| {
                    window.dispatch_action(Box::new(OpenTodo), cx);
                }),
        )
    }
}

impl StatusItemView for TodoButton {
    fn set_active_pane_item(
        &mut self,
        active_pane_item: Option<&dyn ItemHandle>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pane_item_focus_handle = active_pane_item.map(|item| item.item_focus_handle(cx));
        self.active = active_pane_item.is_some_and(|item| item.downcast::<TodoView>().is_some());
        cx.notify();
    }

    fn hide_setting(&self, _: &App) -> Option<HideStatusItem> {
        None
    }
}
