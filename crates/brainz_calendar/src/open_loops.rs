//! Brainz: the Open loops tab. Every current ⏳ and ⏰ line in the brain,
//! grouped by who owes it, oldest first, from the signals pass. Brainz
//! opens the file at the line; striking a loop is the owner's edit.

use std::{path::PathBuf, time::Duration};

use chrono::Local;
use editor::Editor;
use gpui::{App, EventEmitter, FocusHandle, Focusable, Task, WeakEntity, Window, actions};
use ui::{Tooltip, prelude::*};
use workspace::{
    HideStatusItem, Item, ItemHandle, OpenOptions, OpenVisible, StatusItemView, Workspace,
};

use crate::{
    brain_config::BrainConfig,
    themes::runner,
    themes_signals::{self as signals, OpenLoop, Signals},
};

actions!(
    brainz_open_loops,
    [
        /// Opens the Open loops tab.
        OpenOpenLoops
    ]
);

const DISK_POLL: Duration = Duration::from_secs(10);

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &OpenOpenLoops, window, cx| {
            OpenLoopsView::open(workspace, window, cx);
        });
    })
    .detach();
}

pub struct OpenLoopsView {
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    repo: PathBuf,
    config: BrainConfig,
    signals: Option<Signals>,
    error: Option<String>,
    signals_modified: Option<std::time::SystemTime>,
    _disk_poll: Task<()>,
    _load: Option<Task<()>>,
}

impl OpenLoopsView {
    pub fn open(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
        let existing = workspace.items_of_type::<OpenLoopsView>(cx).next();
        if let Some(existing) = existing {
            workspace.activate_item(&existing, true, true, window, cx);
            existing.update(cx, |view, cx| view.reload(cx));
            return;
        }
        let Some(repo) = workspace.root_paths(cx).first().map(|path| path.to_path_buf()) else {
            return;
        };
        let weak = cx.entity().downgrade();
        let view = cx.new(|cx| OpenLoopsView::new(weak, repo, cx));
        workspace.add_item_to_active_pane(Box::new(view), None, true, window, cx);
    }

