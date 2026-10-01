//! Brainz: a Calendar tab that shows the next seven days, read through the
//! bundled `brainz-calendar` helper (a small EventKit program).

pub mod brain_config;
pub mod brain_match;
pub mod github_sync;
pub mod ocr;
pub mod open_loops;
pub mod prep;
pub mod launcher;
pub mod mcp;
pub mod memory_share;
pub mod status_decay;
pub mod themes;
pub mod themes_signals;
pub mod todo;

use std::{path::PathBuf, time::Duration};

use anyhow::{Context as _, Result, anyhow};
use chrono::{DateTime, Local, Timelike};
use gpui::{App, EventEmitter, FocusHandle, Focusable, Hsla, Task, Window, actions};
use serde::Deserialize;
use ui::{Tooltip, prelude::*};
use workspace::{HideStatusItem, Item, ItemHandle, StatusItemView, Workspace};

actions!(
    brainz_calendar,
    [
        /// Opens the Calendar tab.
        OpenCalendar
    ]
);

const DAYS: i64 = 7;
const REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);

pub fn init(cx: &mut App) {
    mcp::init(cx);
    todo::init(cx);
    themes::init(cx);
    github_sync::init(cx);
    prep::init(cx);
    open_loops::init(cx);
    status_decay::init(cx);
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &OpenCalendar, window, cx| {
            CalendarView::open(workspace, window, cx);
        });
    })
    .detach();
}

#[derive(Debug, Clone, Deserialize)]
struct HelperOutput {
    status: String,
    #[serde(default)]
    events: Vec<HelperEvent>,
}

