//! Brainz: the menu behind the floating avatar. It holds the quick capture
//! field, the next calendar events, and the newest My Man screenshots and
//! recordings ready to copy. It also keeps a small notice beside the avatar
//! from shortly before an event until it ends or is dismissed. The notice is a
//! child of the avatar's window, so it travels with the avatar when dragged.

use std::{
    cell::Cell,
    collections::HashSet,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, anyhow};
use chrono::{DateTime, Local};
use editor::Editor;
use gpui::{
    Animation, AnimationExt as _, AnyWindowHandle, App, AppContext as _, Bounds, ClipboardItem,
    DisplayId, Entity, FocusHandle, Focusable, Global, Hsla, Image, ImageFormat, MouseButton,
    ObjectFit, Pixels, Size, WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions,
    img, point, pulsating_between, px,
};
use ui::prelude::*;
use util::ResultExt as _;

use crate::{
    CalendarEvent, LoadState,
    avatar_help::{self, NextStep, StepAction},
    myman::MyMan,
    quick_capture::{self, Reaction},
};

const MENU_WIDTH: Pixels = px(340.);
/// The menu's window is this tall and see-through; the panel inside it hugs
/// the edge nearest the avatar, so it can grow without the window moving.
const MENU_HEIGHT: Pixels = px(560.);
/// The menu rolls out of the avatar over this long, and back in on close.
const MENU_OPEN: Duration = Duration::from_millis(280);
const MENU_CLOSE: Duration = Duration::from_millis(170);
/// How far the panel drifts toward the avatar as it rolls in and out.
const MENU_SLIDE: f32 = 10.;
const TILE_WIDTH: Pixels = px(136.);
/// A tile's padding on each side; the title scrolls within what is left.
const TILE_PADDING: Pixels = px(8.);
/// How fast a title too long for its tile scrolls past, in pixels a second.
const TITLE_SCROLL_SPEED: f32 = 28.;
/// A long title rests this long at each end before scrolling back.
const TITLE_SCROLL_REST: f32 = 1.6;
const NOTICE_WIDTH: Pixels = px(244.);
const NOTICE_HEIGHT: Pixels = px(50.);
/// A notice with buttons under its text.
const NOTICE_HEIGHT_WITH_BUTTONS: Pixels = px(84.);
/// How often the avatar rereads the to-do board and its suggestions.
const SNAPSHOT_REFRESH: Duration = Duration::from_secs(60);
const WRAP_UP_PROMPT: &str = "Help me wrap up today. Look at my to-do board, today's calendar, and the notes I changed today. Tell me what got done and what's still open, then suggest my top three for tomorrow.";
const GAP: Pixels = px(8.);
const CAPTURE_LIMIT: usize = 6;
/// Recent captures shown at once; the rest of the list scrolls.
const CAPTURES_VISIBLE: usize = 3;
const CAPTURE_ROW_HEIGHT: Pixels = px(44.);
/// Newest files read per My Man folder; names start with the capture date.
const CAPTURE_SCAN: usize = 40;
const CALENDAR_REFRESH: Duration = Duration::from_secs(5 * 60);
const NOTICE_CHECK: Duration = Duration::from_secs(15);
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

/// The newest screenshots and recordings My Man still has files for. Each
/// kind is capped on its own, so the far more numerous screenshots cannot
/// push recordings out of the list.
fn recent_captures(myman: &MyMan) -> Vec<Capture> {
    let mut captures = Vec::new();
    for (folder, kind) in [
        ("screenshots", CaptureKind::Screenshot),
        ("recordings", CaptureKind::Recording),
    ] {
        let mut of_kind = folder_captures(&myman.join(folder), kind);
        of_kind.retain(|capture| capture.file.is_file());
        of_kind.sort_by_key(|capture| std::cmp::Reverse(capture.captured));
        of_kind.truncate(CAPTURE_LIMIT);
        captures.extend(of_kind);
    }
    captures.sort_by_key(|capture| std::cmp::Reverse(capture.captured));
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
    if seconds <= -60 {
        return format!("started {clock}");
    }
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
    /// Notices that were dismissed; each later event still gets one.
    dismissed: HashSet<String>,
    /// Events the avatar already chimed for, so a notice that is hidden and
    /// shown again (behind the menu) does not chime twice.
    chimed: HashSet<String>,
    /// Meetings the avatar already greeted as they started.
    greeted: HashSet<String>,
    closed_at: Option<Instant>,
    avatar: Option<fn(&mut App) -> Option<AnyWindowHandle>>,
    snapshot: avatar_help::Snapshot,
    top_today: Option<String>,
    /// "Right now" steps already shown in the menu; read from disk once.
    seen: Option<HashSet<String>>,
}

impl Global for MenuState {}

/// Keeps the calendar and the to-do board fresh, and keeps a notice beside
/// `avatar` (the avatar's window, when it is on screen) for what is next.
pub fn init(avatar: fn(&mut App) -> Option<AnyWindowHandle>, cx: &mut App) {
    let state = cx.default_global::<MenuState>();
    state.avatar = Some(avatar);
    if let Ok(text) = std::fs::read_to_string(dismissed_steps_file()) {
        state.dismissed.extend(
            text.lines()
                .filter(|key| is_step_key(key))
                .map(str::to_owned),
        );
    }
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
            let brain = cx.update(|cx| {
                crate::github_sync::state(cx).and_then(|state| state.read(cx).repo().cloned())
            });
            if let Some(brain) = brain {
                let snapshot = cx
                    .background_spawn(async move { avatar_help::snapshot(&brain) })
                    .await;
                cx.update(|cx| {
                    let state = cx.default_global::<MenuState>();
                    state.top_today = snapshot.top_today.clone();
                    state.snapshot = snapshot;
                    quick_capture::refresh_avatar(cx);
                    sync_notice(cx);
                });
            }
            cx.background_executor().timer(SNAPSHOT_REFRESH).await;
        }
    })
    .detach();
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(NOTICE_CHECK).await;
            cx.update(sync_notice);
        }
    })
    .detach();
    // The menu closing brings the notice back; the avatar closing takes it away.
    cx.on_window_closed(|cx, _| cx.defer(sync_notice)).detach();
}

