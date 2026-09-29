//! Brainz: a To-Do tab over `ops/desk/TODO.md`, the check-off board that
//! TodoBot maintains in the brain. Brainz only flips checkboxes; TodoBot
//! keeps the counts, Notion mirror, and wording in order.

use std::{path::PathBuf, time::Duration};

use anyhow::{Context as _, Result};
use gpui::{App, EventEmitter, FocusHandle, Focusable, Task, Window, actions};
use ui::{Tooltip, prelude::*};
use workspace::{HideStatusItem, Item, ItemHandle, StatusItemView, Workspace};

actions!(
    brainz_todo,
    [
        /// Opens the To-Do tab.
        OpenTodo
    ]
);

const TODO_RELATIVE_PATH: &str = "ops/desk/TODO.md";
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
struct TodoItem {
    /// Line index in the file, so toggles edit the right line.
    line: usize,
    done: bool,
    title: String,
    tag: Option<String>,
    note: Option<String>,
}

#[derive(Debug, Clone)]
struct TodoSection {
    name: String,
    items: Vec<TodoItem>,
    empty_note: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct TodoBoard {
    sections: Vec<TodoSection>,
    updated: Option<String>,
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

fn parse_board(text: &str) -> TodoBoard {
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
            } else if let Some(note) = line.trim().strip_prefix('_').and_then(|s| s.strip_suffix('_')) {
                section.empty_note = Some(note.trim_matches(|c| c == '(' || c == ')').to_owned());
            }
        }
    }
    board
}

fn load_board(path: &PathBuf) -> Result<TodoBoard> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    Ok(parse_board(&text))
}

/// Flips one checkbox in place. Checking appends today's date; the item
/// stays in its section so it reads as done rather than vanishing. TodoBot
/// tidies it into "## Done" on its next pass.
fn toggle_item(path: &PathBuf, line_index: usize) -> Result<()> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
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
    path: PathBuf,
    board: TodoBoard,
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
        let Some(root) = workspace.root_paths(cx).first().map(|path| path.to_path_buf()) else {
            return;
        };
        let path = root.join(TODO_RELATIVE_PATH);
        let view = cx.new(|cx| TodoView::new(path, cx));
        workspace.add_item_to_active_pane(Box::new(view), None, true, window, cx);
    }

    fn new(path: PathBuf, cx: &mut Context<Self>) -> Self {
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
            path,
            board: TodoBoard::default(),
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
        self._load = Some(cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { load_board(&path) }).await;
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
                                        Label::new(note)
                                            .size(LabelSize::XSmall)
                                            .color(Color::Muted),
                                    )
                                })
                                .when_some(item.tag.clone(), |this, tag| {
                                    this.child(
                                        Label::new(tag)
                                            .size(LabelSize::XSmall)
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
                    .child(Icon::new(IconName::BrainzCheckboxChecked).color(Color::Accent))
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
                            "Brainz looks for {TODO_RELATIVE_PATH} in the open project."
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
                            .color(if is_done { Color::Muted } else { Color::Accent }),
                    )
                    .child(
                        Label::new(section.items.len().to_string())
                            .size(LabelSize::XSmall)
                            .color(Color::Placeholder),
                    )
                    .when(is_done, |this| {
                        this.child(
                            Button::new("brainz-todo-toggle-done", if self.show_done {
                                "Hide"
                            } else {
                                "Show"
                            })
                            .label_size(LabelSize::XSmall)
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
                        Label::new(section.empty_note.clone().unwrap_or_else(|| "Nothing here".into()))
                            .size(LabelSize::Small)
                            .color(Color::Placeholder),
                    ),
                );
            }
            for item in &section.items {
                block = block.child(self.render_item(item_ix, item, cx));
                item_ix += 1;
            }
            body.push(block.into_any_element());
        }

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
                .icon_color(if active { Color::Accent } else { Color::Default })
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
