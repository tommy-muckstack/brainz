//! Brainz: a To-Do tab over the brain's check-off board (`todo` in
//! `brainz.toml`, `TODO.md` by default). The tab checks items off, adds new
//! ones, edits their wording and priority, and drags them between positions
//! and sections; every change is a line edit to the Markdown file, so a bot
//! keeping the board reads the same file it always did. Below the board,
//! "Flagged in notes" lists the ⏳ and ⏰ lines the signals pass found in
//! notes, each with Add to To-Do (copies it onto the board and ticks the
//! note) or Dismiss (ticks the note).

use std::{
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};

use anyhow::{Context as _, Result};
use chrono::Local;
use editor::{Editor, EditorEvent};
use gpui::{
    App, Entity, EventEmitter, FocusHandle, Focusable, Subscription, Task, WeakEntity, Window,
    actions,
};
use ui::{ContextMenu, PopoverMenu, Tooltip, prelude::*};
use workspace::{Item, OpenOptions, OpenVisible, Workspace};

use crate::{
    brain_config::BrainConfig,
    insights::{Feature, Insight},
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

/// Board writes run on background threads; each is a read-modify-write of
/// the whole file, so two quick edits must not interleave.
static BOARD_LOCK: Mutex<()> = Mutex::new(());

const STALE_BOARD: &str = "the to-do list changed underneath; refresh and try again";

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &OpenTodo, window, cx| {
            TodoView::open(workspace, window, cx);
        });
    })
    .detach();
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Priority {
    High,
    Medium,
    Low,
}

impl Priority {
    const ALL: [Priority; 3] = [Priority::High, Priority::Medium, Priority::Low];