/// What the avatar suggests doing now; see [`avatar_help::next_steps`].
fn next_steps(cx: &mut App) -> Vec<NextStep> {
    let state = cx.default_global::<MenuState>();
    avatar_help::next_steps(
        &state.events,
        &state.snapshot,
        Local::now(),
        &state.dismissed,
    )
}

fn dismissed_steps_file() -> PathBuf {
    paths::data_dir().join("avatar-not-now")
}

fn is_step_key(key: &str) -> bool {
    ["prep:", "loop:", "todo:"]
        .iter()
        .any(|prefix| key.starts_with(prefix))
}

fn seen_file() -> PathBuf {
    paths::data_dir().join("avatar-seen")
}

/// Whether the menu holds "For you" items the menu has not shown yet.
pub fn has_unseen(cx: &mut App) -> bool {
    let steps = next_steps(cx);
    let MenuState { seen, .. } = cx.default_global::<MenuState>();
    let seen = seen.get_or_insert_with(|| {
        std::fs::read_to_string(seen_file())
            .map(|text| text.lines().map(str::to_owned).collect())
            .unwrap_or_default()
    });
    steps.iter().any(|step| !seen.contains(&step.key))
}

/// Remembers everything "For you" as shown, which clears the avatar's dot.
fn mark_seen(cx: &mut App) {
    let keys = next_steps(cx)
        .into_iter()
        .map(|step| step.key)
        .collect::<HashSet<_>>();
    let state = cx.default_global::<MenuState>();
    let text = keys.iter().cloned().collect::<Vec<_>>().join("\n");
    state.seen = Some(keys);
    std::fs::write(seen_file(), text).log_err();
    quick_capture::refresh_avatar(cx);
}

/// The event the notice is about: the soonest one starting within the lead
/// time, or else the latest one still going. Dismissed events are skipped.
fn notice_event<'a>(
    events: &'a [CalendarEvent],
    now: DateTime<Local>,
    dismissed: &HashSet<String>,
) -> Option<&'a CalendarEvent> {
    let lead = chrono::Duration::minutes(NOTICE_LEAD_MINUTES);
    let live = || {
        events.iter().filter(move |event| {
            !event.all_day
                && event.end > now
                && event.start - now <= lead
                && !dismissed.contains(&notice_key(event))
        })
    };
    live()
        .filter(|event| event.start > now)
        .min_by_key(|event| event.start)
        .or_else(|| live().max_by_key(|event| event.start))
}

/// What the card beside the avatar is about.
#[derive(Debug, Clone)]
enum NoticeKind {
    Meeting(Arc<CalendarEvent>),
    /// Just out of a meeting with time to spare, and the to-do to spend it on.
    Free {
        key: String,
        until: Option<DateTime<Local>>,
        todo: String,
    },
    /// The day's meetings are over.
    WrapUp {
        key: String,
    },
}

impl NoticeKind {
    fn key(&self) -> String {
        match self {
            Self::Meeting(event) => notice_key(event),
            Self::Free { key, .. } | Self::WrapUp { key } => key.clone(),
        }
    }
}

/// A meeting comes first; otherwise free time, if there is a to-do for it;
/// otherwise the end-of-day wrap-up.
fn pick_notice(
    events: &[CalendarEvent],
    top_today: Option<&str>,
    now: DateTime<Local>,
    dismissed: &HashSet<String>,
) -> Option<NoticeKind> {
    if let Some(event) = notice_event(events, now, dismissed) {
        return Some(NoticeKind::Meeting(Arc::new(event.clone())));
    }
    if let Some(todo) = top_today
        && let Some((key, until)) = avatar_help::free_time(events, now)
        && !dismissed.contains(&key)
    {
        return Some(NoticeKind::Free {
            key,
            until,
            todo: todo.to_owned(),
        });
    }
    avatar_help::wrap_up(events, now)
        .filter(|key| !dismissed.contains(key))
        .map(|key| NoticeKind::WrapUp { key })
}

#[derive(Debug, Clone, Default, PartialEq)]
struct NoticeActions {
    /// The call link, for Join.
    join: Option<String>,
    /// The meeting matches a folder in the brain, for Prep.
    prep: bool,
}

fn notice_actions(kind: &NoticeKind, cx: &App) -> NoticeActions {
    let NoticeKind::Meeting(event) = kind else {
        return NoticeActions::default();
    };
    NoticeActions {
        join: avatar_help::meeting_link(event),
        prep: crate::prep::state(cx)
            .and_then(|state| {
                state
                    .read(cx)
                    .banner()
                    .map(|banner| banner.event_id == event.id)
            })
            .unwrap_or(false),
    }
}

fn notice_size(kind: &NoticeKind, actions: &NoticeActions, cx: &App) -> Size<Pixels> {
    use settings::Settings as _;
    // The text follows the UI font size, so the card grows with it.
    let scale =
        (f32::from(theme_settings::ThemeSettings::get_global(cx).ui_font_size(cx)) / 16.).max(1.);
    let buttons = match kind {
        // The card itself joins the call, so only Prep needs a button.
        NoticeKind::Meeting(_) => actions.prep,
        NoticeKind::Free { .. } | NoticeKind::WrapUp { .. } => true,
    };
    let height = if buttons {
        NOTICE_HEIGHT_WITH_BUTTONS
    } else {
        NOTICE_HEIGHT
    };
    Size {
        width: (NOTICE_WIDTH * scale).round(),
        height: (height * scale).round(),
    }
}