    fn new(workspace: WeakEntity<Workspace>, repo: PathBuf, cx: &mut Context<Self>) -> Self {
        let disk_poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(DISK_POLL).await;
                let alive = this.update(cx, |this, cx| {
                    let modified = this.signals_file_modified();
                    if modified != this.signals_modified {
                        this.signals_modified = modified;
                        this.reload(cx);
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        });
        let config = BrainConfig::load(&repo);
        let mut this = Self {
            focus_handle: cx.focus_handle(),
            workspace,
            signals_modified: None,
            repo,
            config,
            signals: None,
            error: None,
            _disk_poll: disk_poll,
            _load: None,
        };
        this.signals_modified = this.signals_file_modified();
        this.reload(cx);
        this
    }

    fn signals_file_modified(&self) -> Option<std::time::SystemTime> {
        std::fs::metadata(self.config.themes_path(&self.repo, signals::SIGNALS_NAME))
            .ok()?
            .modified()
            .ok()
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let repo = self.repo.clone();
        let config = self.config.clone();
        self._load = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { signals::load_signals(&repo, &config) })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(signals) => {
                        // A file from an older Brainz lacks the newer fields;
                        // refresh it once rather than show stale shapes.
                        if signals.schema < signals::SCHEMA {
                            this.run_now(cx);
                        }
                        this.signals = Some(signals);
                        this.error = None;
                    }
                    Err(error) => {
                        if this.signals.is_none() {
                            this.error = Some(format!("{error:#}"));
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn run_now(&mut self, cx: &mut Context<Self>) {
        let repo = self.repo.clone();
        if let Some(runner) = runner(cx) {
            runner.update(cx, |runner, cx| runner.run(repo, cx));
        }
    }

    /// Opens the loop's file with the cursor on its line.
    fn open_loop(&self, open_loop: &OpenLoop, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.repo.join(&open_loop.file);
        if !path.is_file() {
            return;
        }
        let row = open_loop.line.saturating_sub(1);
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        let task = workspace.update(cx, |workspace, cx| {
            workspace.open_abs_path(
                path,
                OpenOptions {
                    visible: Some(OpenVisible::None),
                    ..Default::default()
                },
                window,
                cx,
            )
        });
        cx.spawn_in(window, async move |_, cx| {
            let item = task.await?;
            if let Some(editor) = item.downcast::<Editor>() {
                editor.update_in(cx, |editor, window, cx| {
                    if let Some(buffer) = editor.buffer().read(cx).as_singleton() {
                        let point = buffer
                            .read(cx)
                            .snapshot()
                            .point_from_external_input(row, 0);
                        editor.go_to_singleton_buffer_point(point, window, cx);
                    }
                })?;
            }
            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
    }

    fn render_group(
        &self,
        title: String,
        loops: &[OpenLoop],
        ix_base: usize,
        today: chrono::NaiveDate,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut block = v_flex().w_full().child(
            h_flex()
                .px_1()
                .pt_4()
                .pb_1()
                .gap_2()
                .child(
                    Label::new(title)
                        .size(LabelSize::Small)
                        .weight(gpui::FontWeight::SEMIBOLD)
                        .color(Color::Accent),
                )
                .child(
                    Label::new(loops.len().to_string())
                        .size(LabelSize::XSmall)
                        .color(Color::Placeholder),
                ),
        );
        if loops.is_empty() {
            block = block.child(
                div().px_2().py_1().child(
                    Label::new("Nothing open")
                        .size(LabelSize::Small)
                        .color(Color::Placeholder),
                ),
            );
        }
        for (ix, open_loop) in loops.iter().enumerate() {
            let age = open_loop.age_days(today);
            let age_label = match age {
                0 => "today".to_owned(),
                1 => "1 day".to_owned(),
                days => format!("{days} days"),
            };
            let who = open_loop
                .counterparty
                .clone()
                .unwrap_or_else(|| "—".to_owned());
            let folder = open_loop.folder.clone();
            let location = format!("{}:{}", open_loop.file, open_loop.line);
            let row_loop = open_loop.clone();
            let done_loop = open_loop.clone();
            block = block.child(
                h_flex()
                    .id(("brainz-open-loop", ix_base + ix))
                    .w_full()
                    .items_center()
                    .gap_2()
                    .px_1()
                    .py_1()
                    .rounded_md()
                    .hover(|this| this.bg(cx.theme().colors().element_hover))
                    .cursor_pointer()
                    .tooltip(Tooltip::text(location))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_loop(&row_loop, window, cx);
                    }))
                    .child(
                        div()
                            .w(px(22.))
                            .flex_none()
                            .child(Label::new(open_loop.marker.clone()).size(LabelSize::Small)),
                    )
                    .child(
                        div().w(px(150.)).flex_none().overflow_hidden().child(
                            Label::new(who)
                                .size(LabelSize::Small)
                                .color(if open_loop.counterparty.is_some() {
                                    Color::Default
                                } else {
                                    Color::Placeholder
                                })
                                .truncate(),
                        ),
                    )
                    .child(
                        div().w(px(120.)).flex_none().overflow_hidden().child(
                            Label::new(folder)
                                .size(LabelSize::XSmall)
                                .color(Color::Muted)
                                .truncate(),
                        ),
                    )
                    .child(
                        div().w(px(64.)).flex_none().child(
                            Label::new(age_label)
                                .size(LabelSize::XSmall)
                                .color(if age >= 14 { Color::Accent } else { Color::Muted }),
                        ),
                    )
                    .child(
                        div().flex_1().min_w_0().child(
                            Label::new(open_loop.text.clone())
                                .size(LabelSize::Small)
                                .truncate(),
                        ),
                    )
                    .child(
                        Button::new(("brainz-open-loop-done", ix_base + ix), "Mark done")
                            .label_size(LabelSize::XSmall)
                            .color(Color::Muted)
                            .tooltip(Tooltip::text(
                                "Opens the file at this line so you can strike it yourself",
                            ))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.open_loop(&done_loop, window, cx);
                            })),
                    ),
            );
        }
        block.into_any_element()
    }
}

impl EventEmitter<()> for OpenLoopsView {}

impl Focusable for OpenLoopsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Item for OpenLoopsView {
    type Event = ();

    fn to_item_events(_: &Self::Event, _: &mut dyn FnMut(workspace::item::ItemEvent)) {}

    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        "Open loops".into()
    }

    fn tab_icon(&self, _window: &Window, _cx: &App) -> Option<Icon> {
        Some(Icon::new(IconName::BrainzLoops))
    }

    fn telemetry_event_text(&self) -> Option<&'static str> {
        None
    }
}