    /// Written at the start of the title, where it stays readable in the
    /// raw file and in Notion.
    fn marker(self) -> &'static str {
        match self {
            Priority::High => "[P1]",
            Priority::Medium => "[P2]",
            Priority::Low => "[P3]",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Priority::High => "High",
            Priority::Medium => "Medium",
            Priority::Low => "Low",
        }
    }

    fn color(self) -> Color {
        match self {
            Priority::High => Color::Error,
            Priority::Medium => Color::Warning,
            Priority::Low => Color::Muted,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TodoItem {
    /// Line index in the file, so edits land on the right line.
    pub(crate) line: usize,
    /// The line as read, so an edit can tell when the file moved on.
    pub(crate) raw: String,
    pub(crate) done: bool,
    pub(crate) priority: Option<Priority>,
    pub(crate) title: String,
    pub(crate) tag: Option<String>,
    pub(crate) note: Option<String>,
}

impl TodoItem {
    /// The item's line with a new priority and title, keeping its
    /// indentation, checkbox, tag and note.
    fn line_with(&self, priority: Option<Priority>, title: &str) -> String {
        let indent = &self.raw[..self.raw.len() - self.raw.trim_start().len()];
        let check = if self.done { "x" } else { " " };
        let mut line = format!("{indent}- [{check}] ");
        if let Some(priority) = priority {
            line.push_str(priority.marker());
            line.push(' ');
        }
        line.push_str(&title.split_whitespace().collect::<Vec<_>>().join(" "));
        if let Some(tag) = &self.tag {
            line.push_str(&format!(" `{tag}`"));
        }
        if let Some(note) = &self.note {
            line.push_str(&format!(" ({note})"));
        }
        line
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TodoSection {
    /// Line index of the `## ` heading.
    pub(crate) line: usize,
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
    let mut title = body.trim();
    let mut priority = None;
    for candidate in Priority::ALL {
        if let Some(rest) = title.strip_prefix(candidate.marker()) {
            priority = Some(candidate);
            title = rest.trim_start();
            break;
        }
    }
    Some(TodoItem {
        line: index,
        raw: line.to_owned(),
        done,
        priority,
        title: title.to_owned(),
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
                line: index,
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

/// What a brain without a to-do file starts from. The tab shows it as an
/// empty board and the file is written on the first edit.
const STARTER_BOARD: &str = "# To-Do\n\n## Today\n\n## Waiting\n\n## Done\n";

pub(crate) fn read_board_text(path: &Path) -> Result<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(STARTER_BOARD.to_owned()),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

pub(crate) fn write_board_text(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))
}

fn load_board(path: &Path) -> Result<TodoBoard> {
    Ok(parse_board(&read_board_text(path)?))
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

/// Where a dragged item lands: just above another item, or at the end of
/// a section (dropped on its heading or its add row).
#[derive(Debug, Clone)]
pub(crate) enum DropTarget {
    Before { line: usize, raw: String },
    EndOfSection { line: usize },
}

fn lines_of(text: &str) -> Vec<String> {
    text.lines().map(str::to_owned).collect()
}

fn join_lines(lines: Vec<String>, original: &str) -> String {
    let mut output = lines.join("\n");
    if original.ends_with('\n') {
        output.push('\n');
    }
    output
}

fn ensure_line(lines: &[String], line_index: usize, expected: &str) -> Result<()> {
    anyhow::ensure!(
        lines.get(line_index).is_some_and(|line| line == expected),
        STALE_BOARD
    );
    Ok(())
}

/// Flips one checkbox in place. Checking appends today's date; the item
/// stays in its section so it reads as done rather than vanishing; whoever
/// maintains the board tidies it into "## Done" later.
fn toggle_line(text: &str, line_index: usize, expected: &str) -> Result<String> {
    let mut lines = lines_of(text);
    ensure_line(&lines, line_index, expected)?;
    let line = lines[line_index].clone();
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
    Ok(join_lines(lines, text))
}

fn replace_line(text: &str, line_index: usize, expected: &str, new_line: &str) -> Result<String> {
    let mut lines = lines_of(text);
    ensure_line(&lines, line_index, expected)?;
    lines[line_index] = new_line.to_owned();
    Ok(join_lines(lines, text))
}

/// The index a new last item of the section headed at `heading` goes to.
/// An italic "nothing here" note in an empty section is removed, since the
/// section is about to have an item.
fn end_of_section(lines: &mut Vec<String>, heading: usize) -> usize {
    let end = (heading + 1..lines.len())
        .find(|&ix| lines[ix].starts_with("## "))
        .unwrap_or(lines.len());
    if let Some(last_item) =
        (heading + 1..end).rfind(|&ix| lines[ix].trim_start().starts_with("- ["))
    {
        return last_item + 1;
    }
    let mut ix = heading + 1;
    while ix < end && lines[ix].trim().is_empty() {
        ix += 1;
    }
    if ix < end && lines[ix].trim().starts_with('_') {
        lines.remove(ix);
        return ix;
    }
    if ix == heading + 1 {
        // Keep the blank line between a heading and its list.
        lines.insert(ix, String::new());
        return ix + 1;
    }
    ix
}

fn insert_into_section(text: &str, heading: usize, new_line: &str) -> Result<String> {
    let mut lines = lines_of(text);
    anyhow::ensure!(
        lines
            .get(heading)
            .is_some_and(|line| line.starts_with("## ")),
        STALE_BOARD
    );
    let at = end_of_section(&mut lines, heading);
    lines.insert(at, new_line.to_owned());
    Ok(join_lines(lines, text))
}

fn move_line(text: &str, from: usize, expected: &str, target: &DropTarget) -> Result<String> {
    let mut lines = lines_of(text);
    ensure_line(&lines, from, expected)?;
    match target {
        DropTarget::Before { line, raw } => ensure_line(&lines, *line, raw)?,
        DropTarget::EndOfSection { line } => anyhow::ensure!(
            lines
                .get(*line)
                .is_some_and(|heading| heading.starts_with("## ")),
            STALE_BOARD
        ),
    }
    let moved = lines.remove(from);
    let shifted = |ix: usize| if ix > from { ix - 1 } else { ix };
    let at = match target {
        DropTarget::Before { line, .. } => shifted(*line),
        DropTarget::EndOfSection { line } => end_of_section(&mut lines, shifted(*line)),
    };
    lines.insert(at, moved);
    Ok(join_lines(lines, text))
}

#[derive(Clone)]
struct DraggedTodo {
    line: usize,
    raw: String,
    title: String,
}

impl Render for DraggedTodo {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .px_2()
            .py_1()
            .gap_2()
            .rounded_md()
            .border_1()
            .border_color(cx.theme().colors().border)
            .bg(cx.theme().colors().elevated_surface_background)
            .shadow_md()
            .max_w(px(420.))
            .child(
                Icon::new(IconName::BrainzCheckboxEmpty)
                    .size(IconSize::Small)
                    .color(Color::Muted),
            )
            .child(Label::new(self.title.clone()).truncate())
    }
}

/// The title being edited in place, or a new item being typed into a
/// section.
enum InlineInput {
    Edit {
        item: TodoItem,
        editor: Entity<Editor>,
        _subscription: Subscription,
    },
    Add {
        heading: usize,
        editor: Entity<Editor>,
        _subscription: Subscription,
    },
}

impl InlineInput {
    fn editor(&self) -> &Entity<Editor> {
        match self {
            InlineInput::Edit { editor, .. } | InlineInput::Add { editor, .. } => editor,
        }
    }
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
    insight: Entity<Insight>,
    /// ⏳ and ⏰ lines from notes, oldest first, from the last signals pass.
    loops: Vec<OpenLoop>,
    /// Open tasks My Man pulled out of meetings and notes, not yet on the
    /// board or dismissed.
    myman_tasks: Vec<myman::MyManTask>,
    error: Option<String>,
    show_done: bool,
    /// No board file yet: the tab shows the starter board until the first add.
    file_missing: bool,
    input: Option<InlineInput>,
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
            insight: Insight::for_repo(&repo, Feature::Todo, cx),
            repo,
            path,
            relative,
            owner,
            board: TodoBoard::default(),
            loops: Vec::new(),
            myman_tasks: Vec::new(),
            error: None,
            show_done: false,
            file_missing: false,
            input: None,
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
            let (result, loops, myman_tasks, file_missing) = cx
                .background_spawn(async move {
                    let myman_tasks = myman::MyMan::detect(&BrainConfig::load(&repo))
                        .map(|myman| myman::pending_tasks(&myman))
                        .unwrap_or_default();
                    let file_missing = !path.exists();
                    (
                        load_board(&path),
                        load_loops(&repo),
                        myman_tasks,
                        file_missing,
                    )
                })
                .await;
            this.update(cx, |this, cx| {
                this.file_missing = file_missing;
                match result {
                    Ok(board) => {
                        this.board = board;
                        this.error = None;
                    }
                    Err(error) => this.error = Some(format!("{error:#}")),
                }
                this.loops = loops;
                this.myman_tasks = myman_tasks;
                let context = crate::insights::todo_context(
                    &this.board,
                    &this.loops,
                    &this.myman_tasks,
                    &this.relative,
                    this.error.is_some(),
                );
                this.insight
                    .update(cx, |insight, cx| insight.set_context(context, cx));
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
                    let _serialised = myman::TASK_LOCK
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if add {
                        let text = read_board_text(&board)?;
                        let output = open_loops::insert_board_line(&text, &task.board_line(), true);
                        write_board_text(&board, &output)?;
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
                            .weight(gpui::FontWeight::SEMIBOLD),
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

    /// Applies one edit to the board file on a background thread, then
    /// reloads it.
    fn edit_board(
        &mut self,
        edit: impl FnOnce(&str) -> Result<String> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let path = self.path.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let _serialised = BOARD_LOCK
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let text = read_board_text(&path)?;
                    write_board_text(&path, &edit(&text)?)?;
                    load_board(&path)
                })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(board) => {
                        this.board = board;
                        this.error = None;
                        this.file_missing = false;
                    }
                    Err(error) => {
                        log::error!("brainz to-do: {error:#}");
                        this.error = Some(format!("{error:#}"));
                        this.refresh(cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn toggle(&mut self, item: &TodoItem, cx: &mut Context<Self>) {
        let line = item.line;
        let raw = item.raw.clone();
        self.edit_board(move |text| toggle_line(text, line, &raw), cx);
    }

    fn set_priority(
        &mut self,
        item: &TodoItem,
        priority: Option<Priority>,
        cx: &mut Context<Self>,
    ) {
        let line = item.line;
        let raw = item.raw.clone();
        let new_line = item.line_with(priority, &item.title);
        self.edit_board(move |text| replace_line(text, line, &raw, &new_line), cx);
    }

    fn move_item(&mut self, dragged: &DraggedTodo, target: DropTarget, cx: &mut Context<Self>) {
        if matches!(&target, DropTarget::Before { line, .. } if *line == dragged.line) {
            return;
        }
        let from = dragged.line;
        let raw = dragged.raw.clone();
        self.edit_board(move |text| move_line(text, from, &raw, &target), cx);
    }

    fn input_editor(
        text: &str,
        placeholder: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Entity<Editor>, Subscription) {
        let editor = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_placeholder_text(placeholder, window, cx);
            editor.set_text(text, window, cx);
            editor.select_all(&editor::actions::SelectAll, window, cx);
            editor
        });
        window.focus(&editor.focus_handle(cx), cx);
        let subscription = cx.subscribe_in(&editor, window, |this, editor, event, window, cx| {
            if matches!(event, EditorEvent::Blurred)
                && this
                    .input
                    .as_ref()
                    .is_some_and(|input| input.editor() == editor)
            {
                this.commit_input(false, window, cx);
            }
        });
        (editor, subscription)
    }

    fn start_edit(&mut self, item: &TodoItem, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_input(false, window, cx);
        let (editor, subscription) = Self::input_editor(&item.title, "To-do", window, cx);
        self.input = Some(InlineInput::Edit {
            item: item.clone(),
            editor,
            _subscription: subscription,
        });
        cx.notify();
    }

    fn start_add(&mut self, heading: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_input(false, window, cx);
        let (editor, subscription) =
            Self::input_editor("", "What needs doing, and by when?", window, cx);
        self.input = Some(InlineInput::Add {
            heading,
            editor,
            _subscription: subscription,
        });
        cx.notify();
    }

    /// Saves the inline input. Enter keeps an add row open for the next
    /// item; clicking away closes it.
    fn commit_input(&mut self, keep_adding: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.input.take() else {
            return;
        };
        let text = input.editor().read(cx).text(cx).trim().to_owned();
        match input {
            InlineInput::Edit { item, .. } => {
                if !text.is_empty() && text != item.title {
                    let new_line = item.line_with(item.priority, &text);
                    let line = item.line;
                    let raw = item.raw;
                    self.edit_board(move |board| replace_line(board, line, &raw, &new_line), cx);
                }
            }
            InlineInput::Add { heading, .. } => {
                if !text.is_empty() {
                    let new_line = format!(
                        "- [ ] {}",
                        text.split_whitespace().collect::<Vec<_>>().join(" ")
                    );
                    self.edit_board(
                        move |board| insert_into_section(board, heading, &new_line),
                        cx,
                    );
                }
                if keep_adding && !text.is_empty() {
                    // The heading line stays put: the new item lands below it.
                    self.start_add(heading, window, cx);
                }
            }
        }
        cx.notify();
    }

    fn cancel_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.input.take().is_some() {
            window.focus(&self.focus_handle, cx);
            cx.notify();
        } else {
            cx.propagate();
        }
    }

    fn render_input(&self, editor: &Entity<Editor>, cx: &mut Context<Self>) -> gpui::AnyElement {
        div()
            .flex_1()
            .min_w_0()
            .px_1p5()
            .py_0p5()
            .rounded_md()
            .border_1()
            .border_color(cx.theme().colors().border_focused)
            .bg(cx.theme().colors().editor_background)
            .child(editor.clone())
            .into_any_element()
    }

    fn render_priority(&self, ix: usize, item: &TodoItem, cx: &mut Context<Self>) -> AnyElement {
        let view = cx.entity().downgrade();
        let current = item.priority;
        let menu = PopoverMenu::new(("brainz-todo-priority-menu", ix))
            .anchor(gpui::Anchor::TopRight)
            .menu({
                let item = item.clone();
                move |window, cx| {
                    let view = view.clone();
                    let item = item.clone();
                    Some(ContextMenu::build(window, cx, move |mut menu, _, _| {
                        menu = menu.header("Priority");
                        for choice in Priority::ALL.map(Some).into_iter().chain([None]) {
                            let view = view.clone();
                            let item = item.clone();
                            menu = menu.toggleable_entry(
                                choice.map_or("None", Priority::label),
                                current == choice,
                                IconPosition::Start,
                                None,
                                move |_, cx| {
                                    view.update(cx, |view, cx| {
                                        view.set_priority(&item, choice, cx)
                                    })
                                    .ok();
                                },
                            );
                        }
                        menu
                    }))
                }
            });
        match current {
            Some(priority) => menu
                .trigger_with_tooltip(
                    Button::new(("brainz-todo-priority", ix), priority.label())
                        .label_size(LabelSize::Small)
                        .color(priority.color()),
                    Tooltip::text("Change priority"),
                )
                .into_any_element(),
            None => div()
                .visible_on_hover("brainz-todo-row")
                .child(
                    menu.trigger_with_tooltip(
                        IconButton::new(("brainz-todo-priority", ix), IconName::SignalHigh)
                            .icon_size(IconSize::Small)
                            .icon_color(Color::Placeholder),
                        Tooltip::text("Set priority"),
                    ),
                )
                .into_any_element(),
        }
    }

    fn render_item(&self, ix: usize, item: &TodoItem, cx: &mut Context<Self>) -> impl IntoElement {
        let icon = if item.done {
            IconName::BrainzCheckboxChecked
        } else {
            IconName::BrainzCheckboxEmpty
        };
        let editing = match &self.input {
            Some(InlineInput::Edit {
                item: edited,
                editor,
                ..
            }) if edited.line == item.line => Some(editor.clone()),
            _ => None,
        };
        let dragged = DraggedTodo {
            line: item.line,
            raw: item.raw.clone(),
            title: item.title.clone(),
        };
        let target = DropTarget::Before {
            line: item.line,
            raw: item.raw.clone(),
        };
        let drop_color = cx.theme().colors().drop_target_border;
        let title = {
            let item = item.clone();
            div()
                .id(("brainz-todo-title", ix))
                .min_w_0()
                .flex_1()
                .cursor_text()
                .child(
                    Label::new(item.title.clone())
                        .color(if item.done {
                            Color::Accent
                        } else {
                            Color::Default
                        })
                        .when(item.done, |label| label.strikethrough()),
                )
                .tooltip(Tooltip::text("Click to edit · drag to reorder"))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.start_edit(&item, window, cx);
                }))
        };
        h_flex()
            .id(("brainz-todo-item", ix))
            .group("brainz-todo-row")
            .w_full()
            .items_start()
            .gap_2p5()
            .px_2()
            .py_1p5()
            .rounded_md()
            .border_t_2()
            .border_color(gpui::transparent_black())
            .hover(|this| this.bg(cx.theme().colors().element_hover))
            .when(editing.is_none(), |this| {
                this.cursor_grab()
                    .on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
            })
            .drag_over::<DraggedTodo>(move |style, _, _, _| style.border_color(drop_color))
            .on_drop(cx.listener(move |this, dragged: &DraggedTodo, _, cx| {
                this.move_item(dragged, target.clone(), cx);
            }))
            .child({
                let item = item.clone();
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
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle(&item, cx)))
            })
            .child(
                v_flex()
                    .min_w_0()
                    .flex_1()
                    .child(match editing {
                        Some(editor) => self.render_input(&editor, cx),
                        None => title.into_any_element(),
                    })
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
            .child(self.render_priority(ix, item, cx))
    }

    fn render_add_row(&self, section: &TodoSection, cx: &mut Context<Self>) -> AnyElement {
        let heading = section.line;
        let adding = match &self.input {
            Some(InlineInput::Add {
                heading: open,
                editor,
                ..
            }) if *open == heading => Some(editor.clone()),
            _ => None,
        };
        let drop_color = cx.theme().colors().drop_target_border;
        let row = h_flex()
            .id(("brainz-todo-add", heading))
            .w_full()
            .items_center()
            .gap_2p5()
            .px_2()
            .py_1()
            .rounded_md()
            .border_t_2()
            .border_color(gpui::transparent_black())
            .drag_over::<DraggedTodo>(move |style, _, _, _| style.border_color(drop_color))
            .on_drop(cx.listener(move |this, dragged: &DraggedTodo, _, cx| {
                this.move_item(dragged, DropTarget::EndOfSection { line: heading }, cx);
            }));
        match adding {
            Some(editor) => row
                .child(
                    Icon::new(IconName::BrainzCheckboxEmpty)
                        .size(IconSize::Small)
                        .color(Color::Placeholder),
                )
                .child(self.render_input(&editor, cx))
                .into_any_element(),
            None => row
                .cursor_pointer()
                .hover(|this| this.bg(cx.theme().colors().element_hover))
                .child(
                    Icon::new(IconName::Plus)
                        .size(IconSize::Small)
                        .color(Color::Placeholder),
                )
                .child(
                    Label::new("Add a to-do")
                        .size(LabelSize::Small)
                        .color(Color::Placeholder),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.start_add(heading, window, cx);
                }))
                .into_any_element(),
        }
    }

    /// The section a header "New to-do" adds to: Today when there is one,
    /// else the first section that is not Done.
    fn default_section(&self) -> Option<usize> {
        let open = |section: &&TodoSection| !section.name.eq_ignore_ascii_case("done");
        self.board
            .sections
            .iter()
            .find(|section| section.name.to_ascii_lowercase().starts_with("today"))
            .or_else(|| self.board.sections.iter().find(open))
            .map(|section| section.line)
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
        let done_count: usize = self
            .board
            .sections
            .iter()
            .flat_map(|section| section.items.iter())
            .filter(|item| item.done)
            .count();
        let default_section = self.default_section();
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
                        Button::new(
                            "brainz-todo-show-done",
                            if self.show_done {
                                "Hide done".to_owned()
                            } else {
                                format!("Show done ({done_count})")
                            },
                        )
                        .label_size(LabelSize::Small)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_done = !this.show_done;
                            cx.notify();
                        })),
                    )
                    .when_some(default_section, |this, heading| {
                        this.child(
                            Button::new("brainz-todo-new", "New to-do")
                                .label_size(LabelSize::Small)
                                .style(ButtonStyle::Filled)
                                .start_icon(Icon::new(IconName::Plus).size(IconSize::Small))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.start_add(heading, window, cx);
                                })),
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
        body.push(self.insight.clone().into_any_element());
        if self.file_missing && self.error.is_none() {
            body.push(
                div()
                    .px_2()
                    .pt_2()
                    .child(
                        Label::new(format!(
                            "Add your first to-do below. Brainz keeps the list in {} in your brain.",
                            self.relative
                        ))
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                    )
                    .into_any_element(),
            );
        }
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
        let drop_color = cx.theme().colors().drop_target_background;
        for section in &self.board.sections {
            let is_done = section.name.eq_ignore_ascii_case("done");
            let heading = section.line;
            let visible: Vec<&TodoItem> = section
                .items
                .iter()
                .filter(|item| self.show_done || !item.done)
                .collect();
            let hidden = section.items.len() - visible.len();
            let mut block = v_flex().w_full().gap_0p5().child(
                h_flex()
                    .id(("brainz-todo-section", heading))
                    .px_2()
                    .pt_3()
                    .pb_1()
                    .gap_2()
                    .rounded_md()
                    .drag_over::<DraggedTodo>(move |style, _, _, _| style.bg(drop_color))
                    .on_drop(cx.listener(move |this, dragged: &DraggedTodo, _, cx| {
                        this.move_item(dragged, DropTarget::EndOfSection { line: heading }, cx);
                    }))
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
                        Label::new(visible.len().to_string())
                            .size(LabelSize::Small)
                            .color(Color::Placeholder),
                    )
                    .when(hidden > 0 && !is_done, |this| {
                        this.child(
                            Label::new(format!("{hidden} done hidden"))
                                .size(LabelSize::Small)
                                .color(Color::Placeholder),
                        )
                    }),
            );
            if is_done && !self.show_done {
                body.push(block.into_any_element());
                continue;
            }
            if visible.is_empty() && hidden == 0 && (is_done || section.empty_note.is_some()) {
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
            for item in visible {
                block = block.child(ui::reveal(
                    ("brainz-todo-item-reveal", item_ix),
                    item_ix,
                    self.render_item(item_ix, item, cx),
                ));
                item_ix += 1;
            }
            if !is_done {
                block = block.child(self.render_add_row(section, cx));
            }
            body.push(block.into_any_element());
        }
        body.push(self.render_loops(cx));
        body.extend(self.render_myman_tasks(cx));

        v_flex()
            .id("brainz-todo")
            .key_context("BrainzTodo")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &menu::Confirm, window, cx| {
                this.commit_input(true, window, cx);
            }))
            .on_action(cx.listener(|this, _: &menu::Cancel, window, cx| {
                this.cancel_input(window, cx);
            }))
            // A single-line editor binds Escape to its own Cancel and
            // propagates it when it has nothing to dismiss.
            .on_action(
                cx.listener(|this, _: &editor::actions::Cancel, window, cx| {
                    this.cancel_input(window, cx);
                }),
            )
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