/// Shows, updates, moves or removes the notice so it matches the calendar and
/// sits beside the avatar. It is hidden while the menu is open. The avatar
/// glances at a card when it appears.
pub fn sync_notice(cx: &mut App) {
    let windows = cx.windows();
    let notice = windows
        .iter()
        .find_map(|window| window.downcast::<NoticeView>());
    let menu_open = windows
        .iter()
        .any(|window| window.downcast::<MenuView>().is_some());
    let avatar = cx
        .default_global::<MenuState>()
        .avatar
        .and_then(|avatar| avatar(cx));
    let state = cx.default_global::<MenuState>();
    let kind = (!menu_open)
        .then(|| {
            pick_notice(
                &state.events,
                state.top_today.as_deref(),
                Local::now(),
                &state.dismissed,
            )
        })
        .flatten();
    let placement = avatar.zip(kind).and_then(|(avatar, kind)| {
        avatar
            .update(cx, |_, window, cx| {
                (
                    window.bounds(),
                    window.display(cx).map(|display| display.id()),
                )
            })
            .ok()
            .map(|(anchor, display)| (avatar, kind, anchor, display))
    });
    let Some((avatar, kind, anchor, display)) = placement else {
        if let Some(notice) = notice {
            notice
                .update(cx, |_, window, _| window.remove_window())
                .log_err();
        }
        return;
    };
    let actions = notice_actions(&kind, cx);
    let size = notice_size(&kind, &actions, cx);
    let (bounds, _) = beside(anchor, size, display, cx);
    let offset = bounds.origin - anchor.origin;
    // A notice that needs a new size, say a meeting card turning into the
    // taller wrap-up card, is reopened: resizing the window in place left
    // its contents laid out for the old size.
    let notice = notice.filter(|notice| {
        let fits = notice
            .update(cx, |_, window, _| {
                let current = window.bounds().size;
                (current.width - size.width).abs() < px(1.)
                    && (current.height - size.height).abs() < px(1.)
            })
            .unwrap_or(false);
        if !fits {
            notice
                .update(cx, |_, window, _| window.remove_window())
                .log_err();
        }
        fits
    });
    let appeared = match notice {
        Some(notice) => {
            let changed = notice
                .update(cx, |notice, _, cx| {
                    let changed = notice.kind.key() != kind.key();
                    if changed || notice.actions != actions {
                        notice.kind = kind;
                        notice.actions = actions;
                        cx.notify();
                    }
                    changed
                })
                .unwrap_or(false);
            native::place(avatar, notice.into(), offset, size, cx);
            changed
        }
        None => {
            let opened = cx
                .open_window(popup(bounds, false, display), move |window, cx| {
                    cx.new(|cx| NoticeView::new(kind, actions, window, cx))
                })
                .log_err();
            if let Some(notice) = opened {
                native::attach(avatar, notice.into(), offset, size, cx);
            }
            opened.is_some()
        }
    };
    if appeared {
        let dx = f32::from(offset.x + size.width / 2. - anchor.size.width / 2.);
        let dy = f32::from(offset.y + size.height / 2. - anchor.size.height / 2.);
        let length = (dx * dx + dy * dy).sqrt().max(1.);
        quick_capture::react(cx, Reaction::LookAt(dx / length, dy / length));
    }
}

/// Where the pointer is from the centre of `window`, in points with y
/// pointing down, wherever it is on screen.
pub fn cursor_offset(window: AnyWindowHandle, cx: &mut App) -> Option<(f32, f32)> {
    native::cursor_offset(window, cx)
}

/// Runs `f` in the main Brainz window, brought to the front.
fn in_workspace(cx: &mut App, f: impl FnOnce(&mut Window, &mut App)) {
    let Some(workspace) = cx
        .windows()
        .into_iter()
        .find_map(|window| window.downcast::<workspace::MultiWorkspace>())
    else {
        return;
    };
    cx.activate(true);
    workspace
        .update(cx, |_, window, cx| {
            window.activate_window();
            f(window, cx);
        })
        .log_err();
}

