//! Brainz: the Brief tab. One page for the start of the day: today's
//! calendar events that match a folder in the brain (with their prep
//! files), folders whose status callout has gone stale, and the oldest
//! open loops. Everything comes from data Brainz already computes; no
//! agent is involved.

use std::{path::PathBuf, time::Duration};

use chrono::{DateTime, Local, Timelike};
use gpui::{App, EventEmitter, FocusHandle, Focusable, Task, WeakEntity, Window, actions};
use ui::{Tooltip, prelude::*};
use workspace::{
    HideStatusItem, Item, ItemHandle, OpenOptions, OpenVisible, StatusItemView, Workspace,
};

use crate::{
    LoadState,
    brain_config::BrainConfig,
    brain_match::BrainIndex,
    status_decay::{self, Decay},
    themes_signals::{self as signals, OpenLoop},
};

actions!(
    brainz_brief,
    [
        /// Opens the Brief tab.
        OpenBrief
    ]
);

const REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);
const LOOP_LIMIT: usize = 10;

#[derive(Debug, Clone, PartialEq)]
pub struct BriefEvent {
    pub title: String,
    pub start: DateTime<Local>,
    pub all_day: bool,
    pub who: Option<String>,
    pub folder: Option<String>,
    pub prep_file: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Brief {
    pub generated_at: Option<DateTime<Local>>,
    pub events: Vec<BriefEvent>,
    pub calendar_problem: Option<String>,
    pub stale: Vec<Decay>,
    pub loops: Vec<OpenLoop>,
    pub loops_total: usize,
    pub owner: String,
}

/// Builds the brief for `today`. Blocking file and process I/O: run on the
/// background executor.
pub fn build(repo: &std::path::Path, now: DateTime<Local>) -> Brief {
    let config = BrainConfig::load(repo);
    let today = now.date_naive();
    let index = BrainIndex::load(repo, &config);

    let mut events = Vec::new();
    let mut calendar_problem = None;
    match crate::load_events() {
        Ok((LoadState::Ready, all)) => {
            for event in all
                .into_iter()
                .filter(|event| event.start.date_naive() == today || (event.all_day && event.start.date_naive() <= today && today < event.end.date_naive()))
            {
                let found = index.match_event(
                    &event.title,
                    &event.attendee_names,
                    &event.attendee_emails,
                    today,
                );
                events.push(BriefEvent {
                    title: event.title.to_string(),
                    start: event.start,
                    all_day: event.all_day,
                    who: found.as_ref().map(|found| found.who.clone()),
                    folder: found.as_ref().map(|found| found.folder.clone()),
                    prep_file: found.and_then(|found| found.prep_file),
                });
            }
            events.sort_by_key(|event| (!event.all_day, event.start));
        }
        Ok((LoadState::NoAccess(message), _)) | Ok((LoadState::Failed(message), _)) => {
            calendar_problem = Some(message);
        }
        Ok((LoadState::Loading, _)) => {}
        Err(error) => calendar_problem = Some(format!("{error:#}")),
    }

    let mut stale: Vec<Decay> = status_decay::scan(repo, &config, today).into_values().collect();
    stale.sort_by(|a, b| a.folder.cmp(&b.folder));

    let (loops, loops_total) = match signals::load_signals(repo, &config) {
        Ok(signals) => {
            let total = signals.open_loops.len();
            let mut loops = signals.open_loops;
            loops.sort_by(|a, b| a.first_seen.cmp(&b.first_seen));
            loops.truncate(LOOP_LIMIT);
            (loops, total)
        }
        Err(_) => (Vec::new(), 0),
    };

    Brief {
        generated_at: Some(now),
        events,
        calendar_problem,
        stale,
        loops,
        loops_total,
        owner: config.owner_label(),
    }
}

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &OpenBrief, window, cx| {
            BriefView::open(workspace, window, cx);
        });
    })
    .detach();
}