impl Render for OpenLoopsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let today = Local::now().date_naive();
        let owner = self.config.owner_label();
        let running = runner(cx).is_some_and(|runner| runner.read(cx).is_running());
        let summary = self
            .signals
            .as_ref()
            .map(|signals| signals::open_loops_summary(&signals.open_loops, &owner, today))
            .unwrap_or_default();
        let header = h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .child(
                h_flex()
                    .gap_2()
                    .child(Icon::new(IconName::BrainzLoops).color(Color::Accent))
                    .child(Label::new("Open loops").size(LabelSize::Large))
                    .child(
                        Label::new(summary)
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    ),
            )
            .child(if running {
                h_flex()
                    .h(px(24.))
                    .px_3()
                    .items_center()
                    .rounded_md()
                    .border_1()
                    .border_color(cx.theme().colors().border)
                    .child(ui::bouncing_dots(
                        "brainz-open-loops-running",
                        cx.theme().colors().text_accent,
                    ))
                    .into_any_element()
            } else {
                IconButton::new("brainz-open-loops-refresh", IconName::ArrowCircle)
                    .icon_size(IconSize::Small)
                    .tooltip(Tooltip::text("Re-scan the brain for ⏳ and ⏰ lines"))
                    .on_click(cx.listener(|this, _, _, cx| this.run_now(cx)))
                    .into_any_element()
            });

        let mut body: Vec<AnyElement> = Vec::new();
        if let Some(error) = &self.error {
            body.push(
                v_flex()
                    .mt_3()
                    .p_4()
                    .gap_2()
                    .rounded_lg()
                    .border_1()
                    .border_color(cx.theme().colors().border)
                    .bg(cx.theme().colors().surface_background)
                    .child(Label::new(error.clone()))
                    .child(
                        Label::new("Run the Themes sync once to produce the signals file.")
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .into_any_element(),
            );
        }
        if let Some(signals) = self.signals.clone() {
            let owed: Vec<OpenLoop> = signals
                .open_loops
                .iter()
                .filter(|l| l.owed_by_owner())
                .cloned()
                .collect();
            let waiting: Vec<OpenLoop> = signals
                .open_loops
                .iter()
                .filter(|l| !l.owed_by_owner())
                .cloned()
                .collect();
            body.push(self.render_group(format!("⏰ {owner} owes"), &owed, 0, today, cx));
            body.push(self.render_group("⏳ Waiting on someone else".to_owned(), &waiting, 10_000, today, cx));
        }

        v_flex()
            .id("brainz-open-loops")
            .key_context("BrainzOpenLoops")
            .track_focus(&self.focus_handle)
            .size_full()
            .overflow_y_scroll()
            .bg(cx.theme().colors().editor_background)
            .child(
                v_flex()
                    .w_full()
                    .max_w(px(1240.))
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

/// Status bar button that opens the Open loops tab.
pub struct OpenLoopsButton {
    pane_item_focus_handle: Option<FocusHandle>,
    active: bool,
}

impl OpenLoopsButton {
    pub fn new() -> Self {
        Self {
            pane_item_focus_handle: None,
            active: false,
        }
    }
}

impl Render for OpenLoopsButton {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let focus_handle = self.pane_item_focus_handle.clone();
        let active = self.active;
        div().child(
            IconButton::new("brainz-open-loops-button", IconName::BrainzLoops)
                .icon_size(IconSize::Small)
                .toggle_state(active)
                .icon_color(if active { Color::Accent } else { Color::Default })
                .tooltip(move |_window, cx| {
                    if let Some(focus_handle) = &focus_handle {
                        Tooltip::for_action_in("Open loops", &OpenOpenLoops, focus_handle, cx)
                    } else {
                        Tooltip::for_action("Open loops", &OpenOpenLoops, cx)
                    }
                })
                .on_click(|_, window, cx| {
                    window.dispatch_action(Box::new(OpenOpenLoops), cx);
                }),
        )
    }
}

impl StatusItemView for OpenLoopsButton {
    fn set_active_pane_item(
        &mut self,
        active_pane_item: Option<&dyn ItemHandle>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pane_item_focus_handle = active_pane_item.map(|item| item.item_focus_handle(cx));
        self.active =
            active_pane_item.is_some_and(|item| item.downcast::<OpenLoopsView>().is_some());
        cx.notify();
    }

    fn hide_setting(&self, _: &App) -> Option<HideStatusItem> {
        None
    }
}