#[derive(Debug, Clone, Deserialize)]
struct HelperEvent {
    id: String,
    title: String,
    start: String,
    end: String,
    all_day: bool,
    calendar: String,
    #[serde(default)]
    #[allow(dead_code)]
    attendees: u32,
    #[serde(default)]
    location: Option<String>,
    #[serde(default)]
    color: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    attendee_names: Vec<String>,
    #[serde(default)]
    organizer: Option<String>,
    #[serde(default)]
    attendee_emails: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CalendarEvent {
    pub id: String,
    pub title: SharedString,
    pub start: DateTime<Local>,
    pub end: DateTime<Local>,
    pub all_day: bool,
    pub calendar: SharedString,
    #[allow(dead_code)]
    pub attendees: u32,
    pub location: Option<SharedString>,
    pub color: Option<Hsla>,
    pub url: Option<String>,
    pub notes: Option<String>,
    pub attendee_names: Vec<String>,
    pub organizer: Option<String>,
    pub attendee_emails: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LoadState {
    Loading,
    Ready,
    NoAccess(String),
    Failed(String),
}

fn helper_path() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("locating Brainz executable")?;
    let dir = exe.parent().context("Brainz executable has no parent")?;
    let helper = dir.join("brainz-calendar");
    if helper.is_file() {
        Ok(helper)
    } else {
        Err(anyhow!(
            "calendar helper missing at {}; run script/brainz-local",
            helper.display()
        ))
    }
}

fn parse_hex(hex: &str) -> Option<Hsla> {
    let hex = hex.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let value = u32::from_str_radix(hex, 16).ok()?;
    Some(gpui::rgb(value).into())
}

/// Blocking on purpose: the helper only ever runs on the background executor.
#[allow(clippy::disallowed_methods)]
pub fn load_events() -> Result<(LoadState, Vec<CalendarEvent>)> {
    let helper = helper_path()?;
    let output = std::process::Command::new(&helper)
        .arg(DAYS.to_string())
        .output()
        .with_context(|| format!("running {}", helper.display()))?;
    if !output.status.success() {
        return Err(anyhow!(
            "calendar helper exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let parsed: HelperOutput =
        serde_json::from_slice(&output.stdout).context("parsing calendar helper output")?;
    match parsed.status.as_str() {
        "ok" => {}
        "denied" => {
            return Ok((
                LoadState::NoAccess(
                    "Calendar access is turned off for Brainz. Turn it on in System Settings \
                     → Privacy & Security → Calendars, then refresh."
                        .into(),
                ),
                Vec::new(),
            ));
        }
        other => {
            return Ok((
                LoadState::NoAccess(format!(
                    "Brainz can't read your calendar right now ({other}). Check System \
                     Settings → Privacy & Security → Calendars."
                )),
                Vec::new(),
            ));
        }
    }
    let events = parsed
        .events
        .into_iter()
        .filter_map(|event| {
            let start = DateTime::parse_from_rfc3339(&event.start)
                .ok()?
                .with_timezone(&Local);
            let end = DateTime::parse_from_rfc3339(&event.end)
                .ok()?
                .with_timezone(&Local);
            Some(CalendarEvent {
                id: event.id,
                title: event.title.into(),
                start,
                end,
                all_day: event.all_day,
                calendar: event.calendar.into(),
                attendees: event.attendees,
                location: event.location.map(Into::into),
                color: event.color.as_deref().and_then(parse_hex),
                url: event.url,
                notes: event.notes,
                attendee_names: event.attendee_names,
                organizer: event.organizer,
                attendee_emails: event.attendee_emails,
            })
        })
        .collect();
    Ok((LoadState::Ready, events))
}

pub struct CalendarView {
    focus_handle: FocusHandle,
    state: LoadState,
    events: Vec<CalendarEvent>,
    refreshed_at: Option<DateTime<Local>>,
    /// Brainz: the event shown in the detail pane, by id.
    selected_event: Option<String>,
    _load: Option<Task<()>>,
    _refresh_loop: Task<()>,
}

impl CalendarView {
    pub fn open(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
        let existing = workspace.items_of_type::<CalendarView>(cx).next();
        if let Some(existing) = existing {
            workspace.activate_item(&existing, true, true, window, cx);
            existing.update(cx, |view, cx| view.refresh(cx));
        } else {
            let view = cx.new(|cx| CalendarView::new(cx));
            workspace.add_item_to_active_pane(Box::new(view), None, true, window, cx);
        }
    }

    fn new(cx: &mut Context<Self>) -> Self {
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
            state: LoadState::Loading,
            events: Vec::new(),
            refreshed_at: None,
            selected_event: None,
            _load: None,
            _refresh_loop: refresh_loop,
        };
        this.refresh(cx);
        this
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.events.is_empty() {
            self.state = LoadState::Loading;
        }
        cx.notify();
        self._load = Some(cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async { load_events() }).await;
            this.update(cx, |this, cx| {
                match result {
                    Ok((state, events)) => {
                        this.state = state;
                        this.events = events;
                    }
                    Err(error) => {
                        log::error!("brainz calendar: {error:#}");
                        this.state = LoadState::Failed(format!("{error:#}"));
                    }
                }
                this.refreshed_at = Some(Local::now());
                cx.notify();
            })
            .ok();
        }));
    }

    fn time_label(event: &CalendarEvent) -> String {
        if event.all_day {
            return "All day".into();
        }
        let fmt = |time: &DateTime<Local>| {
            let (_, hour) = time.hour12();
            if time.minute() == 0 {
                format!("{hour}")
            } else {
                format!("{hour}:{:02}", time.minute())
            }
        };
        let suffix = |time: &DateTime<Local>| if time.hour12().0 { "pm" } else { "am" };
        if suffix(&event.start) == suffix(&event.end) {
            format!(
                "{}–{} {}",
                fmt(&event.start),
                fmt(&event.end),
                suffix(&event.end)
            )
        } else {
            format!(
                "{} {}–{} {}",
                fmt(&event.start),
                suffix(&event.start),
                fmt(&event.end),
                suffix(&event.end)
            )
        }
    }