#[cfg(test)]
mod tests {
    use super::*;

    const BOARD: &str = "# TODO\n\n## Today\n\n- [ ] Call the dentist `dentist`\n- [ ] [P1] Send the deck (due Fri)\n\n## Delayed / Waiting\n\n_(nothing waiting)_\n\n## Done\n\n- [x] Book the room (2026-10-01)\n";

    fn item(text: &str, title: &str) -> TodoItem {
        parse_board(text)
            .sections
            .into_iter()
            .flat_map(|section| section.items)
            .find(|item| item.title == title)
            .expect("item on the board")
    }

    #[test]
    fn parses_priority_tag_and_note() {
        let deck = item(BOARD, "Send the deck");
        assert_eq!(deck.priority, Some(Priority::High));
        assert_eq!(deck.note.as_deref(), Some("due Fri"));
        let dentist = item(BOARD, "Call the dentist");
        assert_eq!(dentist.priority, None);
        assert_eq!(dentist.tag.as_deref(), Some("dentist"));
    }

    #[test]
    fn line_with_keeps_tag_note_and_checkbox() {
        let deck = item(BOARD, "Send the deck");
        assert_eq!(
            deck.line_with(Some(Priority::Low), "Send  the final deck"),
            "- [ ] [P3] Send the final deck (due Fri)"
        );
        let room = item(BOARD, "Book the room");
        assert_eq!(
            room.line_with(None, "Book the big room"),
            "- [x] Book the big room (2026-10-01)"
        );
        let dentist = item(BOARD, "Call the dentist");
        assert_eq!(
            dentist.line_with(Some(Priority::Medium), &dentist.title),
            "- [ ] [P2] Call the dentist `dentist`"
        );
    }

