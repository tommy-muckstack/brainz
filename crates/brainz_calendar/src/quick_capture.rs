//! Brainz: quick capture. Control-Option-N anywhere on the Mac opens a small
//! box over every app; Enter adds the thought to the brain's `Inbox.md` under
//! today's date. The shortcut is a Carbon hot key, so it works while Brainz is
//! in the background, and the box is a non-activating panel, so capturing
//! does not bring Brainz's windows forward.

use std::path::Path;

use anyhow::Result;
use chrono::{DateTime, Local};

const INBOX: &str = "Inbox.md";

gpui::actions!(
    brainz_capture,
    [
        /// Opens the quick capture box, which saves a thought to Inbox.md.
        QuickCapture
    ]
);

/// Adds `text` to the inbox under a `## <date>` heading, creating the file or
/// the day's heading when needed. Other days' entries are left as they are.
fn append_capture(existing: &str, text: &str, now: DateTime<Local>) -> String {
    let heading = format!("## {}", now.format("%Y-%m-%d"));
    let line = format!(
        "- {} · {}",
        text.split_whitespace().collect::<Vec<_>>().join(" "),
        now.format("%-I:%M %p")
    );
    let mut note = if existing.trim().is_empty() {
        "# Inbox\n".to_owned()
    } else {
        existing.trim_end().to_owned() + "\n"
    };
    let found = note
        .lines()
        .collect::<Vec<_>>()
        .iter()
        .rposition(|existing| existing.trim_end() == heading);
    match found {
        Some(index) => {
            // The day's list ends at the next heading or the end of the file.
            let lines: Vec<&str> = note.lines().collect();
            let end = lines[index + 1..]
                .iter()
                .position(|line| line.starts_with('#'))
                .map_or(lines.len(), |offset| index + 1 + offset);
            let mut insert_at = end;
            while insert_at > index + 1 && lines[insert_at - 1].trim().is_empty() {
                insert_at -= 1;
            }
            let mut updated: Vec<String> = lines.iter().map(|line| (*line).to_owned()).collect();
            updated.insert(insert_at, line);
            note = updated.join("\n") + "\n";
        }
        None => {
            note.push('\n');
            note.push_str(&heading);
            note.push_str("\n\n");
            note.push_str(&line);
            note.push('\n');
        }
    }
    note
}

pub(crate) fn save_capture(brain: &Path, text: &str) -> Result<()> {
    let path = brain.join(INBOX);
    let existing = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };
    std::fs::write(&path, append_capture(&existing, text, Local::now()))?;
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn init(cx: &mut gpui::App) {
    native::init(cx);
}

#[cfg(not(target_os = "macos"))]
pub fn init(_: &mut gpui::App) {}

#[cfg(target_os = "macos")]
mod native {
    use std::{ffi::c_void, sync::OnceLock};

    use editor::Editor;
    use std::{
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
        time::Duration,
    };

    use gpui::{
        Animation, AnimationExt as _, App, AppContext as _, Bounds, Entity, FocusHandle, Focusable,
        Hsla, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Size,
        WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions, point, px, rgb, svg,
    };
    use ui::prelude::*;
    use util::ResultExt as _;
    use workspace::Workspace;

    type OsStatus = i32;
    type EventRef = *mut c_void;

    #[repr(C)]
    struct EventHotKeyId {
        signature: u32,
        id: u32,
    }

    #[repr(C)]
    struct EventTypeSpec {
        event_class: u32,
        event_kind: u32,
    }

    #[link(name = "Carbon", kind = "framework")]
    unsafe extern "C" {
        fn GetApplicationEventTarget() -> *mut c_void;
        fn InstallEventHandler(
            target: *mut c_void,
            handler: extern "C" fn(*mut c_void, EventRef, *mut c_void) -> OsStatus,
            type_count: u32,
            types: *const EventTypeSpec,
            user_data: *mut c_void,
            handler_ref: *mut *mut c_void,
        ) -> OsStatus;
        fn RegisterEventHotKey(
            key_code: u32,
            modifiers: u32,
            id: EventHotKeyId,
            target: *mut c_void,
            options: u32,
            hot_key_ref: *mut *mut c_void,
        ) -> OsStatus;
    }

    const KEYBOARD_EVENTS: u32 = u32::from_be_bytes(*b"keyb");
    const HOT_KEY_PRESSED: u32 = 5;
    const OPTION_KEY: u32 = 1 << 11;
    const CONTROL_KEY: u32 = 1 << 12;
    const N_KEY: u32 = 0x2D;
    const SIGNATURE: u32 = u32::from_be_bytes(*b"BrnZ");