pub struct BriefView {
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    repo: PathBuf,
    brief: Brief,
    loading: bool,
    _refresh_loop: Task<()>,
    _load: Option<Task<()>>,
}

impl BriefView {
    pub fn open(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
        let existing = workspace.items_of_type::<BriefView>(cx).next();
        if let Some(existing) = existing {
            workspace.activate_item(&existing, true, true, window, cx);
            existing.update(cx, |view, cx| view.refresh(cx));
            return;
        }
        let Some(repo) = workspace.root_paths(cx).first().map(|path| path.to_path_buf()) else {
            return;
        };
        let weak = cx.entity().downgrade();
        let view = cx.new(|cx| BriefView::new(weak, repo, cx));
        workspace.add_item_to_active_pane(Box::new(view), None, true, window, cx);
    }

    fn new(workspace: WeakEntity<Workspace>, repo: PathBuf, cx: &mut Context<Self>) -> Self {
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
            brief: Brief::default(),
            loading: true,
            _refresh_loop: refresh_loop,
            _load: None,
        };
        this.refresh(cx);
        this
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let repo = self.repo.clone();
        self.loading = true;
        cx.notify();
        self._load = Some(cx.spawn(async move |this, cx| {
            let brief = cx
                .background_spawn(async move { build(&repo, Local::now()) })
                .await;
            this.update(cx, |this, cx| {
                this.brief = brief;
                this.loading = false;
                cx.notify();
            })
            .ok();
        }));
    }

    fn open_relative(&self, relative: &str, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.repo.join(relative);
        let target = if path.is_dir() {
            ["CLAUDE.md", "README.md"]
                .iter()
                .map(|name| path.join(name))
                .find(|candidate| candidate.is_file())
                .unwrap_or(path)
        } else {
            path
        };
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

    fn section(&self, title: impl Into<SharedString>, count: Option<usize>) -> AnyElement {
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
            .when_some(count, |this, count| {
                this.child(
                    Label::new(count.to_string())
                        .size(LabelSize::XSmall)
                        .color(Color::Placeholder),
                )
            })
            .into_any_element()
    }

    fn empty(&self, text: &'static str) -> AnyElement {
        div()
            .px_2()
            .py_1()
            .child(Label::new(text).size(LabelSize::Small).color(Color::Placeholder))
            .into_any_element()
    }

    fn time_label(event: &BriefEvent) -> String {
        if event.all_day {
            return "All day".to_owned();
        }
        let (_, hour) = event.start.hour12();
        let suffix = if event.start.hour12().0 { "pm" } else { "am" };
        if event.start.minute() == 0 {
            format!("{hour}{suffix}")
        } else {
            format!("{hour}:{:02}{suffix}", event.start.minute())
        }
    }

    fn render_events(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut rows = vec![self.section("Today", Some(self.brief.events.len()))];
        if let Some(problem) = &self.brief.calendar_problem {
            rows.push(
                div()
                    .px_2()
                    .py_1()
                    .child(Label::new(problem.clone()).size(LabelSize::Small).color(Color::Muted))
                    .into_any_element(),
            );
            return rows;
        }
        if self.brief.events.is_empty() {
            rows.push(self.empty("Nothing on the calendar today"));
            return rows;
        }
        let now = Local::now();
        for (ix, event) in self.brief.events.iter().enumerate() {
            let past = !event.all_day && event.start < now;
            let matched = event.folder.is_some();
            let mut row = h_flex()
                .id(("brainz-brief-event", ix))
                .w_full()
                .items_center()
                .gap_2()
                .px_1()
                .py_1()
                .rounded_md()
                .child(
                    div().w(px(64.)).flex_none().child(
                        Label::new(Self::time_label(event))
                            .size(LabelSize::XSmall)
                            .color(if past { Color::Placeholder } else { Color::Muted }),
                    ),
                )
                .child(
                    div().flex_1().min_w_0().child(
                        Label::new(event.title.clone())
                            .size(LabelSize::Small)
                            .color(if past { Color::Muted } else { Color::Default })
                            .truncate(),
                    ),
                );
            if matched {
                let folder = event.folder.clone().unwrap_or_default();
                let who = event.who.clone().unwrap_or_default();
                row = row.child(
                    Label::new(format!("{who} · {}", signals::file_label(&folder)))
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                );
                match event.prep_file.clone() {
                    Some(prep) => {
                        row = row.child(
                            Button::new(("brainz-brief-prep", ix), "Open prep")
                                .label_size(LabelSize::XSmall)
                                .style(ButtonStyle::Filled)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_relative(&prep, window, cx);
                                })),
                        );
                    }
                    None => {
                        row = row.child(
                            Button::new(("brainz-brief-folder", ix), "Open folder")
                                .label_size(LabelSize::XSmall)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_relative(&folder, window, cx);
                                })),
                        );
                    }
                }
            }
            rows.push(row.into_any_element());
        }
        rows
    }

    fn render_stale(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut rows = vec![self.section("Status lines behind their notes", Some(self.brief.stale.len()))];
        if self.brief.stale.is_empty() {
            rows.push(self.empty("Every status callout is as new as its newest note"));
            return rows;
        }
        for (ix, decay) in self.brief.stale.iter().enumerate() {
            let folder = decay.folder.clone();
            rows.push(
                h_flex()
                    .id(("brainz-brief-stale", ix))
                    .w_full()
                    .items_center()
                    .gap_2()
                    .px_1()
                    .py_1()
                    .rounded_md()
                    .hover(|this| this.bg(cx.theme().colors().element_hover))
                    .cursor_pointer()
                    .tooltip(Tooltip::text("Open the folder's CLAUDE.md"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_relative(&folder, window, cx);
                    }))
                    .child(
                        div().flex_1().min_w_0().child(
                            Label::new(decay.folder.clone())
                                .size(LabelSize::Small)
                                .color(Color::Accent)
                                .truncate(),
                        ),
                    )
                    .child(
                        Label::new(decay.tooltip())
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    )
                    .into_any_element(),
            );
        }
        rows
    }

    fn render_loops(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let today = Local::now().date_naive();
        let mut rows = vec![self.section(
            format!("Oldest open loops of {}", self.brief.loops_total),
            None,
        )];
        if self.brief.loops.is_empty() {
            rows.push(self.empty("No open loops, or the Themes pass has not run yet"));
            return rows;
        }
        for (ix, open_loop) in self.brief.loops.iter().enumerate() {
            let text = markdown::markdown_to_plain_text(&open_loop.text)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let who = open_loop.counterparty.clone().unwrap_or_else(|| {
                if open_loop.owed_by_owner() {
                    self.brief.owner.clone()
                } else {
                    "someone".to_owned()
                }
            });
            let file = open_loop.file.clone();
            rows.push(
                h_flex()
                    .id(("brainz-brief-loop", ix))
                    .w_full()
                    .items_center()
                    .gap_2()
                    .px_1()
                    .py_1()
                    .rounded_md()
                    .hover(|this| this.bg(cx.theme().colors().element_hover))
                    .cursor_pointer()
                    .tooltip(Tooltip::text(format!("{}:{}", open_loop.file, open_loop.line)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_relative(&file, window, cx);
                    }))
                    .child(div().w(px(22.)).flex_none().child(Label::new(open_loop.marker.clone()).size(LabelSize::Small)))
                    .child(
                        div().w(px(140.)).flex_none().overflow_hidden().child(
                            Label::new(who).size(LabelSize::Small).truncate(),
                        ),
                    )
                    .child(
                        div().w(px(64.)).flex_none().child(
                            Label::new(format!("{} days", open_loop.age_days(today)))
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                        ),
                    )
                    .child(div().flex_1().min_w_0().child(Label::new(text).size(LabelSize::Small).truncate()))
                    .into_any_element(),
            );
        }
        rows.push(
            div()
                .px_1()
                .pt_1()
                .child(
                    Button::new("brainz-brief-all-loops", "All open loops")
                        .label_size(LabelSize::XSmall)
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(crate::open_loops::OpenOpenLoops), cx);
                        }),
                )
                .into_any_element(),
        );
        rows
    }
}

