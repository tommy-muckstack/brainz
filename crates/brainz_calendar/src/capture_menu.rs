//! Brainz: the menu behind the floating avatar. It holds the quick capture
//! field, the next calendar events, and the newest My Man screenshots and
//! recordings ready to copy. It also raises a small notice beside the avatar
//! shortly before an event starts.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, anyhow};
use chrono::{DateTime, Local};
use editor::Editor;
use gpui::{
    Animation, AnimationExt as _, App, AppContext as _, Bounds, ClipboardItem, DisplayId, Entity,
    FocusHandle, Focusable, Global, Image, ImageFormat, MouseButton, ObjectFit, Pixels, Size,
    WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions, ease_out_quint, img,
    point, pulsating_between, px,
};
use ui::prelude::*;
use util::ResultExt as _;

use crate::{CalendarEvent, LoadState, myman::MyMan};

const MENU_WIDTH: Pixels = px(340.);
/// The menu's window is this tall and see-through; the panel inside it hugs
/// the edge nearest the avatar, so it can grow without the window moving.
const MENU_HEIGHT: Pixels = px(560.);
const MENU_OPEN: Duration = Duration::from_millis(180);
/// How far the panel travels as it slides out of the avatar.
const MENU_SLIDE: f32 = 28.;
const TILE_WIDTH: Pixels = px(136.);
const NOTICE_SIZE: Size<Pixels> = Size {
    width: px(232.),
    height: px(50.),
};
const GAP: Pixels = px(8.);
const CAPTURE_LIMIT: usize = 6;
/// Newest files read per My Man folder; names start with the capture date.
const CAPTURE_SCAN: usize = 40;
const CALENDAR_REFRESH: Duration = Duration::from_secs(5 * 60);
const NOTICE_CHECK: Duration = Duration::from_secs(30);
const NOTICE_LEAD_MINUTES: i64 = 10;
/// A click on the avatar first closes the open menu by taking focus from it;
/// within this window the same click must not open it again.
const REOPEN_GUARD: Duration = Duration::from_millis(400);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureKind {
    Screenshot,
    Recording,
}

#[derive(Debug, Clone, PartialEq)]
struct Capture {
    kind: CaptureKind,
    file: PathBuf,
    thumbnail: Option<PathBuf>,
    title: String,
    captured: DateTime<Local>,
}

/// My Man writes paths as JSON strings, with `\/` for each slash.
fn front_path(value: &str) -> PathBuf {
    PathBuf::from(value.replace("\\/", "/"))
}

fn parse_capture(kind: CaptureKind, note: &str) -> Option<Capture> {
    let front = crate::myman::front_matter(note);
    let file = front_path(front.get("file")?);
    let captured = DateTime::parse_from_rfc3339(front.get("captured")?)
        .ok()?
        .with_timezone(&Local);
    let title = match kind {
        CaptureKind::Screenshot => front
            .get("summary")
            .map(|summary| {
                summary
                    .trim_start_matches("Screenshot showing ")
                    .trim_end_matches('.')
                    .to_owned()
            })
            .filter(|summary| !summary.is_empty())
            .unwrap_or_else(|| "Screenshot".to_owned()),
        CaptureKind::Recording => match front
            .get("duration_seconds")
            .and_then(|seconds| seconds.parse::<u64>().ok())
        {
            Some(seconds) if seconds >= 60 => {
                format!("Recording · {}m {}s", seconds / 60, seconds % 60)
            }
            Some(seconds) => format!("Recording · {seconds}s"),
            None => "Recording".to_owned(),
        },
    };
    Some(Capture {
        kind,
        file,
        thumbnail: front.get("thumbnail").map(|path| front_path(path)),
        title,
        captured,
    })
}

fn folder_captures(folder: &Path, kind: CaptureKind) -> Vec<Capture> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut names: Vec<PathBuf> = entries
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|path| path.extension().is_some_and(|extension| extension == "md"))
        .collect();
    names.sort();
    names
        .iter()
        .rev()
        .take(CAPTURE_SCAN)
        .filter_map(|path| parse_capture(kind, &std::fs::read_to_string(path).ok()?))
        .collect()
}

/// The newest screenshots and recordings My Man still has files for.
fn recent_captures(myman: &MyMan) -> Vec<Capture> {
    let mut captures = folder_captures(&myman.join("screenshots"), CaptureKind::Screenshot);
    captures.extend(folder_captures(
        &myman.join("recordings"),
        CaptureKind::Recording,
    ));
    captures.retain(|capture| capture.file.is_file());
    captures.sort_by_key(|capture| std::cmp::Reverse(capture.captured));
    captures.truncate(CAPTURE_LIMIT);
    captures
}