    static PRESSED: OnceLock<smol::channel::Sender<()>> = OnceLock::new();

    extern "C" fn on_hot_key(_: *mut c_void, _: EventRef, _: *mut c_void) -> OsStatus {
        if let Some(sender) = PRESSED.get() {
            sender.try_send(()).ok();
        }
        0
    }

    pub(super) fn init(cx: &mut App) {
        cx.on_action(|_: &super::QuickCapture, cx| open(cx));
        // The avatar lives only while a workspace window does: a lone
        // floating panel would count as an open window, so closing the last
        // real window would neither quit Brainz nor let the Dock reopen it.
        cx.observe_new(|_: &mut Workspace, _, cx| cx.defer(show_avatar))
            .detach();
        cx.on_window_closed(|cx, _| {
            let only_capture_windows_remain = cx.windows().into_iter().all(|window| {
                window.downcast::<AvatarView>().is_some()
                    || window.downcast::<CaptureView>().is_some()
                    || crate::capture_menu::owns(window)
            });
            if only_capture_windows_remain {
                hide_avatar(cx);
            }
        })
        .detach();
        crate::capture_menu::init(avatar_bounds, cx);
        let (sender, receiver) = smol::channel::bounded(1);
        if PRESSED.set(sender).is_err() {
            return;
        }
        let types = [EventTypeSpec {
            event_class: KEYBOARD_EVENTS,
            event_kind: HOT_KEY_PRESSED,
        }];
        // SAFETY: Carbon calls `on_hot_key` on the main thread for the
        // lifetime of the app; the handler and hot key are never removed.
        let registered = unsafe {
            let target = GetApplicationEventTarget();
            let mut handler = std::ptr::null_mut();
            let mut hot_key = std::ptr::null_mut();
            InstallEventHandler(
                target,
                on_hot_key,
                1,
                types.as_ptr(),
                std::ptr::null_mut(),
                &mut handler,
            ) == 0
                && RegisterEventHotKey(
                    N_KEY,
                    CONTROL_KEY | OPTION_KEY,
                    EventHotKeyId {
                        signature: SIGNATURE,
                        id: 1,
                    },
                    target,
                    0,
                    &mut hot_key,
                ) == 0
        };
        if !registered {
            log::warn!("brainz: quick capture shortcut is taken by another app");
            return;
        }
        cx.spawn(async move |cx| {
            while receiver.recv().await.is_ok() {
                cx.update(open);
            }
        })
        .detach();
    }