    fn render_event_card(
        &self,
        ix: usize,
        event: &CalendarEvent,
        now: DateTime<Local>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let in_progress = !event.all_day && event.start <= now && now < event.end;
        let past = event.end <= now;
        let accent = event
            .color
            .unwrap_or_else(|| cx.theme().colors().icon_accent);
        let selected = self.selected_event.as_deref() == Some(event.id.as_str());
        let event_id = event.id.clone();
        let text_color = if past { Color::Muted } else { Color::Default };
        let mut tooltip = event.title.to_string();
        if let Some(location) = &event.location {
            tooltip.push('\n');
            tooltip.push_str(location);
        }
        tooltip.push('\n');
        tooltip.push_str(event.calendar.as_ref());
        v_flex()
            .id(("brainz-calendar-event", ix))
            .w_full()
            .px_2()
            .py_1()
            .gap_0p5()
            .rounded_md()
            .border_l_2()
            .border_color(accent)
            .bg(accent.opacity(if past { 0.08 } else { 0.18 }))
            .when(in_progress, |this| this.bg(accent.opacity(0.32)))
            .when(selected, |this| {
                this.bg(accent.opacity(0.45))
                    .border_1()
                    .border_l_2()
                    .border_color(accent)
            })
            .hover(|this| this.bg(accent.opacity(0.4)))
            .cursor_pointer()
            .tooltip(Tooltip::text(tooltip))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.selected_event = if this.selected_event.as_deref() == Some(&event_id) {
                    None
                } else {
                    Some(event_id.clone())
                };
                cx.notify();
            }))
            .child(
                Label::new(Self::time_label(event))
                    .size(LabelSize::XSmall)
                    .color(if in_progress {
                        Color::Accent
                    } else {
                        Color::Muted
                    }),
            )
            .child(
                Label::new(event.title.clone())
                    .size(LabelSize::Small)
                    .color(text_color)
                    .when(past, |label| label.strikethrough())
                    .truncate(),
            )
    }

    /// Brainz: the right-hand detail pane for the selected event.
    fn render_detail(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let id = self.selected_event.as_deref()?;
        let event = self.events.iter().find(|event| event.id == id)?;
        let accent = event
            .color
            .unwrap_or_else(|| cx.theme().colors().icon_accent);
        let when = if event.all_day {
            let start = event.start.format("%A, %b %-d");
            let last = (event.end - chrono::Duration::seconds(1)).date_naive();
            if last == event.start.date_naive() {
                format!("{start} · All day")
            } else {
                format!("{start} – {} · All day", last.format("%A, %b %-d"))
            }
        } else if event.start.date_naive() == event.end.date_naive() {
            format!(
                "{} · {}",
                event.start.format("%A, %b %-d"),
                Self::time_label(event)
            )
        } else {
            format!(
                "{} – {}",
                event.start.format("%a %b %-d, %-I:%M %p"),
                event.end.format("%a %b %-d, %-I:%M %p")
            )
        };
        let row = |label: &str, value: SharedString| {
            v_flex()
                .gap_0p5()
                .child(
                    Label::new(label.to_owned())
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                )
                .child(Label::new(value).size(LabelSize::Small))
        };
        let mut body = v_flex()
            .gap_3()
            .child(row("When", when.into()))
            .child(row("Calendar", event.calendar.clone()));
        if let Some(location) = &event.location {
            body = body.child(row("Where", location.clone()));
        }
        if let Some(organizer) = &event.organizer {
            body = body.child(row("Organizer", organizer.clone().into()));
        }
        if !event.attendee_names.is_empty() {
            body = body.child(row(
                &format!("Guests ({})", event.attendee_names.len()),
                event.attendee_names.join(", ").into(),
            ));
        }
        if let Some(notes) = &event.notes {
            body = body.child(row("Notes", notes.clone().into()));
        }
        if let Some(url) = event.url.clone() {
            let shown = url.clone();
            body = body.child(
                Button::new("brainz-calendar-open-link", "Open Link")
                    .style(ButtonStyle::Filled)
                    .tooltip(Tooltip::text(shown))
                    .on_click(move |_, _, cx| cx.open_url(&url)),
            );
        }
        Some(
            v_flex()
                .id("brainz-calendar-detail")
                .flex_none()
                .w(px(300.))
                .h_full()
                .ml_3()
                .p_3()
                .gap_3()
                .rounded_lg()
                .border_1()
                .border_color(cx.theme().colors().border)
                .bg(cx.theme().colors().surface_background)
                .overflow_y_scroll()
                .child(
                    h_flex()
                        .items_start()
                        .justify_between()
                        .gap_2()
                        .child(
                            h_flex()
                                .gap_2()
                                .min_w_0()
                                .child(div().flex_none().size_2p5().rounded_full().bg(accent).mt_1p5())
                                .child(Label::new(event.title.clone()).weight(gpui::FontWeight::SEMIBOLD)),
                        )
                        .child(
                            IconButton::new("brainz-calendar-detail-close", IconName::Close)
                                .icon_size(IconSize::XSmall)
                                .tooltip(Tooltip::text("Close"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.selected_event = None;
                                    cx.notify();
                                })),
                        ),
                )
                .child(body)
                .into_any_element(),
        )
    }

    /// Seven columns, one per day, like a week view without the hour grid.
    fn render_week(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let now = Local::now();
        let today = now.date_naive();
        let mut columns = Vec::with_capacity(DAYS as usize);
        let mut event_ix = 0usize;
        let border = cx.theme().colors().border_variant;
        for offset in 0..DAYS {
            let day = today + chrono::Days::new(offset as u64);
            let is_today = day == today;
            let events: Vec<&CalendarEvent> = self
                .events
                .iter()
                .filter(|event| {
                    let start = event.start.date_naive();
                    let end_inclusive = if event.all_day {
                        (event.end - chrono::Duration::seconds(1)).date_naive()
                    } else if event.end.time() == chrono::NaiveTime::MIN {
                        (event.end - chrono::Duration::seconds(1)).date_naive()
                    } else {
                        event.end.date_naive()
                    };
                    start <= day && day <= end_inclusive
                })
                .collect();

            let header = v_flex()
                .w_full()
                .items_center()
                .gap_0p5()
                .pb_2()
                .child(
                    Label::new(day.format("%a").to_string().to_uppercase())
                        .size(LabelSize::XSmall)
                        .color(if is_today {
                            Color::Accent
                        } else {
                            Color::Muted
                        }),
                )
                .child(
                    div()
                        .size_8()
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(is_today, |this| this.bg(cx.theme().colors().text_accent))
                        .child(
                            Label::new(day.format("%-d").to_string())
                                .size(LabelSize::Large)
                                .when(is_today, |label| {
                                    label.color(Color::Custom(gpui::hsla(0., 0., 0.08, 1.)))
                                }),
                        ),
                );

            let mut column = v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .px_1()
                .when(offset > 0, |this| this.border_l_1().border_color(border))
                .child(header);
            if events.is_empty() {
                column = column.child(
                    div().px_1().child(
                        Label::new("—")
                            .size(LabelSize::Small)
                            .color(Color::Placeholder),
                    ),
                );
            } else {
                for event in events {
                    column = column.child(self.render_event_card(event_ix, event, now, cx));
                    event_ix += 1;
                }
            }
            columns.push(column.into_any_element());
        }
        h_flex()
            .w_full()
            .items_start()
            .pt_2()
            .children(columns)
            .into_any_element()
    }
}