fn age(captured: DateTime<Local>, now: DateTime<Local>) -> String {
    let minutes = (now - captured).num_minutes();
    match minutes {
        ..1 => "now".to_owned(),
        1..60 => format!("{minutes}m"),
        60..1440 => format!("{}h", minutes / 60),
        _ => captured.format("%b %-d").to_string(),
    }
}

/// The heads-up counts down to the second, so it never reads "in 10 min"
/// eight minutes out.
fn countdown_label(event: &CalendarEvent, now: DateTime<Local>) -> String {
    let seconds = (event.start - now).num_seconds();
    let clock = event.start.format("%-I:%M %p");
    if seconds <= 0 {
        return format!("now · {clock}");
    }
    format!("in {}:{:02} · {clock}", seconds / 60, seconds % 60)
}

fn starts_label(event: &CalendarEvent, now: DateTime<Local>) -> String {
    let minutes = (event.start - now).num_minutes();
    let clock = event.start.format("%-I:%M %p");
    match minutes {
        ..0 => format!("now · started {clock}"),
        0 => format!("now · {clock}"),
        1..60 => format!("in {minutes} min · {clock}"),
        _ => clock.to_string(),
    }
}

/// The identity of one occurrence, so a repeating event is announced each time.
/// The heading an agenda day goes under.
fn day_label(day: chrono::NaiveDate, today: chrono::NaiveDate) -> String {
    match (day - today).num_days() {
        0 => "Today".into(),
        1 => "Tomorrow".into(),
        _ => day.format("%A, %b %-d").to_string(),
    }
}

fn time_label(event: &CalendarEvent) -> String {
    if event.all_day {
        "All day".into()
    } else {
        event.start.format("%-I:%M %p").to_string()
    }
}

/// Everything still to come, in order, each with its day's heading when it is
/// the first event of that day.
fn agenda(events: &[CalendarEvent], now: DateTime<Local>) -> Vec<(Option<String>, CalendarEvent)> {
    let mut coming = events
        .iter()
        .filter(|event| event.end > now)
        .cloned()
        .collect::<Vec<_>>();
    coming.sort_by_key(|event| (event.start, !event.all_day));
    let today = now.date_naive();
    let mut previous = None;
    coming
        .into_iter()
        .map(|event| {
            let day = event.start.date_naive().max(today);
            let heading = (previous != Some(day)).then(|| day_label(day, today));
            previous = Some(day);
            (heading, event)
        })
        .collect()
}

fn notice_key(event: &CalendarEvent) -> String {
    format!("{}@{}", event.id, event.start.to_rfc3339())
}

#[derive(Default)]
struct MenuState {
    events: Vec<CalendarEvent>,
    needs_permission: bool,
    announced: HashSet<String>,
    closed_at: Option<Instant>,
}

impl Global for MenuState {}

/// Keeps the calendar fresh and raises a notice beside `anchor` (the avatar's
/// bounds, when it is on screen) shortly before each event.
pub fn init(anchor: fn(&mut App) -> Option<(Bounds<Pixels>, Option<DisplayId>)>, cx: &mut App) {
    cx.spawn(async move |cx| {
        loop {
            let loaded = cx.background_spawn(async { crate::load_events() }).await;
            match loaded {
                Ok((state, events)) => cx.update(|cx| store_events(state, events, cx)),
                Err(error) => log::warn!("brainz: avatar calendar refresh failed: {error:#}"),
            }
            cx.background_executor().timer(CALENDAR_REFRESH).await;
        }
    })
    .detach();
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(NOTICE_CHECK).await;
            cx.update(|cx| {
                let Some((anchor, display)) = anchor(cx) else {
                    return;
                };
                let now = Local::now();
                let state = cx.default_global::<MenuState>();
                let due = state
                    .events
                    .iter()
                    .filter(|event| !event.all_day)
                    .find(|event| {
                        let minutes = (event.start - now).num_minutes();
                        (0..=NOTICE_LEAD_MINUTES).contains(&minutes)
                            && event.start > now
                            && !state.announced.contains(&notice_key(event))
                    })
                    .cloned();
                if let Some(event) = due {
                    state.announced.insert(notice_key(&event));
                    open_notice(event, anchor, display, cx);
                }
            });
        }
    })
    .detach();
}

fn store_events(state: LoadState, events: Vec<CalendarEvent>, cx: &mut App) {
    let menu = cx.default_global::<MenuState>();
    match state {
        LoadState::Ready => {
            menu.events = events;
            menu.needs_permission = false;
        }
        LoadState::NeedsPermission => menu.needs_permission = true,
        _ => {}
    }
}