    #[test]
    fn replace_line_refuses_a_stale_board() {
        let deck = item(BOARD, "Send the deck");
        assert!(replace_line(BOARD, deck.line, "- [ ] something else", "x").is_err());
        let output = replace_line(BOARD, deck.line, &deck.raw, "- [ ] Send it").unwrap();
        assert!(output.contains("- [ ] Send it\n"));
        assert!(output.ends_with('\n'));
    }

    #[test]
    fn moves_an_item_above_another_and_into_an_empty_section() {
        let dentist = item(BOARD, "Call the dentist");
        let deck = item(BOARD, "Send the deck");
        let reordered = move_line(
            BOARD,
            deck.line,
            &deck.raw,
            &DropTarget::Before {
                line: dentist.line,
                raw: dentist.raw.clone(),
            },
        )
        .unwrap();
        let today = &parse_board(&reordered).sections[0];
        assert_eq!(today.items[0].title, "Send the deck");
        assert_eq!(today.items[1].title, "Call the dentist");

        let waiting = parse_board(BOARD).sections[1].line;
        let moved = move_line(
            BOARD,
            dentist.line,
            &dentist.raw,
            &DropTarget::EndOfSection { line: waiting },
        )
        .unwrap();
        let board = parse_board(&moved);
        assert_eq!(board.sections[0].items.len(), 1);
        assert_eq!(board.sections[1].items[0].title, "Call the dentist");
        assert!(!moved.contains("nothing waiting"));
    }