#[cfg(target_os = "macos")]
mod native {
    use cocoa::{
        base::{YES, id, nil},
        foundation::{NSPoint, NSRect, NSSize},
    };
    use gpui::{AnyWindowHandle, App, Pixels, Point, Size};
    use objc::{class, msg_send, sel, sel_impl};
    use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};

    const NS_WINDOW_ABOVE: i64 = 1;

    fn ns_window(handle: AnyWindowHandle, cx: &mut App) -> Option<id> {
        handle
            .update(cx, |_, window, _| {
                let RawWindowHandle::AppKit(appkit) = window.window_handle().ok()?.as_raw() else {
                    return None;
                };
                let view = appkit.ns_view.as_ptr() as id;
                // SAFETY: the view belongs to a live gpui window.
                let ns_window: id = unsafe { msg_send![view, window] };
                (ns_window != nil).then_some(ns_window)
            })
            .ok()
            .flatten()
    }

    /// Makes `child` a child window of `parent`, so AppKit moves it along
    /// whenever the parent moves, even mid-drag, then places it at `offset`
    /// from the parent's top-left corner.
    pub(super) fn attach(
        parent: AnyWindowHandle,
        child: AnyWindowHandle,
        offset: Point<Pixels>,
        size: Size<Pixels>,
        cx: &mut App,
    ) {
        let (Some(parent_window), Some(child_window)) =
            (ns_window(parent, cx), ns_window(child, cx))
        else {
            return;
        };
        // SAFETY: both windows are live; AppKit retains the child.
        unsafe {
            let _: () =
                msg_send![parent_window, addChildWindow: child_window ordered: NS_WINDOW_ABOVE];
        }
        place(parent, child, offset, size, cx);
    }

    /// Puts `child` at `offset` from `parent`'s top-left corner at `size`.
    /// Works in AppKit's global, bottom-up coordinates, so it holds on any
    /// display.
    pub(super) fn place(
        parent: AnyWindowHandle,
        child: AnyWindowHandle,
        offset: Point<Pixels>,
        size: Size<Pixels>,
        cx: &mut App,
    ) {
        let (Some(parent), Some(child)) = (ns_window(parent, cx), ns_window(child, cx)) else {
            return;
        };
        // SAFETY: both windows are live.
        unsafe {
            let frame: NSRect = msg_send![parent, frame];
            let current: NSRect = msg_send![child, frame];
            let (width, height) = (f64::from(size.width), f64::from(size.height));
            let target = NSRect::new(
                NSPoint::new(
                    frame.origin.x + f64::from(offset.x),
                    frame.origin.y + frame.size.height - f64::from(offset.y) - height,
                ),
                NSSize::new(width, height),
            );
            let moved = (target.origin.x - current.origin.x).abs() > 0.5
                || (target.origin.y - current.origin.y).abs() > 0.5
                || (target.size.width - current.size.width).abs() > 0.5
                || (target.size.height - current.size.height).abs() > 0.5;
            if moved {
                let _: () = msg_send![child, setFrame: target display: YES];
            }
        }
    }

    pub(super) fn cursor_offset(window: AnyWindowHandle, cx: &mut App) -> Option<(f32, f32)> {
        let window = ns_window(window, cx)?;
        // SAFETY: the window is live; mouseLocation is a class method.
        unsafe {
            let frame: NSRect = msg_send![window, frame];
            let mouse: NSPoint = msg_send![class!(NSEvent), mouseLocation];
            Some((
                (mouse.x - (frame.origin.x + frame.size.width / 2.)) as f32,
                ((frame.origin.y + frame.size.height / 2.) - mouse.y) as f32,
            ))
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod native {
    use gpui::{AnyWindowHandle, App, Pixels, Point, Size};

    pub(super) fn attach(
        _: AnyWindowHandle,
        _: AnyWindowHandle,
        _: Point<Pixels>,
        _: Size<Pixels>,
        _: &mut App,
    ) {
    }
    pub(super) fn place(
        _: AnyWindowHandle,
        _: AnyWindowHandle,
        _: Point<Pixels>,
        _: Size<Pixels>,
        _: &mut App,
    ) {
    }
    pub(super) fn cursor_offset(_: AnyWindowHandle, _: &mut App) -> Option<(f32, f32)> {
        None
    }
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
            .update(cx, |menu, window, cx| menu.close(window, cx))
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
            let steps = next_steps(cx);
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
                    MenuView::new(
                        brain,
                        events,
                        captures,
                        steps,
                        above,
                        needs_permission,
                        window,
                        cx,
                    )
                });
                let focus = view.read(cx).focus_handle(cx);
                window.focus(&focus, cx);
                window.activate_window();
                view
            })
            .log_err();
            sync_notice(cx);
            mark_seen(cx);
            // The avatar looks up (or down) at the menu as it opens.
            quick_capture::react(cx, Reaction::LookAt(-0.35, if above { -1. } else { 1. }));
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
    steps: Vec<NextStep>,
    /// Whether the menu sits above the avatar, and so grows upward from it.
    above: bool,
    needs_permission: bool,
    copied: Option<usize>,
    error: Option<String>,
    focus_handle: FocusHandle,
    /// Set once the menu starts rolling back into the avatar.
    closing: bool,
    /// The panel's natural height, measured each frame, which the roll-out
    /// reveals.
    panel_height: Rc<Cell<Pixels>>,
    _activation: gpui::Subscription,
    _transparency: gpui::Subscription,
}

impl MenuView {
    fn new(
        brain: Option<PathBuf>,
        events: Vec<CalendarEvent>,
        captures: Vec<Capture>,
        steps: Vec<NextStep>,
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
        let activation = cx.observe_window_activation(window, |this, window, cx| {
            if !window.is_window_active() {
                cx.default_global::<MenuState>().closed_at = Some(Instant::now());
                this.close(window, cx);
            }
        });
        Self {
            brain,
            editor,
            events,
            captures,
            steps,
            above,
            needs_permission,
            copied: None,
            error: None,
            focus_handle: cx.focus_handle(),
            closing: false,
            panel_height: Rc::new(Cell::new(Pixels::ZERO)),
            _activation: activation,
            _transparency: keep_transparent(window, cx),
        }
    }