    fn open(cx: &mut App) {
        let Some(brain) =
            crate::github_sync::state(cx).and_then(|state| state.read(cx).repo().cloned())
        else {
            return;
        };
        if let Some(existing) = cx
            .windows()
            .into_iter()
            .find_map(|window| window.downcast::<CaptureView>())
        {
            existing
                .update(cx, |_, window, _| window.activate_window())
                .log_err();
            return;
        }
        let Some(display) = cx.primary_display() else {
            return;
        };
        let size = Size {
            width: px(560.),
            height: px(64.),
        };
        let screen = display.bounds();
        let bounds = Bounds {
            origin: point(
                screen.origin.x + (screen.size.width - size.width) / 2.,
                screen.origin.y + screen.size.height * 0.28,
            ),
            size,
        };
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: None,
                focus: true,
                show: true,
                kind: WindowKind::PopUp,
                is_movable: false,
                display_id: Some(display.id()),
                window_background: WindowBackgroundAppearance::Transparent,
                ..Default::default()
            },
            move |window, cx| {
                let view = cx.new(|cx| CaptureView::new(brain, window, cx));
                let focus = view.read(cx).editor.focus_handle(cx);
                window.focus(&focus, cx);
                window.activate_window();
                view
            },
        )
        .log_err();
    }

    const AVATAR_WINDOW: Size<Pixels> = Size {
        width: px(96.),
        height: px(96.),
    };
    const AVATAR_MARGIN: Pixels = px(28.);
    /// Movement past this turns a press on the avatar into a drag.
    const AVATAR_DRAG_DISTANCE: Pixels = px(4.);
    /// One pass through the idle routine: bob, blinks, a look to each side.
    const AVATAR_LOOP: Duration = Duration::from_secs(12);
    const AVATAR_COLORS: &[u32] = &[0xf2f2f0, 0xf5c84b, 0xf59ab5, 0x8fc4f7, 0x9be0a8, 0xc3a6f5];
    const AVATAR_INK: u32 = 0x2b2b2b;

    /// Set by right-clicking the avatar; it stays away until Brainz restarts.
    static AVATAR_HIDDEN: AtomicBool = AtomicBool::new(false);
    static AVATAR_COLOR: AtomicUsize = AtomicUsize::new(0);

    fn avatar_color_file() -> std::path::PathBuf {
        paths::data_dir().join("avatar-color")
    }

    fn show_avatar(cx: &mut App) {
        if AVATAR_HIDDEN.load(Ordering::Relaxed)
            || cx
                .windows()
                .into_iter()
                .any(|window| window.downcast::<AvatarView>().is_some())
        {
            return;
        }
        let Some(display) = cx.primary_display() else {
            return;
        };
        if let Some(saved) = std::fs::read_to_string(avatar_color_file())
            .ok()
            .and_then(|saved| saved.trim().parse::<usize>().ok())
            .filter(|saved| *saved < AVATAR_COLORS.len())
        {
            AVATAR_COLOR.store(saved, Ordering::Relaxed);
        }
        let screen = display.bounds();
        let bounds = Bounds {
            origin: point(
                screen.origin.x + screen.size.width - AVATAR_WINDOW.width - AVATAR_MARGIN,
                screen.origin.y + screen.size.height - AVATAR_WINDOW.height - AVATAR_MARGIN * 3.,
            ),
            size: AVATAR_WINDOW,
        };
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: None,
                focus: false,
                show: true,
                kind: WindowKind::PopUp,
                is_movable: true,
                display_id: Some(display.id()),
                window_background: WindowBackgroundAppearance::Transparent,
                ..Default::default()
            },
            |_, cx| {
                cx.new(|_| AvatarView {
                    pressed_at: None,
                    dragged: false,
                    hovered: false,
                })
            },
        )
        .log_err();
    }

    fn avatar_bounds(cx: &mut App) -> Option<Bounds<Pixels>> {
        cx.windows().into_iter().find_map(|window| {
            window
                .downcast::<AvatarView>()?
                .update(cx, |_, window, _| window.bounds())
                .ok()
        })
    }

    fn hide_avatar(cx: &mut App) {
        for window in cx.windows() {
            if let Some(avatar) = window.downcast::<AvatarView>() {
                avatar
                    .update(cx, |_, window, _| window.remove_window())
                    .log_err();
            }
        }
    }

    /// Smoothly 0 → 1 → 0 across `start..end`, holding 1 between the ramps.
    fn hold(time: f32, start: f32, end: f32, ramp: f32) -> f32 {
        if time <= start || time >= end {
            return 0.;
        }
        let edge = ((time - start).min(end - time) / ramp).clamp(0., 1.);
        edge * edge * (3. - 2. * edge)
    }

    /// How open the eyes are at `time` seconds into the loop: 1 is open.
    fn eye_openness(time: f32) -> f32 {
        const BLINKS: [f32; 4] = [2.1, 6.4, 6.75, 10.3];
        const BLINK_SECONDS: f32 = 0.16;
        BLINKS
            .iter()
            .map(|blink| ((time - blink).abs() / (BLINK_SECONDS / 2.)).min(1.))
            .fold(1., f32::min)
    }

    /// The wink near the end of the loop closes only the left eye.
    fn wink(time: f32) -> f32 {
        hold(time, 11.0, 11.45, 0.12)
    }

    /// How far the eyebrows are raised at `time`, from 0 to 1: a quick lift
    /// on their own, a smaller one with each glance, and one with the wink.
    fn brow_lift(time: f32) -> f32 {
        hold(time, 0.8, 1.5, 0.2)
            .max(hold(time, 3.4, 5.2, 0.35) * 0.5)
            .max(hold(time, 7.2, 7.7, 0.18))
            .max(hold(time, 8.0, 9.5, 0.35) * 0.5)
            .max(wink(time))
    }

    /// Sideways glance at `time`: negative looks left, positive right.
    fn glance(time: f32) -> f32 {
        hold(time, 3.4, 5.2, 0.35) * -1. + hold(time, 8.0, 9.5, 0.35)
    }

    /// The floating character that opens the menu from any app.
    pub struct AvatarView {
        pressed_at: Option<Point<Pixels>>,
        dragged: bool,
        hovered: bool,
    }

    impl AvatarView {
        fn press(&mut self, event: &MouseDownEvent, _: &mut Window, _: &mut Context<Self>) {
            self.pressed_at = Some(event.position);
            self.dragged = false;
        }

        fn drag(&mut self, event: &MouseMoveEvent, window: &mut Window, _: &mut Context<Self>) {
            let Some(pressed_at) = self.pressed_at else {
                return;
            };
            if self.dragged || event.pressed_button != Some(MouseButton::Left) {
                return;
            }
            let moved = event.position - pressed_at;
            if moved.x.abs() > AVATAR_DRAG_DISTANCE || moved.y.abs() > AVATAR_DRAG_DISTANCE {
                self.dragged = true;
                window.start_window_move();
            }
        }

        fn release(&mut self, _: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
            if self.pressed_at.take().is_some() && !self.dragged {
                let anchor = window.bounds();
                cx.defer(move |cx| crate::capture_menu::toggle(anchor, cx));
            }
        }

        fn pick_color(&mut self, index: usize, cx: &mut Context<Self>) {
            AVATAR_COLOR.store(index, Ordering::Relaxed);
            std::fs::write(avatar_color_file(), index.to_string()).log_err();
            cx.notify();
        }
    }

    impl Render for AvatarView {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let selected = AVATAR_COLOR.load(Ordering::Relaxed);
            let color: Hsla = rgb(AVATAR_COLORS.get(selected).copied().unwrap_or(0xf2f2f0)).into();
            let mut shade = color;
            shade.l *= 0.82;
            let ink: Hsla = rgb(AVATAR_INK).into();
            // The artwork is drawn on a 120 × 100 grid and shown at 0.65.
            let width = px(78.);
            let height = px(65.);
            let character = div()
                .id("brainz-avatar")
                .relative()
                .w(px(84.))
                .h(px(74.))
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, cx.listener(Self::press))
                .on_mouse_move(cx.listener(Self::drag))
                .on_mouse_up(MouseButton::Left, cx.listener(Self::release))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|_, _, window, _| {
                        AVATAR_HIDDEN.store(true, Ordering::Relaxed);
                        window.remove_window();
                    }),
                )
                .with_animation(
                    "brainz-avatar-idle",
                    Animation::new(AVATAR_LOOP).repeat(),
                    move |this, delta| {
                        let time = delta * AVATAR_LOOP.as_secs_f32();
                        let bob = (time / 4. * std::f32::consts::TAU).sin() * 1.5;
                        let look = glance(time) * 3.5;
                        let open = eye_openness(time);
                        let brows = brow_lift(time) * 2.5;
                        let top = 4. + bob;
                        let eye = |center: f32, open: f32| {
                            let eye_height = (13. * open).max(1.5);
                            div()
                                .absolute()
                                .left(px(3. + center * 0.65 - 3. + look))
                                .top(px(top + 62. * 0.65 - eye_height / 2.))
                                .w(px(6.))
                                .h(px(eye_height))
                                .rounded_full()
                                .bg(ink)
                        };
                        this.child(
                            svg()
                                .path("icons/brainz/avatar-body.svg")
                                .absolute()
                                .left(px(3.))
                                .top(px(top + 3.))
                                .w(width)
                                .h(height)
                                .text_color(shade),
                        )
                        .child(
                            svg()
                                .path("icons/brainz/avatar-body.svg")
                                .absolute()
                                .left(px(3.))
                                .top(px(top))
                                .w(width)
                                .h(height)
                                .text_color(color),
                        )
                        .child(
                            svg()
                                .path("icons/brainz/avatar-brows.svg")
                                .absolute()
                                .left(px(3. + look * 0.6))
                                .top(px(top - brows))
                                .w(width)
                                .h(height)
                                .text_color(ink),
                        )
                        .child(eye(45., open.min(1. - wink(time))))
                        .child(eye(75., open))
                    },
                );
            v_flex()
                .id("brainz-avatar-window")
                .size_full()
                .items_center()
                .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                    this.hovered = *hovered;
                    cx.notify();
                }))
                .child(character)
                .child(
                    h_flex()
                        .h(px(18.))
                        .gap_1()
                        .when(!self.hovered, |this| this.invisible())
                        .children(AVATAR_COLORS.iter().enumerate().map(|(index, swatch)| {
                            div()
                                .id(("brainz-avatar-color", index))
                                .size(px(11.))
                                .rounded_full()
                                .bg(rgb(*swatch))
                                .border_1()
                                .border_color(if index == selected {
                                    rgb(0xffffff)
                                } else {
                                    rgb(0x55554f)
                                })
                                .cursor_pointer()
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.pick_color(index, cx)),
                                )
                        })),
                )
        }
    }

    pub struct CaptureView {
        brain: std::path::PathBuf,
        editor: Entity<Editor>,
        focus_handle: FocusHandle,
        error: Option<String>,
        _activation: gpui::Subscription,
    }

    impl CaptureView {
        fn new(brain: std::path::PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
            let editor = cx.new(|cx| {
                let mut editor = Editor::single_line(window, cx);
                editor.set_placeholder_text("Capture a thought…  Enter saves to Inbox", window, cx);
                editor
            });
            // Clicking away is the same as Escape: the box only lives while in use.
            let activation = cx.observe_window_activation(window, |_, window, _| {
                if !window.is_window_active() {
                    window.remove_window();
                }
            });
            Self {
                brain,
                editor,
                focus_handle: cx.focus_handle(),
                error: None,
                _activation: activation,
            }
        }

        fn confirm(&mut self, _: &menu::Confirm, window: &mut Window, cx: &mut Context<Self>) {
            let text = self.editor.read(cx).text(cx);
            if text.trim().is_empty() {
                window.remove_window();
                return;
            }
            match super::save_capture(&self.brain, &text) {
                Ok(()) => window.remove_window(),
                Err(error) => {
                    self.error = Some(format!("Couldn't save to Inbox: {error:#}"));
                    cx.notify();
                }
            }
        }

        fn cancel(&mut self, _: &menu::Cancel, window: &mut Window, _: &mut Context<Self>) {
            window.remove_window();
        }
    }

    impl Focusable for CaptureView {
        fn focus_handle(&self, _: &App) -> FocusHandle {
            self.focus_handle.clone()
        }
    }

    impl Render for CaptureView {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let colors = cx.theme().colors();
            v_flex()
                .key_context("BrainzQuickCapture")
                .on_action(cx.listener(Self::confirm))
                .on_action(cx.listener(Self::cancel))
                // Escape in the text field is the editor's Cancel, which it
                // passes on when it has nothing of its own to cancel.
                .on_action(
                    cx.listener(|_, _: &editor::actions::Cancel, window, _| window.remove_window()),
                )
                .size_full()
                .justify_center()
                .px_4()
                .gap_1()
                .rounded_xl()
                .border_1()
                .border_color(colors.border)
                .bg(colors.elevated_surface_background)
                .shadow_lg()
                .child(
                    h_flex()
                        .gap_3()
                        .child(Icon::new(IconName::Pencil).color(Color::Accent))
                        .child(div().flex_1().child(self.editor.clone()))
                        .child(Label::new("⌃⌥N").size(LabelSize::Small).color(Color::Muted)),
                )
                .when_some(self.error.clone(), |this, error| {
                    this.child(Label::new(error).size(LabelSize::Small).color(Color::Error))
                })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone as _;

    fn at(day: u32, hour: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 10, day, hour, 5, 0).unwrap()
    }

    #[test]
    fn captures_start_an_inbox_and_group_by_day() {
        let first = append_capture("", "Call  Sam\nabout rent", at(22, 9));
        assert_eq!(
            first,
            "# Inbox\n\n## 2026-10-22\n\n- Call Sam about rent · 9:05 AM\n"
        );
        let second = append_capture(&first, "Buy stamps", at(22, 15));
        assert_eq!(
            second,
            "# Inbox\n\n## 2026-10-22\n\n- Call Sam about rent · 9:05 AM\n- Buy stamps · 3:05 PM\n"
        );
        let next_day = append_capture(&second, "Book flights", at(23, 8));
        assert!(
            next_day
                .ends_with("- Buy stamps · 3:05 PM\n\n## 2026-10-23\n\n- Book flights · 8:05 AM\n")
        );
    }

    #[test]
    fn a_days_list_grows_before_the_next_heading() {
        let existing =
            "# Inbox\n\n## 2026-10-22\n\n- First · 9:05 AM\n\n## Someday\n\n- Learn piano\n";
        assert_eq!(
            append_capture(existing, "Second", at(22, 10)),
            "# Inbox\n\n## 2026-10-22\n\n- First · 9:05 AM\n- Second · 10:05 AM\n\n## Someday\n\n- Learn piano\n"
        );
    }
}