/// Where a window of `size` sits beside the avatar: above it and flush with
/// its right edge, or below when there is no room above. Also says whether it
/// ended up above. Window positions are measured from the corner of the
/// window's own display, so only that display's size is needed.
fn beside(
    anchor: Bounds<Pixels>,
    size: Size<Pixels>,
    display: Option<DisplayId>,
    cx: &App,
) -> (Bounds<Pixels>, bool) {
    let screen = cx
        .displays()
        .into_iter()
        .find(|candidate| Some(candidate.id()) == display)
        .or_else(|| cx.primary_display())
        .map(|display| display.bounds().size);
    let mut origin = point(
        anchor.right() - size.width,
        anchor.top() - size.height - GAP,
    );
    let above = origin.y >= GAP * 4.;
    if !above {
        origin.y = anchor.bottom() + GAP;
    }
    if let Some(screen) = screen {
        origin.x = origin.x.min(screen.width - size.width - GAP).max(GAP);
    }
    (Bounds { origin, size }, above)
}

/// Brainz applies the theme's window background to every window whenever a
/// setting changes, which would turn a long-lived see-through window into an
/// opaque black box. This puts the transparency back each time.
pub fn keep_transparent<V: 'static>(
    window: &mut Window,
    cx: &mut Context<V>,
) -> gpui::Subscription {
    cx.observe_global_in::<settings::SettingsStore>(window, |_, window, _| {
        window.set_background_appearance(WindowBackgroundAppearance::Transparent);
    })
}

fn popup(bounds: Bounds<Pixels>, focus: bool, display_id: Option<DisplayId>) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: None,
        focus,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        display_id,
        window_background: WindowBackgroundAppearance::Transparent,
        ..Default::default()
    }
}

/// Opens the menu beside the avatar, or closes it when it is already open.
pub fn toggle(anchor: Bounds<Pixels>, display: Option<DisplayId>, cx: &mut App) {
    if let Some(existing) = cx
        .windows()
        .into_iter()
        .find_map(|window| window.downcast::<MenuView>())
    {
        existing
            .update(cx, |_, window, _| window.remove_window())
            .log_err();
        return;
    }
    if cx
        .default_global::<MenuState>()
        .closed_at
        .is_some_and(|closed_at| closed_at.elapsed() < REOPEN_GUARD)
    {
        return;
    }
    let brain = crate::github_sync::state(cx).and_then(|state| state.read(cx).repo().cloned());
    let myman = MyMan::detect_for(brain.as_deref());
    cx.spawn(async move |cx| {
        let captures = cx
            .background_spawn(async move {
                myman
                    .map(|myman| recent_captures(&myman))
                    .unwrap_or_default()
            })
            .await;
        cx.update(|cx| {
            let state = cx.default_global::<MenuState>();
            let events = state.events.clone();
            let needs_permission = state.needs_permission;
            let (bounds, above) = beside(
                anchor,
                Size {
                    width: MENU_WIDTH,
                    height: MENU_HEIGHT,
                },
                display,
                cx,
            );
            cx.open_window(popup(bounds, true, display), move |window, cx| {
                let view = cx.new(|cx| {
                    MenuView::new(brain, events, captures, above, needs_permission, window, cx)
                });
                let focus = view.read(cx).focus_handle(cx);
                window.focus(&focus, cx);
                window.activate_window();
                view
            })
            .log_err();
        });
    })
    .detach();
}

/// Whether the menu or a notice is on screen, for callers that track which
/// windows are only companions of the avatar.
pub fn owns(window: gpui::AnyWindowHandle) -> bool {
    window.downcast::<MenuView>().is_some() || window.downcast::<NoticeView>().is_some()
}

pub struct MenuView {
    brain: Option<PathBuf>,
    editor: Option<Entity<Editor>>,
    events: Vec<CalendarEvent>,
    captures: Vec<Capture>,
    /// Whether the menu sits above the avatar, and so grows upward from it.
    above: bool,
    needs_permission: bool,
    copied: Option<usize>,
    error: Option<String>,
    focus_handle: FocusHandle,
    _activation: gpui::Subscription,
    _transparency: gpui::Subscription,
}