    /// Rolls the panel back into the avatar, then closes the window.
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        self.closing = true;
        cx.notify();
        cx.spawn_in(window, async move |_, cx| {
            cx.background_executor().timer(MENU_CLOSE).await;
            cx.update(|window, _| window.remove_window()).log_err();
        })
        .detach();
    }

    fn confirm(&mut self, _: &menu::Confirm, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(brain), Some(editor)) = (&self.brain, &self.editor) else {
            return;
        };
        let text = editor.read(cx).text(cx);
        if text.trim().is_empty() {
            self.close(window, cx);
            return;
        }
        match crate::quick_capture::save_capture(brain, &text) {
            Ok(()) => {
                self.close(window, cx);
                cx.defer(|cx| quick_capture::react(cx, Reaction::Wink));
            }
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
        in_workspace(cx, |window, cx| {
            window.dispatch_action(Box::new(crate::OpenCalendar), cx)
        });
        window.remove_window();
    }

    /// Does a "Right now" step: prep, a message drafted with Claude, or the
    /// to-do board. The menu closes behind it.
    fn run_step(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(step) = self.steps.get(index).cloned() else {
            return;
        };
        let prompt = match step.action {
            StepAction::Prep { event_id, prompt } => {
                let has_prep = crate::prep::state(cx).is_some_and(|state| {
                    state
                        .read(cx)
                        .banner()
                        .is_some_and(|banner| banner.event_id == event_id)
                });
                if has_prep {
                    in_workspace(cx, |window, cx| {
                        if let Some(state) = crate::prep::state(cx) {
                            state.update(cx, |state, cx| state.open_for_event(window, cx));
                        }
                    });
                    None
                } else {
                    Some(prompt)
                }
            }
            StepAction::Claude { prompt } => Some(prompt),
            StepAction::OpenTodo => {
                in_workspace(cx, |window, cx| {
                    window.dispatch_action(Box::new(crate::todo::OpenTodo), cx)
                });
                None
            }
        };
        if let Some(text) = prompt {
            in_workspace(cx, |window, cx| {
                window.dispatch_action(Box::new(crate::prep::OpenClaudePrefilled { text }), cx)
            });
        }
        quick_capture::react(cx, Reaction::Cheer);
        self.close(window, cx);
    }

    /// Hides a "Right now" step for the rest of the session.
    fn dismiss_step(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.steps.len() {
            return;
        }
        let step = self.steps.remove(index);
        let state = cx.default_global::<MenuState>();
        state.dismissed.insert(step.key);
        // Remembered across restarts, so a stale follow-up stays gone.
        let text = state
            .dismissed
            .iter()
            .filter(|key| is_step_key(key))
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(dismissed_steps_file(), text).log_err();
        // Fill the gap with the next best step.
        self.steps = next_steps(cx);
        cx.notify();
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

/// Where a long title sits along its scroll at `time` seconds into a cycle,
/// from 0 (start showing) to 1 (end showing): it rests, glides to the end,
/// rests, and glides back.
fn title_scroll(time: f32, glide: f32) -> f32 {
    let rest = TITLE_SCROLL_REST;
    let linear = if time < rest {
        0.
    } else if time < rest + glide {
        (time - rest) / glide
    } else if time < rest * 2. + glide {
        1.
    } else {
        1. - ((time - rest * 2. - glide) / glide).min(1.)
    };
    linear * linear * (3. - 2. * linear)
}

/// A tile's one-line title. One too long for the tile scrolls sideways on
/// its own, so it can be read without wrapping.
fn tile_title(
    index: usize,
    title: SharedString,
    color: Hsla,
    window: &mut Window,
    cx: &App,
) -> AnyElement {
    use settings::Settings as _;
    let font = theme_settings::ThemeSettings::get_global(cx)
        .ui_font
        .clone();
    // The same size as the tile's other lines, so it follows the UI font
    // size instead of outgrowing the tile.
    let font_size = TextSize::XSmall.rems(cx).to_pixels(window.rem_size());
    // Room for emoji, which stand taller than letters.
    let line_height = (font_size * 1.45).round();
    let width = window
        .text_system()
        .shape_line(
            title.clone(),
            font_size,
            &[gpui::TextRun {
                len: title.len(),
                font,
                color,
                background_color: None,
                underline: None,
                strikethrough: None,
            }],
            None,
        )
        .width;
    let label = div()
        .flex_none()
        .text_size(font_size)
        .line_height(line_height)
        .text_color(color)
        .whitespace_nowrap()
        .child(title);
    let frame = div().w_full().h(line_height).overflow_hidden();
    let room = TILE_WIDTH - TILE_PADDING * 2. - px(2.);
    if width <= room {
        return frame.child(label).into_any_element();
    }
    let travel = f32::from(width - room) + 6.;
    let glide = (travel / TITLE_SCROLL_SPEED).max(0.6);
    let cycle = (TITLE_SCROLL_REST + glide) * 2.;
    frame
        .child(
            div()
                .relative()
                .w(width + px(4.))
                .child(label)
                .with_animation(
                    ("brainz-menu-event-title", index),
                    Animation::new(Duration::from_secs_f32(cycle)).repeat(),
                    move |this, delta| this.left(px(-travel * title_scroll(delta * cycle, glide))),
                ),
        )
        .into_any_element()
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        let now = Local::now();
        let coming = agenda(&self.events, now);
        let stickies = crate::sticky::list(cx);
        let no_stickies = stickies.is_empty();
        let above = self.above;
        let nothing_to_show =
            self.editor.is_none() && self.events.is_empty() && self.captures.is_empty();
        let panel = v_flex()
            .key_context("BrainzQuickCapture")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(|this, _: &menu::Cancel, window, cx| this.close(window, cx)))
            // Escape in the text field is the editor's Cancel, which it
            // passes on when it has nothing of its own to cancel.
            .on_action(
                cx.listener(|this, _: &editor::actions::Cancel, window, cx| this.close(window, cx)),
            )
            // A press on the panel must not reach the see-through backdrop,
            // which closes the menu.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .relative()
            .w_full()
            .flex_none()
            .max_h(MENU_HEIGHT - px(8.))
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
            .when(!self.steps.is_empty(), |this| {
                this.child(section("Right now"))
                    .children(self.steps.iter().enumerate().map(|(index, step)| {
                        let group = SharedString::from(format!("brainz-menu-step-{index}"));
                        h_flex()
                            .id(("brainz-menu-step", index))
                            .group(group.clone())
                            .py_1()
                            .px_1()
                            .gap_2()
                            .rounded_md()
                            .hover(|this| this.bg(colors.element_hover))
                            .child(
                                Icon::new(match step.action {
                                    StepAction::Prep { .. } => IconName::BrainzCalendar,
                                    StepAction::Claude { .. } => IconName::Sparkle,
                                    StepAction::OpenTodo => IconName::BrainzCheckboxEmpty,
                                })
                                .size(IconSize::Small)
                                .color(Color::Accent),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        Label::new(step.title.clone())
                                            .size(LabelSize::Small)
                                            .truncate(),
                                    )
                                    .child(
                                        Label::new(step.why.clone())
                                            .size(LabelSize::XSmall)
                                            .color(Color::Muted)
                                            .truncate(),
                                    ),
                            )
                            .child(
                                Button::new(("brainz-menu-step-run", index), step.button)
                                    .style(ButtonStyle::Filled)
                                    .size(ButtonSize::Compact)
                                    .label_size(LabelSize::Small)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.run_step(index, window, cx)
                                    })),
                            )
                            .child(
                                IconButton::new(("brainz-menu-step-drop", index), IconName::Close)
                                    .icon_size(IconSize::Small)
                                    .icon_color(Color::Muted)
                                    .visible_on_hover(group)
                                    .tooltip(ui::Tooltip::text("Not now"))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.dismiss_step(index, cx)
                                    })),
                            )
                    }))
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
                        ),
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
                    let happening_color = cx.theme().status().warning;
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
                                    // A meeting under way is outlined in yellow.
                                    let happening =
                                        !event.all_day && event.start <= now && now < event.end;
                                    let when = if starts_today {
                                        starts_label(&event, now)
                                    } else {
                                        time_label(&event)
                                    };
                                    v_flex()
                                        .id(("brainz-menu-event", index))
                                        .flex_none()
                                        .w(TILE_WIDTH)
                                        .p(TILE_PADDING)
                                        .gap_px()
                                        .rounded_lg()
                                        .border_1()
                                        .border_color(if happening {
                                            happening_color
                                        } else {
                                            colors.border_variant
                                        })
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
                                        .child(tile_title(
                                            index,
                                            event.title,
                                            colors.text,
                                            window,
                                            cx,
                                        ))
                                },
                            )),
                    )
                })
            })
            .when(!self.captures.is_empty(), |this| {
                this.child(section("Recent from My Man")).child(
                    // The newest few show; the rest scroll into view.
                    v_flex()
                        .id("brainz-captures")
                        .flex_none()
                        .max_h(CAPTURE_ROW_HEIGHT * CAPTURES_VISIBLE as f32)
                        .overflow_y_scroll()
                        .children(self.captures.iter().enumerate().map(|(index, capture)| {
                            let copied = self.copied == Some(index);
                            let group = SharedString::from(format!("brainz-capture-row-{index}"));
                            let file = capture.file.clone();
                            let preview = match (&capture.thumbnail, capture.kind) {
                                (Some(thumbnail), CaptureKind::Screenshot) => {
                                    img(thumbnail.clone())
                                        .w(px(48.))
                                        .h(px(32.))
                                        .rounded_sm()
                                        .object_fit(ObjectFit::Cover)
                                        .into_any_element()
                                }
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
                                .flex_none()
                                .h(CAPTURE_ROW_HEIGHT)
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
                                                cx.listener(move |this, _, _, cx| {
                                                    this.copy(index, cx)
                                                }),
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
                        })),
                )
            })
            .child(
                h_flex()
                    .h(px(30.))
                    .px_1()
                    .justify_between()
                    .items_end()
                    .pb_1()
                    .child(Label::new("Stickies").size(LabelSize::Small).color(Color::Muted))
                    .child(
                        IconButton::new("brainz-sticky-new", IconName::Plus)
                            .icon_size(IconSize::Small)
                            .tooltip(ui::Tooltip::text("New sticky"))
                            .on_click(cx.listener(|_, _, _, cx| crate::sticky::add(cx))),
                    ),
            )
            .when(no_stickies, |this| {
                this.child(
                    div().px_2().py_1().child(
                        Label::new("No stickies yet. Pin a note over your screen with +.")
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    ),
                )
            })
            .children(stickies.into_iter().map(|sticky| {
                let id = sticky.id;
                let shown = sticky.shown;
                let group = SharedString::from(format!("brainz-sticky-row-{id}"));
                let title = sticky
                    .text
                    .lines()
                    .map(str::trim)
                    .find(|line| !line.is_empty())
                    .unwrap_or("Empty sticky")
                    .to_owned();
                let title_color = if shown { Color::Default } else { Color::Muted };
                h_flex()
                    .id(("brainz-sticky-row", id))
                    .group(group.clone())
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .hover(|style| style.bg(cx.theme().colors().element_hover))
                    .child(
                        Label::new(title)
                            .size(LabelSize::Small)
                            .color(title_color)
                            .truncate()
                            .flex_1(),
                    )
                    .child(
                        h_flex()
                            .gap_0p5()
                            .visible_on_hover(group)
                            .child(
                                IconButton::new(("brainz-sticky-copy", id), IconName::Copy)
                                    .icon_size(IconSize::Small)
                                    .tooltip(ui::Tooltip::text("Duplicate"))
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        crate::sticky::duplicate(id, cx)
                                    })),
                            )
                            .child(
                                IconButton::new(("brainz-sticky-remove", id), IconName::Trash)
                                    .icon_size(IconSize::Small)
                                    .tooltip(ui::Tooltip::text("Delete"))
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        crate::sticky::delete(id, cx)
                                    })),
                            ),
                    )
                    .child(
                        IconButton::new(
                            ("brainz-sticky-toggle", id),
                            if shown { IconName::Eye } else { IconName::EyeOff },
                        )
                        .icon_size(IconSize::Small)
                        .tooltip(ui::Tooltip::text(if shown { "Hide" } else { "Show" }))
                        .on_click(cx.listener(move |_, _, _, cx| {
                            crate::sticky::set_shown(id, !shown, cx)
                        })),
                    )
            }))
            .when_some(self.error.clone(), |this, error| {
                this.child(Label::new(error).size(LabelSize::Small).color(Color::Error))
            });
        // The panel sits in a clip that grows from the avatar's side, so the
        // menu rolls out of the avatar like a dropdown and rolls back in.
        let measured = self.panel_height.clone();
        let clip = div()
            .flex()
            .flex_col()
            .relative()
            .flex_none()
            .w_full()
            .overflow_hidden()
            .when(above, |this| this.justify_end())
            .on_children_prepainted(move |bounds, window, _| {
                let height = bounds
                    .first()
                    .map_or(Pixels::ZERO, |bounds| bounds.size.height);
                if (measured.get() - height).abs() > px(0.5) {
                    measured.set(height);
                    window.refresh();
                }
            })
            .child(panel);
        let height = self.panel_height.clone();
        let roll = move |clip: Div, shown: f32| {
            let full = height.get();
            // Drifts in from the avatar's side as it unrolls.
            let drift = (1. - shown) * MENU_SLIDE;
            clip.h(full * shown)
                .opacity((shown * 1.6).min(1.))
                .top(px(if above { drift } else { -drift }))
        };
        let clip = if self.closing {
            clip.with_animation(
                "brainz-menu-close",
                Animation::new(MENU_CLOSE).with_easing(|t: f32| t * t),
                move |clip, delta| roll(clip, 1. - delta),
            )
            .into_any_element()
        } else {
            clip.with_animation(
                "brainz-menu-open",
                Animation::new(MENU_OPEN).with_easing(|t: f32| 1. - (1. - t).powi(3)),
                move |clip, delta| {
                    if delta >= 1. {
                        // Open: the panel takes whatever height it needs.
                        clip
                    } else {
                        roll(clip, delta)
                    }
                },
            )
            .into_any_element()
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .when(above, |this| this.justify_end())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.close(window, cx)),
            )
            .child(clip)
    }
}