impl EventEmitter<()> for BriefView {}

impl Focusable for BriefView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Item for BriefView {
    type Event = ();

    fn to_item_events(_: &Self::Event, _: &mut dyn FnMut(workspace::item::ItemEvent)) {}

    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        "Brief".into()
    }

    fn tab_icon(&self, _window: &Window, _cx: &App) -> Option<Icon> {
        Some(Icon::new(IconName::BrainzBrief))
    }

    fn telemetry_event_text(&self) -> Option<&'static str> {
        None
    }
}

impl Render for BriefView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let date = Local::now().format("%A, %B %-d").to_string();
        let updated = self
            .brief
            .generated_at
            .map(|at| format!("Updated {}", at.format("%-I:%M %p")))
            .unwrap_or_default();
        let header = h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .child(
                h_flex()
                    .gap_2()
                    .child(Icon::new(IconName::BrainzBrief).color(Color::Accent))
                    .child(Label::new("Brief").size(LabelSize::Large))
                    .child(Label::new(date).size(LabelSize::Small).color(Color::Muted)),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(Label::new(updated).size(LabelSize::Small).color(Color::Muted))
                    .child(if self.loading {
                        h_flex()
                            .h(px(24.))
                            .px_3()
                            .items_center()
                            .child(ui::bouncing_dots("brainz-brief-loading", cx.theme().colors().text_accent))
                            .into_any_element()
                    } else {
                        IconButton::new("brainz-brief-refresh", IconName::ArrowCircle)
                            .icon_size(IconSize::Small)
                            .tooltip(Tooltip::text("Refresh"))
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx)))
                            .into_any_element()
                    }),
            );

        let mut body: Vec<AnyElement> = Vec::new();
        body.extend(self.render_events(cx));
        body.extend(self.render_stale(cx));
        body.extend(self.render_loops(cx));

        v_flex()
            .id("brainz-brief")
            .key_context("BrainzBrief")
            .track_focus(&self.focus_handle)
            .size_full()
            .overflow_y_scroll()
            .bg(cx.theme().colors().editor_background)
            .child(
                v_flex()
                    .w_full()
                    .max_w(px(1100.))
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

/// Status bar button that opens the Brief tab.
pub struct BriefButton {
    pane_item_focus_handle: Option<FocusHandle>,
    active: bool,
}

impl BriefButton {
    pub fn new() -> Self {
        Self {
            pane_item_focus_handle: None,
            active: false,
        }
    }
}

impl Render for BriefButton {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let focus_handle = self.pane_item_focus_handle.clone();
        let active = self.active;
        div().child(
            IconButton::new("brainz-brief-button", IconName::BrainzBrief)
                .icon_size(IconSize::Small)
                .toggle_state(active)
                .icon_color(if active { Color::Accent } else { Color::Default })
                .tooltip(move |_window, cx| {
                    if let Some(focus_handle) = &focus_handle {
                        Tooltip::for_action_in("Brief", &OpenBrief, focus_handle, cx)
                    } else {
                        Tooltip::for_action("Brief", &OpenBrief, cx)
                    }
                })
                .on_click(|_, window, cx| {
                    window.dispatch_action(Box::new(OpenBrief), cx);
                }),
        )
    }
}

impl StatusItemView for BriefButton {
    fn set_active_pane_item(
        &mut self,
        active_pane_item: Option<&dyn ItemHandle>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pane_item_focus_handle = active_pane_item.map(|item| item.item_focus_handle(cx));
        self.active = active_pane_item.is_some_and(|item| item.downcast::<BriefView>().is_some());
        cx.notify();
    }

    fn hide_setting(&self, _: &App) -> Option<HideStatusItem> {
        None
    }
}
