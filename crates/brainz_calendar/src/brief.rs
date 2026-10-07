//! Brainz: the Brief tab. One page for the start of the day: a narrative
//! block a bot writes into the brain's brief file (Brainz only renders it),
//! today's calendar events that match a folder in the brain (with their
//! prep files), the open owes from the To-Do board, the desk's blocked
//! items and pending decisions, folders whose status callout has gone
//! stale, and the oldest lines flagged ⏳/⏰ in notes. Everything below the narrative comes
//! from data Brainz already computes; no agent is involved.

use std::{path::PathBuf, time::Duration};

use chrono::{DateTime, Local, NaiveDate, Timelike};
use gpui::{
    App, Entity, EventEmitter, FocusHandle, Focusable, Task, TextStyleRefinement, UnderlineStyle,
    WeakEntity, Window, actions,
};
use markdown::{Markdown, MarkdownElement, MarkdownStyle};
use ui::{Tooltip, prelude::*};
use workspace::{
    HideStatusItem, Item, ItemHandle, OpenOptions, OpenVisible, StatusItemView, Workspace,
};

use crate::{
    LoadState,
    brain_config::BrainConfig,
    brain_match::BrainIndex,
    open_loops,
    status_decay::{self, Decay},
    themes_signals::{self as signals, OpenLoop},
    todo,
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

/// The bot-written block between `<!-- narrative:start -->` and
/// `<!-- narrative:end -->` in the brief file.
#[derive(Debug, Clone, PartialEq)]
pub struct Narrative {
    pub text: String,
    /// The first `YYYY-MM-DD` on the block's first line, so the tab can say
    /// how old the read is.
    pub written: Option<NaiveDate>,
    /// True while the block still holds the "no narrative yet" placeholder.
    pub placeholder: bool,
}

/// One unchecked item from the To-Do board.
#[derive(Debug, Clone, PartialEq)]
pub struct Owe {
    pub section: String,
    pub title: String,
    pub tag: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Brief {
    pub generated_at: Option<DateTime<Local>>,
    pub narrative: Option<Narrative>,
    pub brief_file: String,
    pub events: Vec<BriefEvent>,
    pub calendar_problem: Option<String>,
    pub owes: Vec<Owe>,
    pub owes_updated: Option<String>,
    pub todo_file: String,
    pub decisions: Vec<String>,
    pub blocked: Vec<String>,
    pub ops_file: Option<String>,
    pub ops_updated: Option<String>,
    pub stale: Vec<Decay>,
    pub loops: Vec<OpenLoop>,
    pub loops_total: usize,
    pub owner: String,
}

const NARRATIVE_START: &str = "<!-- narrative:start -->";
const NARRATIVE_END: &str = "<!-- narrative:end -->";

/// The narrative block of a brief file, or `None` when the markers are
/// missing.
pub fn narrative_block(text: &str) -> Option<Narrative> {
    let start = text.find(NARRATIVE_START)? + NARRATIVE_START.len();
    let end = text[start..].find(NARRATIVE_END)? + start;
    let inner = text[start..end].trim();
    let placeholder = inner.is_empty()
        || inner
            .trim_start_matches(['_', '*'])
            .to_ascii_lowercase()
            .starts_with("no narrative yet");
    let written = inner.lines().next().and_then(first_iso_date);
    Some(Narrative {
        text: inner.to_owned(),
        written,
        placeholder,
    })
}

/// The first `YYYY-MM-DD` in `line`.
fn first_iso_date(line: &str) -> Option<NaiveDate> {
    let bytes = line.as_bytes();
    (0..bytes.len().saturating_sub(9))
        .filter(|&i| {
            bytes[i..i + 10].iter().enumerate().all(|(j, b)| {
                if j == 4 || j == 7 {
                    *b == b'-'
                } else {
                    b.is_ascii_digit()
                }
            })
        })
        .find_map(|i| NaiveDate::parse_from_str(&line[i..i + 10], "%Y-%m-%d").ok())
}

/// The "## Blocked" and "## Decisions …" bullet lists of a desk file, minus
/// lines already marked ✅, plus its `**Last updated:**` line.
pub fn ops_lists(text: &str) -> (Vec<String>, Vec<String>, Option<String>) {
    let mut blocked = Vec::new();
    let mut decisions = Vec::new();
    let mut updated = None;
    let mut target: Option<&mut Vec<String>> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("**Last updated:**") {
            updated = Some(rest.trim().to_owned());
        } else if let Some(name) = line.strip_prefix("## ") {
            let name = name.trim().to_ascii_lowercase();
            target = if name.starts_with("blocked") {
                Some(&mut blocked)
            } else if name.starts_with("decision") {
                Some(&mut decisions)
            } else {
                None
            };
        } else if let Some(list) = target.as_deref_mut()
            && let Some(item) = line.trim_start().strip_prefix("- ")
            && !item.contains('✅')
            && !item.trim().is_empty()
        {
            list.push(item.trim().to_owned());
        }
    }
    (blocked, decisions, updated)
}

/// Unchecked items from every To-Do section except Done.
fn open_owes(board: &todo::TodoBoard) -> Vec<Owe> {
    board
        .sections
        .iter()
        .filter(|section| !section.name.to_ascii_lowercase().starts_with("done"))
        .flat_map(|section| {
            section
                .items
                .iter()
                .filter(|item| !item.done)
                .map(|item| Owe {
                    section: section.name.clone(),
                    title: item.title.clone(),
                    tag: item.tag.clone(),
                })
        })
        .collect()
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
            for event in all.into_iter().filter(|event| {
                event.start.date_naive() == today
                    || (event.all_day
                        && event.start.date_naive() <= today
                        && today < event.end.date_naive())
            }) {
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

    let mut stale: Vec<Decay> = status_decay::scan(repo, &config, today)
        .into_values()
        .collect();
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

    let narrative = std::fs::read_to_string(repo.join(&config.brief))
        .ok()
        .and_then(|text| narrative_block(&text));

    let (owes, owes_updated) = match std::fs::read_to_string(repo.join(&config.todo)) {
        Ok(text) => {
            let board = todo::parse_board(&text);
            (open_owes(&board), board.updated)
        }
        Err(_) => (Vec::new(), None),
    };

    let (blocked, decisions, ops_updated) = config
        .ops
        .as_ref()
        .and_then(|ops| std::fs::read_to_string(repo.join(ops)).ok())
        .map(|text| ops_lists(&text))
        .unwrap_or_default();

    Brief {
        generated_at: Some(now),
        narrative,
        brief_file: config.brief.clone(),
        events,
        calendar_problem,
        owes,
        owes_updated,
        todo_file: config.todo.clone(),
        decisions,
        blocked,
        ops_file: config.ops.clone(),
        ops_updated,
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
    /// The narrative block as a Markdown entity, rebuilt on every refresh.
    narrative: Option<Entity<Markdown>>,
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
        let Some(repo) = workspace
            .root_paths(cx)
            .first()
            .map(|path| path.to_path_buf())
        else {
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
            narrative: None,
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
                this.narrative = brief
                    .narrative
                    .as_ref()
                    .filter(|narrative| !narrative.placeholder)
                    .map(|narrative| {
                        let text: SharedString = narrative.text.clone().into();
                        cx.new(|cx| Markdown::new(text, None, None, cx))
                    });
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
                    .weight(gpui::FontWeight::SEMIBOLD),
            )
            .when_some(count, |this, count| {
                this.child(
                    Label::new(count.to_string())
                        .size(LabelSize::Small)
                        .color(Color::Placeholder),
                )
            })
            .into_any_element()
    }

    fn empty(&self, text: &'static str) -> AnyElement {
        div()
            .px_2()
            .py_1()
            .child(
                Label::new(text)
                    .size(LabelSize::Small)
                    .color(Color::Placeholder),
            )
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

    fn narrative_style(window: &Window, cx: &App) -> MarkdownStyle {
        let colors = cx.theme().colors();
        let mut text_style = window.text_style();
        text_style.refine(&TextStyleRefinement {
            font_size: Some(TextSize::Small.rems(cx).into()),
            color: Some(colors.text),
            ..Default::default()
        });
        MarkdownStyle {
            base_text_style: text_style,
            selection_background_color: colors.element_selection_background,
            link: TextStyleRefinement {
                color: Some(colors.text_accent),
                underline: Some(UnderlineStyle {
                    color: Some(colors.text_accent.opacity(0.5)),
                    thickness: px(1.),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// The bot-written read at the top of the page.
    fn render_narrative(&self, window: &Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let today = Local::now().date_naive();
        let written = self
            .brief
            .narrative
            .as_ref()
            .and_then(|narrative| narrative.written);
        let age = written.map(|date| (today - date).num_days());
        let subtitle = match age {
            Some(0) => Some("written today".to_owned()),
            Some(1) => Some("written yesterday".to_owned()),
            Some(days) if days > 1 => Some(format!("written {days} days ago")),
            _ => None,
        };
        let mut rows = vec![
            h_flex()
                .px_1()
                .pt_2()
                .pb_1()
                .gap_2()
                .child(
                    Label::new("The read")
                        .size(LabelSize::Small)
                        .weight(gpui::FontWeight::SEMIBOLD),
                )
                .when_some(subtitle, |this, subtitle| {
                    this.child(Label::new(subtitle).size(LabelSize::Small).color(
                        if age.is_some_and(|days| days > 1) {
                            Color::Warning
                        } else {
                            Color::Placeholder
                        },
                    ))
                })
                .into_any_element(),
        ];
        let file = self.brief.brief_file.clone();
        match &self.narrative {
            Some(markdown) => rows.push(
                div()
                    .id("brainz-brief-narrative")
                    .w_full()
                    .px_3()
                    .py_2()
                    .rounded_lg()
                    .border_1()
                    .border_color(cx.theme().colors().border_variant)
                    .bg(cx.theme().colors().surface_background)
                    .child(MarkdownElement::new(
                        markdown.clone(),
                        Self::narrative_style(window, cx),
                    ))
                    .into_any_element(),
            ),
            None => rows.push(
                h_flex()
                    .px_2()
                    .py_1()
                    .gap_2()
                    .items_center()
                    .child(
                        Label::new(if self.brief.narrative.is_some() {
                            "No read yet today."
                        } else {
                            "No brief file in this brain yet."
                        })
                        .size(LabelSize::Small)
                        .color(Color::Placeholder),
                    )
                    .when(self.brief.narrative.is_some(), |this| {
                        this.child(
                            Button::new("brainz-brief-open-narrative", "Open brief")
                                .label_size(LabelSize::Small)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_relative(&file, window, cx);
                                })),
                        )
                    })
                    .into_any_element(),
            ),
        }
        rows
    }

    /// Unchecked items from the To-Do board, grouped by its sections.
    fn render_owes(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut rows = vec![self.section("Open owes", Some(self.brief.owes.len()))];
        if self.brief.owes.is_empty() {
            rows.push(self.empty("Nothing owed"));
            return rows;
        }
        let todo_file = self.brief.todo_file.clone();
        for (ix, owe) in self.brief.owes.iter().enumerate() {
            let file = todo_file.clone();
            let urgent = {
                let name = owe.section.to_ascii_lowercase();
                name.starts_with("today") || name.starts_with("urgent")
            };
            let row = h_flex()
                .id(("brainz-brief-owe", ix))
                .w_full()
                .items_center()
                .gap_2()
                .px_1()
                .py_1()
                .rounded_md()
                .hover(|this| this.bg(cx.theme().colors().element_hover))
                .cursor_pointer()
                .tooltip(Tooltip::text("Open the To-Do board"))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_relative(&file, window, cx);
                }))
                .child(
                    div().w(px(110.)).flex_none().child(
                        Label::new(owe.section.clone())
                            .size(LabelSize::Small)
                            .color(if urgent { Color::Warning } else { Color::Muted })
                            .truncate(),
                    ),
                )
                .child(
                    div().flex_1().min_w_0().child(
                        Label::new(owe.title.clone())
                            .size(LabelSize::Small)
                            .truncate(),
                    ),
                );
            rows.push(ui::reveal(("brainz-brief-owe-reveal", ix), ix, row).into_any_element());
        }
        if let Some(updated) = &self.brief.owes_updated {
            rows.push(
                div()
                    .px_1()
                    .child(
                        Label::new(format!("Board updated {updated}"))
                            .size(LabelSize::Small)
                            .color(Color::Placeholder),
                    )
                    .into_any_element(),
            );
        }
        rows
    }

    /// The desk's pending decisions and blocked items.
    fn render_desk(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(ops_file) = self.brief.ops_file.clone() else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        for (title, items, key) in [
            (
                "Decisions only you can make",
                &self.brief.decisions,
                "decision",
            ),
            ("Blocked", &self.brief.blocked, "blocked"),
        ] {
            rows.push(self.section(title, Some(items.len())));
            if items.is_empty() {
                rows.push(self.empty("Nothing right now"));
                continue;
            }
            for (ix, item) in items.iter().enumerate() {
                let file = ops_file.clone();
                let text = markdown::markdown_to_plain_text(item)
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                rows.push(
                    h_flex()
                        .id((key, ix))
                        .w_full()
                        .items_center()
                        .gap_2()
                        .px_1()
                        .py_1()
                        .rounded_md()
                        .hover(|this| this.bg(cx.theme().colors().element_hover))
                        .cursor_pointer()
                        .tooltip(Tooltip::text("Open the desk file"))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_relative(&file, window, cx);
                        }))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(Label::new(text).size(LabelSize::Small).truncate()),
                        )
                        .into_any_element(),
                );
            }
        }
        if let Some(updated) = &self.brief.ops_updated {
            rows.push(
                div()
                    .px_1()
                    .child(
                        Label::new(format!("Desk updated {updated}"))
                            .size(LabelSize::Small)
                            .color(Color::Placeholder),
                    )
                    .into_any_element(),
            );
        }
        rows
    }

    fn render_events(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut rows = vec![self.section("Today", Some(self.brief.events.len()))];
        if let Some(problem) = &self.brief.calendar_problem {
            rows.push(
                div()
                    .px_2()
                    .py_1()
                    .child(
                        Label::new(problem.clone())
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .into_any_element(),
            );
            return rows;
        }
        if self.brief.events.is_empty() {
            rows.push(self.empty("Nothing on the calendar"));
            return rows;
        }
        let now = Local::now();
        for (ix, event) in self.brief.events.iter().enumerate() {
            let past = !event.all_day && event.start < now;
            let matched = event.folder.is_some();
            let mut row = h_flex()
                .id(("brainz-brief-event", ix))
                .group("brainz-brief-event")
                .w_full()
                .items_center()
                .gap_2()
                .px_1()
                .py_1()
                .rounded_md()
                .hover(|this| this.bg(cx.theme().colors().element_hover))
                .child(
                    div().w(px(64.)).flex_none().child(
                        Label::new(Self::time_label(event))
                            .size(LabelSize::Small)
                            .color(if past {
                                Color::Placeholder
                            } else {
                                Color::Muted
                            }),
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
                        .size(LabelSize::Small)
                        .color(Color::Muted)
                        .truncate(),
                );
                let (id, label, target) = match event.prep_file.clone() {
                    Some(prep) => ("brainz-brief-prep", "Open prep", prep),
                    None => ("brainz-brief-folder", "Open folder", folder),
                };
                row = row.child(
                    div().visible_on_hover("brainz-brief-event").child(
                        Button::new((id, ix), label)
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_relative(&target, window, cx);
                            })),
                    ),
                );
            }
            rows.push(ui::reveal(("brainz-brief-event-reveal", ix), ix, row).into_any_element());
        }
        rows
    }

    fn render_stale(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut rows = vec![self.section(
            "Status lines behind their notes",
            Some(self.brief.stale.len()),
        )];
        if self.brief.stale.is_empty() {
            rows.push(self.empty("Every status line is current"));
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
                                .truncate(),
                        ),
                    )
                    .child(
                        Label::new(decay.tooltip())
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .into_any_element(),
            );
        }
        rows
    }

    /// Copies the flagged line onto the To-Do board and ticks it in its
    /// note, then re-runs the signals pass so it stays gone.
    fn add_loop_to_board(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(open_loop) = self.brief.loops.get(ix).cloned() else {
            return;
        };
        self.brief.loops.remove(ix);
        self.brief.loops_total = self.brief.loops_total.saturating_sub(1);
        cx.notify();
        let note = self.repo.join(&open_loop.file);
        let board = self.repo.join(&self.brief.todo_file);
        let owner = self.brief.owner.clone();
        let repo = self.repo.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    open_loops::add_to_board(&board, &open_loop, &owner)?;
                    open_loops::strike_marker(&note, &open_loop)
                })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        if let Some(runner) = crate::themes::runner(cx) {
                            runner.update(cx, |runner, cx| runner.run(repo, cx));
                        }
                    }
                    Err(error) => log::error!("brainz brief add to board: {error:#}"),
                }
                this.refresh(cx);
            })
            .ok();
        })
        .detach();
    }

    fn render_loops(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let today = Local::now().date_naive();
        let mut rows = vec![self.section("Flagged in notes", Some(self.brief.loops_total))];
        if self.brief.loops.is_empty() {
            rows.push(self.empty("Nothing flagged"));
            return rows;
        }
        rows.push(
            div()
                .px_1()
                .pb_1()
                .child(
                    Label::new(
                        "Lines in notes starting with ⏳ (waiting on someone) or ⏰ (you owe it) that are not on the To-Do board yet, oldest first.",
                    )
                    .size(LabelSize::Small)
                    .color(Color::Muted),
                )
                .into_any_element(),
        );
        for (ix, open_loop) in self.brief.loops.iter().enumerate() {
            let text = markdown::markdown_to_plain_text(&open_loop.text)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let who = match (&open_loop.counterparty, open_loop.owed_by_owner()) {
                (_, true) => format!("{} owes", self.brief.owner),
                (Some(person), false) => format!("Waiting on {person}"),
                (None, false) => "Waiting".to_owned(),
            };
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
                    .tooltip(Tooltip::text(format!(
                        "Open {}:{}",
                        open_loop.file, open_loop.line
                    )))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_relative(&file, window, cx);
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
                                .color(if open_loop.owed_by_owner() {
                                    Color::Warning
                                } else {
                                    Color::Muted
                                })
                                .truncate(),
                        ),
                    )
                    .child(
                        div().w(px(56.)).flex_none().child(
                            Label::new(format!("{} days", open_loop.age_days(today)))
                                .size(LabelSize::Small)
                                .color(Color::Placeholder),
                        ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Label::new(text).size(LabelSize::Small).truncate()),
                    )
                    .child(
                        Button::new(("brainz-brief-loop-add", ix), "Add to To-Do")
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.add_loop_to_board(ix, cx);
                            })),
                    )
                    .into_any_element(),
            );
        }
        rows.push(
            div()
                .px_1()
                .pt_1()
                .child(
                    Button::new("brainz-brief-all-loops", "All flagged lines")
                        .label_size(LabelSize::Small)
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(crate::todo::OpenTodo), cx);
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                    .child(Icon::new(IconName::BrainzBrief).color(Color::Muted))
                    .child(Label::new("Brief").size(LabelSize::Large))
                    .child(Label::new(date).size(LabelSize::Small).color(Color::Muted)),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Label::new(updated)
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .child(if self.loading {
                        h_flex()
                            .h(px(24.))
                            .px_3()
                            .items_center()
                            .child(ui::bouncing_dots(
                                "brainz-brief-loading",
                                cx.theme().colors().text_accent,
                            ))
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
        body.extend(self.render_narrative(window, cx));
        body.extend(self.render_events(cx));
        body.extend(self.render_owes(cx));
        body.extend(self.render_desk(cx));
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
                .icon_color(if active {
                    Color::Accent
                } else {
                    Color::Default
                })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn narrative_block_reads_the_bot_block_and_its_date() {
        let text = "# Brief\n\n<!-- narrative:start -->\n_Written 2026-10-06._\n\nThe day is about the Tekmetric counter.\n<!-- narrative:end -->\n\nTrailer.\n";
        let narrative = narrative_block(text).unwrap();
        assert_eq!(narrative.written, NaiveDate::from_ymd_opt(2026, 10, 6));
        assert!(!narrative.placeholder);
        assert!(narrative.text.starts_with("_Written 2026-10-06._"));
        assert!(narrative.text.ends_with("Tekmetric counter."));
    }

    #[test]
    fn narrative_block_flags_the_placeholder_and_missing_markers() {
        let placeholder = narrative_block(
            "<!-- narrative:start -->\n_No narrative yet. Paste the prompt into Grokbot._\n<!-- narrative:end -->",
        )
        .unwrap();
        assert!(placeholder.placeholder);
        assert_eq!(placeholder.written, None);
        assert!(narrative_block("# Brief without markers").is_none());
        assert!(
            narrative_block("<!-- narrative:start -->\n<!-- narrative:end -->")
                .unwrap()
                .placeholder
        );
    }

    #[test]
    fn ops_lists_take_open_blocked_and_decision_bullets() {
        let text = "# OPS\n**Last updated:** 2026-09-30 evening\n\n## Priorities\n- ignored\n\n## Blocked\n- ✅ done thing\n- Cancel Peacock by Tue 10/6\n- MA inspection\n\n## Decisions only Tommy can make\n- Accept the offer when it lands\n\n## Shipped\n- not a decision\n";
        let (blocked, decisions, updated) = ops_lists(text);
        assert_eq!(blocked, vec!["Cancel Peacock by Tue 10/6", "MA inspection"]);
        assert_eq!(decisions, vec!["Accept the offer when it lands"]);
        assert_eq!(updated.as_deref(), Some("2026-09-30 evening"));
    }

    #[test]
    fn open_owes_skip_done_items_and_the_done_section() {
        let board = todo::parse_board(
            "# TODO\n## Today\n- [ ] Pick up the jersey `jersey`\n- [x] Already done `done`\n## Done\n- [ ] Stray unchecked in Done\n",
        );
        let owes = open_owes(&board);
        assert_eq!(owes.len(), 1);
        assert_eq!(owes[0].section, "Today");
        assert_eq!(owes[0].title, "Pick up the jersey");
        assert_eq!(owes[0].tag.as_deref(), Some("jersey"));
    }
}