/// The card kept beside the avatar: the next meeting, free time after one,
/// or the end-of-day wrap-up. Only its dismiss button closes it for good,
/// and only for that one card.
pub struct NoticeView {
    kind: NoticeKind,
    actions: NoticeActions,
    _transparency: gpui::Subscription,
    _tick: gpui::Task<()>,
}

impl NoticeView {
    fn new(
        kind: NoticeKind,
        actions: NoticeActions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            kind,
            actions,
            _transparency: keep_transparent(window, cx),
            _tick: cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    let ticked = this.update(cx, |notice: &mut NoticeView, cx| {
                        if let NoticeKind::Meeting(event) = &notice.kind {
                            let event = event.clone();
                            notice_tick(&event, Local::now(), cx);
                        }
                        cx.notify();
                    });
                    if ticked.is_err() {
                        break;
                    }
                }
            }),
        }
    }

    fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.default_global::<MenuState>()
            .dismissed
            .insert(self.kind.key());
        window.remove_window();
    }

    /// A click on a meeting card joins its call, or opens the Calendar tab
    /// when the invite has no call link.
    fn open_meeting(&mut self, cx: &mut Context<Self>) {
        match &self.actions.join {
            Some(link) => cx.open_url(link),
            None => in_workspace(cx, |window, cx| {
                window.dispatch_action(Box::new(crate::OpenCalendar), cx)
            }),
        }
    }

    fn prep(&mut self, cx: &mut Context<Self>) {
        in_workspace(cx, |window, cx| {
            if let Some(state) = crate::prep::state(cx) {
                state.update(cx, |state, cx| state.open_for_event(window, cx));
            }
        });
    }

    fn open_todo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        in_workspace(cx, |window, cx| {
            window.dispatch_action(Box::new(crate::todo::OpenTodo), cx)
        });
        self.dismiss(window, cx);
    }

    fn wrap_up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        in_workspace(cx, |window, cx| {
            window.dispatch_action(
                Box::new(crate::prep::OpenClaudePrefilled {
                    text: WRAP_UP_PROMPT.to_owned(),
                }),
                cx,
            )
        });
        self.dismiss(window, cx);
    }
}