impl MenuView {
    fn new(
        brain: Option<PathBuf>,
        events: Vec<CalendarEvent>,
        captures: Vec<Capture>,
        above: bool,
        needs_permission: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let editor = brain.is_some().then(|| {
            cx.new(|cx| {
                let mut editor = Editor::single_line(window, cx);
                editor.set_placeholder_text("Capture a thought…  Enter saves to Inbox", window, cx);
                editor
            })
        });
        let activation = cx.observe_window_activation(window, |_, window, cx| {
            if !window.is_window_active() {
                cx.default_global::<MenuState>().closed_at = Some(Instant::now());
                window.remove_window();
            }
        });
        Self {
            brain,
            editor,
            events,
            captures,
            above,
            needs_permission,
            copied: None,
            error: None,
            focus_handle: cx.focus_handle(),
            _activation: activation,
            _transparency: keep_transparent(window, cx),
        }
    }

    fn confirm(&mut self, _: &menu::Confirm, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(brain), Some(editor)) = (&self.brain, &self.editor) else {
            return;
        };
        let text = editor.read(cx).text(cx);
        if text.trim().is_empty() {
            window.remove_window();
            return;
        }
        match crate::quick_capture::save_capture(brain, &text) {
            Ok(()) => window.remove_window(),
            Err(error) => {
                self.error = Some(format!("Couldn't save to Inbox: {error:#}"));
                cx.notify();
            }
        }
    }

    /// Asks macOS for calendar access, then fills the menu with what it allows.
    fn allow_calendar(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let loaded = cx.background_spawn(async { crate::request_events() }).await;
            this.update(cx, |this, cx| {
                match loaded {
                    Ok((state, events)) => {
                        store_events(state, events, cx);
                        let menu = cx.default_global::<MenuState>();
                        this.events = menu.events.clone();
                        this.needs_permission = menu.needs_permission;
                        this.error = None;
                    }
                    Err(error) => {
                        this.error = Some(format!("Couldn't read the calendar: {error:#}"));
                    }
                }
                cx.notify();
            })
            .log_err();
        })
        .detach();
    }

    /// Brings a Brainz window forward on its Calendar tab and closes the menu.
    fn open_calendar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let workspace = cx
            .windows()
            .into_iter()
            .find_map(|window| window.downcast::<workspace::MultiWorkspace>());
        if let Some(workspace) = workspace {
            cx.activate(true);
            workspace
                .update(cx, |_, window, cx| {
                    window.activate_window();
                    window.dispatch_action(Box::new(crate::OpenCalendar), cx);
                })
                .log_err();
        }
        window.remove_window();
    }

    fn copy(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(capture) = self.captures.get(index).cloned() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let copied = cx
                .background_spawn(async move { clipboard_item(&capture) })
                .await;
            this.update(cx, |this, cx| {
                match copied {
                    Ok(Some(item)) => {
                        cx.write_to_clipboard(item);
                        this.copied = Some(index);
                        this.error = None;
                    }
                    Ok(None) => {
                        this.copied = Some(index);
                        this.error = None;
                    }
                    Err(error) => {
                        this.copied = None;
                        this.error = Some(format!("Couldn't copy: {error:#}"));
                    }
                }
                cx.notify();
            })
            .log_err();
        })
        .detach();
    }
}

/// A screenshot becomes an image on the clipboard. A recording is put there
/// as a file, which this returns `None` for because it writes the pasteboard
/// itself: gpui cannot place file references on the clipboard.
#[allow(clippy::disallowed_methods, reason = "runs on the background executor")]
fn clipboard_item(capture: &Capture) -> Result<Option<ClipboardItem>> {
    match capture.kind {
        CaptureKind::Screenshot => {
            let format = match capture
                .file
                .extension()
                .and_then(|extension| extension.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref()
            {
                Some("jpg" | "jpeg") => ImageFormat::Jpeg,
                _ => ImageFormat::Png,
            };
            let bytes = std::fs::read(&capture.file)
                .with_context(|| format!("reading {}", capture.file.display()))?;
            Ok(Some(ClipboardItem::new_image(&Image::from_bytes(
                format, bytes,
            ))))
        }
        CaptureKind::Recording => {
            let path = capture
                .file
                .to_str()
                .context("recording path is not valid text")?
                .replace('\\', "\\\\")
                .replace('"', "\\\"");
            let output = std::process::Command::new("/usr/bin/osascript")
                .arg("-e")
                .arg(format!("set the clipboard to (POSIX file \"{path}\")"))
                .output()
                .context("running osascript")?;
            if !output.status.success() {
                return Err(anyhow!(
                    "{}",
                    String::from_utf8_lossy(&output.stderr).trim().to_owned()
                ));
            }
            Ok(None)
        }
    }
}

impl Focusable for MenuView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match &self.editor {
            Some(editor) => editor.focus_handle(cx),
            None => self.focus_handle.clone(),
        }
    }
}

fn section(title: &'static str) -> impl IntoElement {
    div()
        .h(px(30.))
        .px_1()
        .flex()
        .items_end()
        .pb_1()
        .child(Label::new(title).size(LabelSize::Small).color(Color::Muted))
}