    #[test]
    fn moves_an_item_down_to_the_end_of_its_own_section() {
        let dentist = item(BOARD, "Call the dentist");
        let today = parse_board(BOARD).sections[0].line;
        let moved = move_line(
            BOARD,
            dentist.line,
            &dentist.raw,
            &DropTarget::EndOfSection { line: today },
        )
        .unwrap();
        let today = &parse_board(&moved).sections[0];
        assert_eq!(today.items[0].title, "Send the deck");
        assert_eq!(today.items[1].title, "Call the dentist");
    }

    #[test]
    fn inserts_new_items_at_the_end_of_a_section() {
        let board = parse_board(BOARD);
        let output =
            insert_into_section(BOARD, board.sections[0].line, "- [ ] Water the plants").unwrap();
        let today = &parse_board(&output).sections[0];
        assert_eq!(today.items.last().unwrap().title, "Water the plants");

        let bare = "## Today\n## Done\n";
        let output = insert_into_section(bare, 0, "- [ ] First").unwrap();
        assert_eq!(output, "## Today\n\n- [ ] First\n## Done\n");
    }

    #[test]
    fn a_missing_board_starts_from_the_starter_sections() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("ops/desk/TODO.md");
        let board = load_board(&path).unwrap();
        let names: Vec<_> = board
            .sections
            .iter()
            .map(|section| section.name.as_str())
            .collect();
        assert_eq!(names, ["Today", "Waiting", "Done"]);

        let text = read_board_text(&path).unwrap();
        let output =
            insert_into_section(&text, board.sections[0].line, "- [ ] First thing").unwrap();
        write_board_text(&path, &output).unwrap();
        let board = load_board(&path).unwrap();
        assert_eq!(board.sections[0].items[0].title, "First thing");
        assert!(board.sections[1].items.is_empty());

        let waiting = open_loops::insert_board_line(STARTER_BOARD, "- [ ] Hear back", false);
        assert_eq!(
            parse_board(&waiting).sections[1].items[0].title,
            "Hear back"
        );
    }

    #[test]
    fn toggle_checks_and_unchecks_in_place() {
        let dentist = item(BOARD, "Call the dentist");
        let checked = toggle_line(BOARD, dentist.line, &dentist.raw).unwrap();
        let dentist = item(&checked, "Call the dentist");
        assert!(dentist.done);
        let unchecked = toggle_line(&checked, dentist.line, &dentist.raw).unwrap();
        assert!(!item(&unchecked, "Call the dentist").done);
    }
}