impl EventEmitter<()> for CalendarView {}

impl Focusable for CalendarView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Item for CalendarView {
    type Event = ();

    fn to_item_events(_: &Self::Event, _: &mut dyn FnMut(workspace::item::ItemEvent)) {}

    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        "Calendar".into()
    }

    fn tab_icon(&self, _window: &Window, _cx: &App) -> Option<Icon> {
        Some(Icon::new(IconName::BrainzCalendar))
    }

    fn telemetry_event_text(&self) -> Option<&'static str> {
        None
    }
}

impl Render for CalendarView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let refreshed = self
            .refreshed_at
            .map(|at| format!("Updated {}", at.format("%-I:%M %p")))
            .unwrap_or_default();
        let header = h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .px_3()
            .child(
                h_flex()
                    .gap_2()
                    .child(Icon::new(IconName::BrainzCalendar).color(Color::Accent))
                    .child(Label::new("Next 7 days").size(LabelSize::Large)),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Label::new(refreshed)
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .child(
                        IconButton::new("brainz-calendar-refresh", IconName::ArrowCircle)
                            .icon_size(IconSize::Small)
                            .tooltip(Tooltip::text("Refresh"))
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                    ),
            );

        let body: Vec<gpui::AnyElement> = match &self.state {
            LoadState::Loading => vec![
                div()
                    .px_3()
                    .py_4()
                    .child(Label::new("Reading your calendar…").color(Color::Muted))
                    .into_any_element(),
            ],
            LoadState::NoAccess(message) | LoadState::Failed(message) => vec![
                v_flex()
                    .mx_3()
                    .my_4()
                    .p_4()
                    .gap_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(cx.theme().colors().border)
                    .bg(cx.theme().colors().surface_background)
                    .child(Label::new(message.clone()))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("brainz-calendar-settings", "Open Calendar Privacy")
                                    .style(ButtonStyle::Filled)
                                    .on_click(|_, _, cx| {
                                        cx.open_url(
                                            "x-apple.systempreferences:com.apple.preference.security?Privacy_Calendars",
                                        )
                                    }),
                            )
                            .child(
                                Button::new("brainz-calendar-retry", "Try Again")
                                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                            ),
                    )
                    .into_any_element(),
            ],
            LoadState::Ready => vec![
                h_flex()
                    .w_full()
                    .items_start()
                    .child(div().flex_1().min_w_0().child(self.render_week(cx)))
                    .children(self.render_detail(cx))
                    .into_any_element(),
            ],
        };

        v_flex()
            .id("brainz-calendar")
            .key_context("BrainzCalendar")
            .track_focus(&self.focus_handle)
            .size_full()
            .overflow_y_scroll()
            .bg(cx.theme().colors().editor_background)
            .child(
                v_flex()
                    .w_full()
                    .pt_4()
                    .pb_8()
                    .px_4()
                    .child(header)
                    .children(body),
            )
    }
}