impl Render for MenuView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        let now = Local::now();
        let coming = agenda(&self.events, now);
        let above = self.above;
        let nothing_to_show =
            self.editor.is_none() && self.events.is_empty() && self.captures.is_empty();
        let panel = v_flex()
            .key_context("BrainzQuickCapture")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(|_, _: &menu::Cancel, window, _| window.remove_window()))
            // Escape in the text field is the editor's Cancel, which it
            // passes on when it has nothing of its own to cancel.
            .on_action(
                cx.listener(|_, _: &editor::actions::Cancel, window, _| window.remove_window()),
            )
            // A press on the panel must not reach the see-through backdrop,
            // which closes the menu.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .relative()
            .w_full()
            .max_h_full()
            .overflow_hidden()
            .p_3()
            .rounded_xl()
            .border_1()
            .border_color(colors.border)
            .bg(colors.elevated_surface_background)
            .when_some(self.editor.clone(), |this, editor| {
                this.child(
                    h_flex()
                        .h(px(48.))
                        .px_1()
                        .gap_3()
                        .child(Icon::new(IconName::Pencil).color(Color::Accent))
                        .child(div().flex_1().child(editor)),
                )
            })
            .when(nothing_to_show, |this| {
                this.child(
                    div().h(px(40.)).px_1().flex().items_center().child(
                        Label::new("Open a brain in Brainz to capture thoughts here.")
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    ),
                )
            })
            .when(self.brain.is_some() || !self.events.is_empty(), |this| {
                this.child(
                    h_flex()
                        .h(px(30.))
                        .px_1()
                        .pb_1()
                        .items_end()
                        .justify_between()
                        .child(
                            Label::new("Coming up")
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        )
                        .when(!self.needs_permission, |this| {
                            this.child(
                                h_flex()
                                    .id("brainz-menu-open-calendar")
                                    .gap_1()
                                    .cursor_pointer()
                                    .opacity(0.7)
                                    .hover(|this| this.opacity(1.))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_calendar(window, cx)
                                    }))
                                    .child(
                                        Label::new("Calendar")
                                            .size(LabelSize::Small)
                                            .color(Color::Muted),
                                    )
                                    .child(
                                        Icon::new(IconName::ArrowUpRight)
                                            .size(IconSize::XSmall)
                                            .color(Color::Muted),
                                    ),
                            )
                        }),
                )
                .when(self.needs_permission, |this| {
                    this.child(
                        h_flex()
                            .id("brainz-menu-allow-calendar")
                            .h(px(44.))
                            .px_1()
                            .gap_3()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|this| this.bg(colors.element_hover))
                            .on_click(cx.listener(|this, _, _, cx| this.allow_calendar(cx)))
                            .child(
                                Icon::new(IconName::BrainzCalendar)
                                    .size(IconSize::Small)
                                    .color(Color::Muted),
                            )
                            .child(Label::new("Allow calendar access to see what's next")),
                    )
                })
                .when(!self.needs_permission && coming.is_empty(), |this| {
                    this.child(
                        div().h(px(44.)).px_1().flex().items_center().child(
                            Label::new("Nothing in the next week")
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        ),
                    )
                })
                .when(!self.needs_permission && !coming.is_empty(), |this| {
                    let today = Local::now().date_naive();
                    let mut day = String::new();
                    this.child(
                        h_flex()
                            .id("brainz-menu-event-tiles")
                            .w_full()
                            .gap_2()
                            .pb_1()
                            .overflow_x_scroll()
                            .children(coming.into_iter().enumerate().map(
                                |(index, (heading, event))| {
                                    if let Some(heading) = heading {
                                        day = heading;
                                    }
                                    let starts_today =
                                        !event.all_day && event.start.date_naive() <= today;
                                    let when = if starts_today {
                                        starts_label(&event, now)
                                    } else {
                                        time_label(&event)
                                    };
                                    v_flex()
                                        .id(("brainz-menu-event", index))
                                        .flex_none()
                                        .w(TILE_WIDTH)
                                        .h(px(84.))
                                        .p_2()
                                        .gap_0p5()
                                        .rounded_lg()
                                        .border_1()
                                        .border_color(colors.border_variant)
                                        .bg(colors.element_background)
                                        .cursor_pointer()
                                        .hover(|this| this.bg(colors.element_hover))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.open_calendar(window, cx)
                                        }))
                                        .child(
                                            h_flex()
                                                .gap_1p5()
                                                .child(
                                                    div()
                                                        .size(px(8.))
                                                        .flex_none()
                                                        .rounded_full()
                                                        .bg(event.color.unwrap_or(colors.border)),
                                                )
                                                .child(
                                                    Label::new(day.clone())
                                                        .size(LabelSize::XSmall)
                                                        .color(if day == "Today" {
                                                            Color::Accent
                                                        } else {
                                                            Color::Muted
                                                        })
                                                        .truncate(),
                                                ),
                                        )
                                        .child(
                                            Label::new(when)
                                                .size(LabelSize::XSmall)
                                                .color(Color::Muted)
                                                .truncate(),
                                        )
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(colors.text)
                                                .line_height(px(17.))
                                                .line_clamp(2)
                                                .child(event.title),
                                        )
                                },
                            )),
                    )
                })
            })
            .when(!self.captures.is_empty(), |this| {
                this.child(section("Recent from My Man")).children(
                    self.captures.iter().enumerate().map(|(index, capture)| {
                        let copied = self.copied == Some(index);
                        let group = SharedString::from(format!("brainz-capture-row-{index}"));
                        let file = capture.file.clone();
                        let preview = match (&capture.thumbnail, capture.kind) {
                            (Some(thumbnail), CaptureKind::Screenshot) => img(thumbnail.clone())
                                .w(px(48.))
                                .h(px(32.))
                                .rounded_sm()
                                .object_fit(ObjectFit::Cover)
                                .into_any_element(),
                            _ => div()
                                .w(px(48.))
                                .h(px(32.))
                                .rounded_sm()
                                .bg(colors.element_background)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    Icon::new(IconName::PlayOutlined)
                                        .size(IconSize::Small)
                                        .color(Color::Muted),
                                )
                                .into_any_element(),
                        };
                        h_flex()
                            .group(group.clone())
                            .h(px(44.))
                            .px_1()
                            .gap_3()
                            .rounded_md()
                            .hover(|this| this.bg(colors.element_hover))
                            .child(preview)
                            .child(
                                div()
                                    .flex_1()
                                    .overflow_hidden()
                                    .child(Label::new(capture.title.clone()).truncate()),
                            )
                            .child(if copied {
                                Label::new("Copied")
                                    .size(LabelSize::Small)
                                    .color(Color::Success)
                            } else {
                                Label::new(age(capture.captured, now))
                                    .size(LabelSize::Small)
                                    .color(Color::Muted)
                            })
                            .child(
                                h_flex()
                                    .gap_0p5()
                                    .visible_on_hover(group)
                                    .child(
                                        IconButton::new(
                                            ("brainz-capture-copy", index),
                                            IconName::Copy,
                                        )
                                        .icon_size(IconSize::Small)
                                        .tooltip(ui::Tooltip::text("Copy to clipboard"))
                                        .on_click(
                                            cx.listener(move |this, _, _, cx| this.copy(index, cx)),
                                        ),
                                    )
                                    .child(
                                        IconButton::new(
                                            ("brainz-capture-reveal", index),
                                            IconName::FolderOpen,
                                        )
                                        .icon_size(IconSize::Small)
                                        .tooltip(ui::Tooltip::text("Show in Finder"))
                                        .on_click(move |_, _, cx| cx.reveal_path(&file)),
                                    ),
                            )
                    }),
                )
            })
            .when_some(self.error.clone(), |this, error| {
                this.child(Label::new(error).size(LabelSize::Small).color(Color::Error))
            });
        div()
            .size_full()
            .flex()
            .flex_col()
            .when(above, |this| this.justify_end())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, window, _| window.remove_window()),
            )
            .child(panel.with_animation(
                "brainz-menu-open",
                Animation::new(MENU_OPEN).with_easing(ease_out_quint()),
                move |panel, delta| {
                    // Starts tucked toward the avatar and slides out of it.
                    let offset = (1. - delta) * MENU_SLIDE;
                    panel
                        .opacity(delta)
                        .top(px(if above { offset } else { -offset }))
                },
            ))
    }
}