/// Once a second while a meeting card is up: chime a minute before, and
/// greet you as it starts.
fn notice_tick(event: &CalendarEvent, now: DateTime<Local>, cx: &mut App) {
    let key = notice_key(event);
    let state = cx.default_global::<MenuState>();
    if urgency(event, now) == Urgency::Now && state.chimed.insert(key.clone()) {
        quick_capture::alert_avatar(cx);
        return;
    }
    let started = (now - event.start).num_seconds();
    if (0..30).contains(&started) && state.greeted.insert(key) {
        quick_capture::react(cx, Reaction::Greet);
    }
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
        // Once a meeting is well under way the notice stops pulsing.
        ..=-60 => Urgency::Calm,
        ..=60 => Urgency::Now,
        61..=300 => Urgency::Soon,
        _ => Urgency::Calm,
    }
}

impl Render for NoticeView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        let now = Local::now();
        let urgency = match &self.kind {
            NoticeKind::Meeting(event) => urgency(event, now),
            _ => Urgency::Calm,
        };
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
        let (icon, title, detail): (IconName, SharedString, SharedString) = match &self.kind {
            NoticeKind::Meeting(event) => (
                IconName::BrainzCalendar,
                event.title.clone(),
                countdown_label(event, now).into(),
            ),
            NoticeKind::Free { until, todo, .. } => (
                IconName::BrainzCheckboxEmpty,
                match until {
                    Some(until) => format!("Free until {}", until.format("%-I:%M %p")),
                    None => "Free for the rest of the day".to_owned(),
                }
                .into(),
                format!("Next up: {todo}").into(),
            ),
            NoticeKind::WrapUp { .. } => (
                IconName::Sparkle,
                "That was the last meeting today".into(),
                "Wrap up the day and plan tomorrow".into(),
            ),
        };
        let mut buttons = Vec::new();
        match &self.kind {
            NoticeKind::Meeting(_) => {
                if self.actions.prep {
                    buttons.push(
                        Button::new("brainz-notice-prep", "Prep")
                            .style(ButtonStyle::Subtle)
                            .size(ButtonSize::Compact)
                            .label_size(LabelSize::Small)
                            .tooltip(ui::Tooltip::text("Open the prep note or the folder"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                // Not a click on the card, which would join.
                                cx.stop_propagation();
                                this.prep(cx)
                            })),
                    );
                }
            }
            NoticeKind::Free { .. } => buttons.push(
                Button::new("brainz-notice-todo", "Open To-Do")
                    .style(ButtonStyle::Filled)
                    .size(ButtonSize::Compact)
                    .label_size(LabelSize::Small)
                    .on_click(cx.listener(|this, _, window, cx| this.open_todo(window, cx))),
            ),
            NoticeKind::WrapUp { .. } => buttons.push(
                Button::new("brainz-notice-wrap", "Wrap up")
                    .style(ButtonStyle::Filled)
                    .size(ButtonSize::Compact)
                    .label_size(LabelSize::Small)
                    .tooltip(ui::Tooltip::text(
                        "Open Claude with a wrap-up request, ready to send",
                    ))
                    .on_click(cx.listener(|this, _, window, cx| this.wrap_up(window, cx))),
            ),
        }
        let surface = colors.elevated_surface_background;
        let notice = v_flex()
            .id("brainz-event-notice")
            .size_full()
            .justify_center()
            .pl_2p5()
            .pr_5()
            .gap_1p5()
            .rounded_lg()
            .border_1()
            .border_color(tint.unwrap_or(colors.text_accent))
            .bg(surface)
            .when(matches!(self.kind, NoticeKind::Meeting(_)), |this| {
                let tip = if self.actions.join.is_some() {
                    "Join the call"
                } else {
                    "Open Calendar"
                };
                this.cursor_pointer()
                    .hover(|this| this.bg(colors.element_hover))
                    .tooltip(ui::Tooltip::text(tip))
                    .on_click(cx.listener(|this, _, _, cx| this.open_meeting(cx)))
            })
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .child(Icon::new(icon).size(IconSize::Small).color(icon_color))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(Label::new(title).size(LabelSize::Small).truncate())
                            .child(
                                Label::new(detail)
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted)
                                    .truncate(),
                            ),
                    ),
            )
            .when(!buttons.is_empty(), |this| {
                this.child(h_flex().pl(px(22.)).gap_1().children(buttons))
            });
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
            .on_click(cx.listener(|this, _, window, cx| this.dismiss(window, cx)))
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
    fn notice_follows_the_next_event_and_skips_dismissed_ones() {
        let first = event("first", at(9, 0), 30, false);
        let second = event("second", at(9, 30), 30, false);
        let events = vec![first.clone(), second];
        let mut dismissed = HashSet::new();
        let pick = |now, dismissed: &HashSet<String>| {
            notice_event(&events, now, dismissed).map(|event| event.id.clone())
        };
        assert_eq!(pick(at(8, 45), &dismissed), None);
        assert_eq!(pick(at(8, 52), &dismissed), Some("first".into()));
        // Still shown while the first meeting runs, until the next is close.
        assert_eq!(pick(at(9, 10), &dismissed), Some("first".into()));
        assert_eq!(pick(at(9, 22), &dismissed), Some("second".into()));
        dismissed.insert(notice_key(&first));
        assert_eq!(pick(at(8, 55), &dismissed), None);
        assert_eq!(pick(at(9, 25), &dismissed), Some("second".into()));
        assert_eq!(pick(at(10, 5), &dismissed), None);
    }

    #[test]
    fn a_meeting_beats_free_time_which_needs_a_to_do() {
        let standup = event("standup", at(9, 0), 30, false);
        let review = event("review", at(11, 0), 30, false);
        let events = vec![standup, review];
        let dismissed = HashSet::new();
        let kind = |now, todo| pick_notice(&events, todo, now, &dismissed).map(|kind| kind.key());
        // Just out of standup with the review well off: free time, if there
        // is something to do.
        assert!(
            kind(at(9, 35), Some("Write the brief")).is_some_and(|key| key.starts_with("free@"))
        );
        assert_eq!(kind(at(9, 35), None), None);
        // The review's notice wins once it is close.
        assert!(
            kind(at(10, 52), Some("Write the brief")).is_some_and(|key| key.starts_with("review@"))
        );
    }

    #[test]
    fn long_titles_rest_then_glide_both_ways() {
        let rest = TITLE_SCROLL_REST;
        assert_eq!(title_scroll(0.5, 2.), 0.);
        assert!((title_scroll(rest + 1., 2.) - 0.5).abs() < 1e-4);
        assert_eq!(title_scroll(rest + 2.5, 2.), 1.);
        assert!(title_scroll(rest * 2. + 4., 2.) < 1e-4);
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
        assert_eq!(
            countdown_label(&soon, start + chrono::Duration::minutes(5)),
            "started 9:10 AM"
        );
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