/// Status bar button that opens the Calendar tab.
pub struct CalendarButton {
    pane_item_focus_handle: Option<FocusHandle>,
    /// The Calendar tab is the active item, so the button lights up.
    active: bool,
}

impl CalendarButton {
    pub fn new() -> Self {
        Self {
            pane_item_focus_handle: None,
            active: false,
        }
    }
}

impl Render for CalendarButton {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let focus_handle = self.pane_item_focus_handle.clone();
        let active = self.active;
        div().child(
            IconButton::new("brainz-calendar-button", IconName::BrainzCalendar)
                .icon_size(IconSize::Small)
                .toggle_state(active)
                .icon_color(if active { Color::Accent } else { Color::Default })
                .tooltip(move |_window, cx| {
                    if let Some(focus_handle) = &focus_handle {
                        Tooltip::for_action_in("Calendar", &OpenCalendar, focus_handle, cx)
                    } else {
                        Tooltip::for_action("Calendar", &OpenCalendar, cx)
                    }
                })
                .on_click(|_, window, cx| {
                    window.dispatch_action(Box::new(OpenCalendar), cx);
                }),
        )
    }
}

impl StatusItemView for CalendarButton {
    fn set_active_pane_item(
        &mut self,
        active_pane_item: Option<&dyn ItemHandle>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pane_item_focus_handle = active_pane_item.map(|item| item.item_focus_handle(cx));
        self.active =
            active_pane_item.is_some_and(|item| item.downcast::<CalendarView>().is_some());
        cx.notify();
    }

    fn hide_setting(&self, _: &App) -> Option<HideStatusItem> {
        None
    }
}