fn open_notice(
    event: CalendarEvent,
    anchor: Bounds<Pixels>,
    display: Option<DisplayId>,
    cx: &mut App,
) {
    for window in cx.windows() {
        if let Some(notice) = window.downcast::<NoticeView>() {
            notice
                .update(cx, |_, window, _| window.remove_window())
                .log_err();
        }
    }
    let remaining = (event.start - Local::now())
        .to_std()
        .unwrap_or(Duration::ZERO)
        + Duration::from_secs(60);
    let (bounds, _) = beside(anchor, NOTICE_SIZE, display, cx);
    let Some(notice) = cx
        .open_window(popup(bounds, false, display), move |window, cx| {
            cx.new(|cx| NoticeView {
                event: Arc::new(event),
                alerted: false,
                _transparency: keep_transparent(window, cx),
                _tick: cx.spawn(async move |this, cx| {
                    loop {
                        cx.background_executor().timer(Duration::from_secs(1)).await;
                        let ticked = this.update(cx, |notice: &mut NoticeView, cx| {
                            if !notice.alerted
                                && urgency(&notice.event, Local::now()) == Urgency::Now
                            {
                                notice.alerted = true;
                                crate::quick_capture::alert_avatar(cx);
                            }
                            cx.notify();
                        });
                        if ticked.is_err() {
                            break;
                        }
                    }
                }),
            })
        })
        .log_err()
    else {
        return;
    };
    cx.spawn(async move |cx| {
        cx.background_executor().timer(remaining).await;
        // The notice may already be gone because it was clicked away.
        notice
            .update(cx, |_, window, _| window.remove_window())
            .ok();
    })
    .detach();
}

/// The heads-up shown beside the avatar before an event; a click dismisses it.
pub struct NoticeView {
    event: Arc<CalendarEvent>,
    /// The chime and shake play once, when the notice first turns red.
    alerted: bool,
    _transparency: gpui::Subscription,
    _tick: gpui::Task<()>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Urgency {
    Calm,
    /// Five minutes or less: the notice pulses yellow.
    Soon,
    /// A minute or less, or already started: the notice pulses red.
    Now,
}

fn urgency(event: &CalendarEvent, now: DateTime<Local>) -> Urgency {
    match (event.start - now).num_seconds() {
        ..=60 => Urgency::Now,
        61..=300 => Urgency::Soon,
        _ => Urgency::Calm,
    }
}

impl Render for NoticeView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        let now = Local::now();
        let urgency = urgency(&self.event, now);
        let tint = match urgency {
            Urgency::Calm => None,
            Urgency::Soon => Some(cx.theme().status().warning),
            Urgency::Now => Some(cx.theme().status().error),
        };
        let icon_color = match urgency {
            Urgency::Calm => Color::Accent,
            Urgency::Soon => Color::Warning,
            Urgency::Now => Color::Error,
        };
        let surface = colors.elevated_surface_background;
        let notice = h_flex()
            .id("brainz-event-notice")
            .size_full()
            .pl_2p5()
            .pr_5()
            .gap_2()
            .rounded_lg()
            .border_1()
            .border_color(tint.unwrap_or(colors.text_accent))
            .bg(surface)
            .child(
                Icon::new(IconName::BrainzCalendar)
                    .size(IconSize::Small)
                    .color(icon_color),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        Label::new(self.event.title.clone())
                            .size(LabelSize::Small)
                            .truncate(),
                    )
                    .child(
                        Label::new(countdown_label(&self.event, now))
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    ),
            );
        let dismiss = div()
            .id("brainz-event-notice-dismiss")
            .absolute()
            .top(px(7.))
            .right(px(7.))
            .size(px(16.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .border_1()
            .border_color(colors.border)
            .bg(colors.element_background)
            .cursor_pointer()
            .visible_on_hover("brainz-event-notice-frame")
            .hover(|this| this.bg(colors.element_hover))
            .tooltip(ui::Tooltip::text("Dismiss"))
            .on_click(|_, window, _| window.remove_window())
            .child(
                Icon::new(IconName::Dash)
                    .size(IconSize::XSmall)
                    .color(Color::Muted),
            );
        // The window is see-through and exactly the notice's size; the inset
        // keeps the rounded border's corners from being clipped by its edge.
        let frame = div()
            .group("brainz-event-notice-frame")
            .relative()
            .size_full()
            .p(px(3.));
        match tint {
            None => frame.child(notice.into_any_element()).child(dismiss),
            Some(tint) => frame
                .child(
                    notice
                        .with_animation(
                            // A new id per level restarts the pulse in the new colour.
                            ("brainz-event-notice-pulse", urgency as usize),
                            Animation::new(Duration::from_millis(2400))
                                .repeat()
                                .with_easing(pulsating_between(0.0, 1.0)),
                            move |notice, delta| {
                                notice
                                    .border_color(tint.opacity(0.45 + 0.55 * delta))
                                    .bg(surface.blend(tint.opacity(0.08 + 0.22 * delta)))
                            },
                        )
                        .into_any_element(),
                )
                .child(dismiss),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone as _;

    fn at(hour: u32, minute: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(2026, 11, 11, hour, minute, 0)
            .single()
            .expect("a valid local time")
    }

    fn event(id: &str, start: DateTime<Local>, minutes: i64, all_day: bool) -> CalendarEvent {
        CalendarEvent {
            id: id.into(),
            title: id.to_owned().into(),
            start,
            end: start + chrono::Duration::minutes(minutes),
            all_day,
            calendar: "Work".into(),
            attendees: 0,
            location: None,
            color: None,
            url: None,
            notes: None,
            attendee_names: Vec::new(),
            organizer: None,
            attendee_emails: Vec::new(),
        }
    }

    #[test]
    fn agenda_lists_what_is_left_under_one_heading_per_day() {
        let now = at(10, 0);
        let events = [
            event("tomorrow", at(9, 0) + chrono::Duration::days(1), 30, false),
            event("over", at(8, 0), 30, false),
            event("later", at(15, 0), 30, false),
            event("running", at(9, 45), 30, false),
            event("friday", at(9, 0) + chrono::Duration::days(2), 30, false),
        ];
        let listed = agenda(&events, now)
            .into_iter()
            .map(|(heading, event)| (heading, event.id))
            .collect::<Vec<_>>();
        assert_eq!(
            listed,
            vec![
                (Some("Today".to_owned()), "running".to_owned()),
                (None, "later".to_owned()),
                (Some("Tomorrow".to_owned()), "tomorrow".to_owned()),
                (Some("Friday, Nov 13".to_owned()), "friday".to_owned()),
            ]
        );
    }

    #[test]
    fn screenshot_note_gives_file_thumbnail_and_summary() {
        let note = "---\nid: 6075\ncaptured: 2026-11-12T02:40:59.046Z\nfile: /Users/ada/Pictures/My Man/My Man 6075.png\nsummary: \"Screenshot showing a release checklist.\"\nthumbnail: \"\\/Users\\/ada\\/MyManBrain\\/assets\\/capture-thumbnails\\/6075.png\"\nocr_text: |-\n  file: not this one\n---\n\n# A release checklist\n";
        let capture = parse_capture(CaptureKind::Screenshot, note).expect("a capture");
        assert_eq!(
            capture.file,
            PathBuf::from("/Users/ada/Pictures/My Man/My Man 6075.png")
        );
        assert_eq!(
            capture.thumbnail,
            Some(PathBuf::from(
                "/Users/ada/MyManBrain/assets/capture-thumbnails/6075.png"
            ))
        );
        assert_eq!(capture.title, "a release checklist");
    }

    #[test]
    fn recording_note_is_titled_by_its_length() {
        let note = "---\nid: FA86\ncaptured: 2026-09-26T19:15:43.081Z\nfile: /Users/ada/Pictures/My Man/Recording.mov\nduration_seconds: 81\n---\n";
        let capture = parse_capture(CaptureKind::Recording, note).expect("a capture");
        assert_eq!(capture.title, "Recording · 1m 21s");
        assert_eq!(capture.thumbnail, None);
        assert!(parse_capture(CaptureKind::Recording, "# No front matter").is_none());
    }

    #[test]
    fn notice_turns_yellow_at_five_minutes_and_red_at_one() {
        let start = at(9, 10);
        let soon = event("soon", start, 30, false);
        let before = |seconds| start - chrono::Duration::seconds(seconds);
        assert_eq!(urgency(&soon, before(301)), Urgency::Calm);
        assert_eq!(urgency(&soon, before(300)), Urgency::Soon);
        assert_eq!(urgency(&soon, before(61)), Urgency::Soon);
        assert_eq!(urgency(&soon, before(60)), Urgency::Now);
        assert_eq!(
            urgency(&soon, start + chrono::Duration::seconds(30)),
            Urgency::Now
        );
    }

    #[test]
    fn countdown_ticks_by_the_second() {
        let start = at(9, 10);
        let soon = event("soon", start, 30, false);
        assert_eq!(
            countdown_label(&soon, start - chrono::Duration::seconds(462)),
            "in 7:42 · 9:10 AM"
        );
        assert_eq!(
            countdown_label(&soon, start - chrono::Duration::seconds(5)),
            "in 0:05 · 9:10 AM"
        );
        assert_eq!(countdown_label(&soon, start), "now · 9:10 AM");
    }

    #[test]
    fn labels_read_like_a_person_would_say_them() {
        let now = at(9, 0);
        assert_eq!(
            starts_label(&event("soon", at(9, 10), 30, false), now),
            "in 10 min · 9:10 AM"
        );
        assert_eq!(
            starts_label(&event("later", at(15, 0), 30, false), now),
            "3:00 PM"
        );
        assert_eq!(age(at(8, 58), now), "2m");
        assert_eq!(age(at(6, 0), now), "3h");
    }
}
