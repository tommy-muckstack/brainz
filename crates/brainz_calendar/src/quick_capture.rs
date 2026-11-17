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

/// Whether the floating avatar is switched on.
#[cfg(target_os = "macos")]
pub fn avatar_enabled() -> bool {
    native::avatar_enabled()
}

#[cfg(not(target_os = "macos"))]
pub fn avatar_enabled() -> bool {
    false
}

/// A meeting is a minute away: a short chime, and the avatar shakes once.
#[cfg(target_os = "macos")]
pub fn alert_avatar(cx: &mut gpui::App) {
    native::alert_avatar(cx);
}

#[cfg(not(target_os = "macos"))]
pub fn alert_avatar(_: &mut gpui::App) {}

/// Shows or hides the floating avatar and remembers the choice.
#[cfg(target_os = "macos")]
pub fn toggle_avatar(cx: &mut gpui::App) {
    native::toggle_avatar(cx);
}

#[cfg(not(target_os = "macos"))]
pub fn toggle_avatar(_: &mut gpui::App) {}

#[cfg(target_os = "macos")]
mod native {
    use std::{ffi::c_void, sync::OnceLock};

    use editor::Editor;
    use std::{
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
        time::{Duration, Instant},
    };

    use gpui::{
        Animation, AnimationExt as _, App, AppContext as _, Bounds, Entity, FocusHandle, Focusable,
        Hsla, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Size,
        Transformation, WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions, point,
        px, rgb, size, svg,
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
        AVATAR_HIDDEN.store(avatar_off_file().exists(), Ordering::Relaxed);
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
        crate::capture_menu::init(avatar_window, cx);
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
        // Room above the character for its jump.
        height: px(118.),
    };
    const AVATAR_MARGIN: Pixels = px(28.);
    const AVATAR_SHAKE: Duration = Duration::from_millis(1400);
    const AVATAR_TRICK_EVERY: Duration = Duration::from_secs(60);
    const AVATAR_TRICK: Duration = Duration::from_millis(1300);
    const ALERT_SOUND: &str = "/System/Library/Sounds/Glass.aiff";
    /// Movement past this turns a press on the avatar into a drag.
    const AVATAR_DRAG_DISTANCE: Pixels = px(4.);
    /// One pass through the idle routine: a blink every four seconds or so,
    /// with glances around the room, brow lifts, a wink and a happy squint
    /// in between.
    const AVATAR_LOOP: Duration = Duration::from_secs(32);
    /// How far the eyes travel at a full glance, in points.
    const LOOK_X: f32 = 6.;
    const LOOK_Y: f32 = 3.5;
    /// How far a fully raised eyebrow lifts, in points.
    const BROW_LIFT: f32 = 4.;
    const AVATAR_COLORS: &[(&str, u32)] = ui::BRAINZ_TAB_COLORS;
    const AVATAR_DEFAULT_COLOR: &str = "White";
    const AVATAR_INK: u32 = 0x2b2b2b;
    /// Features drawn on a body too dark for the usual ink.
    const AVATAR_LIGHT_INK: u32 = 0xf3f0e8;

    /// Switched from the status bar and remembered across launches.
    static AVATAR_HIDDEN: AtomicBool = AtomicBool::new(false);

    fn avatar_off_file() -> std::path::PathBuf {
        paths::data_dir().join("avatar-off")
    }

    pub(super) fn avatar_enabled() -> bool {
        !AVATAR_HIDDEN.load(Ordering::Relaxed)
    }

    pub(super) fn toggle_avatar(cx: &mut App) {
        let hide = avatar_enabled();
        AVATAR_HIDDEN.store(hide, Ordering::Relaxed);
        if hide {
            std::fs::write(avatar_off_file(), "").log_err();
            hide_avatar(cx);
        } else {
            if let Err(error) = std::fs::remove_file(avatar_off_file())
                && error.kind() != std::io::ErrorKind::NotFound
            {
                log::warn!("brainz: could not remember the avatar is on: {error}");
            }
            show_avatar(cx);
        }
    }
    static AVATAR_COLOR: AtomicUsize = AtomicUsize::new(usize::MAX);

    /// The chosen colour's place in the palette; the default until one is
    /// picked, or when a saved name is no longer offered.
    fn avatar_color_index() -> usize {
        let index = AVATAR_COLOR.load(Ordering::Relaxed);
        if index < AVATAR_COLORS.len() {
            return index;
        }
        AVATAR_COLORS
            .iter()
            .position(|(name, _)| *name == AVATAR_DEFAULT_COLOR)
            .unwrap_or(0)
    }

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
        // Saved by name, so the choice survives the palette being reordered.
        if let Some(saved) = std::fs::read_to_string(avatar_color_file())
            .ok()
            .and_then(|saved| {
                AVATAR_COLORS
                    .iter()
                    .position(|(name, _)| *name == saved.trim())
            })
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
            |window, cx| {
                cx.new(|cx| AvatarView {
                    born: Instant::now(),
                    alerts: 0,
                    pressed_at: None,
                    dragged: false,
                    palette_open: false,
                    _transparency: crate::capture_menu::keep_transparent(window, cx),
                    // The notice is a child window and moves with the avatar;
                    // this re-places it once the avatar settles, e.g. flipping
                    // it below the avatar near the top of the screen.
                    _moved: cx.observe_window_bounds(window, |_, _, cx| {
                        cx.defer(crate::capture_menu::sync_notice)
                    }),
                })
            },
        )
        .log_err();
        cx.defer(crate::capture_menu::sync_notice);
    }

    pub(super) fn alert_avatar(cx: &mut App) {
        cx.background_spawn(async {
            smol::process::Command::new("/usr/bin/afplay")
                .arg(ALERT_SOUND)
                .status()
                .await
                .log_err();
        })
        .detach();
        for window in cx.windows() {
            if let Some(avatar) = window.downcast::<AvatarView>() {
                avatar
                    .update(cx, |avatar, _, cx| {
                        avatar.alerts += 1;
                        cx.notify();
                    })
                    .log_err();
            }
        }
    }

    fn avatar_window(cx: &mut App) -> Option<gpui::AnyWindowHandle> {
        cx.windows()
            .into_iter()
            .find(|window| window.downcast::<AvatarView>().is_some())
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
    /// Includes a double blink and a happy squint at the top of the loop.
    fn eye_openness(time: f32) -> f32 {
        const BLINKS: [f32; 9] = [2.2, 6.5, 10.8, 11.15, 15.6, 19.4, 23.8, 27.6, 30.6];
        const BLINK_SECONDS: f32 = 0.18;
        let blink = BLINKS
            .iter()
            .map(|blink| ((time - blink).abs() / (BLINK_SECONDS / 2.)).min(1.))
            .fold(1., f32::min);
        blink.min(1. - squint(time) * 0.45)
    }

    fn squint(time: f32) -> f32 {
        hold(time, 0.3, 1.6, 0.25)
    }

    /// The wink closes only the left eye.
    fn wink(time: f32) -> f32 {
        hold(time, 21.0, 21.55, 0.12)
    }

    /// Where the eyes look at `time`, each axis from -1 to 1: negative x is
    /// left, negative y is up. Left, up-right, a sweep from left to right,
    /// right, a curious look up-left, and a peek down-left.
    pub(super) fn look(time: f32) -> (f32, f32) {
        let up_right = hold(time, 8.0, 9.2, 0.3);
        let curious = hold(time, 24.5, 26.2, 0.35);
        let peek = hold(time, 28.0, 28.9, 0.3);
        let x = -hold(time, 4.0, 5.6, 0.3) + up_right * 0.7 - hold(time, 13.0, 13.9, 0.25)
            + hold(time, 13.7, 14.9, 0.25)
            + hold(time, 17.0, 18.6, 0.3)
            - curious * 0.6
            - peek * 0.5;
        let y = -up_right * 0.7 - curious * 0.5 + peek * 0.6;
        (x.clamp(-1., 1.), y)
    }

    /// How far each eyebrow is raised at `time`, left then right; negative
    /// lowers it. Both rise in surprise, with each glance and with the
    /// squint; the right one alone looks skeptical, the left one curious, and
    /// the winking eye's brow dips while the other lifts.
    fn brow_lift(time: f32) -> (f32, f32) {
        let (x, y) = look(time);
        let both = (hold(time, 10.0, 10.8, 0.2) * 0.9)
            .max(x.abs() * 0.45)
            .max(squint(time) * 0.7)
            - y.max(0.) * 0.4;
        let wink = wink(time);
        let left = (both + hold(time, 24.5, 26.2, 0.35) * 0.6) * (1. - wink) - wink * 0.6;
        let right = (both + hold(time, 19.8, 20.9, 0.25)).max(wink);
        (left, right)
    }

    /// The body's pose during a trick. A plain idle pose has `turn` 1,
    /// `side` 0 and no hop or squash.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub(super) struct Pose {
        /// Cosine of the turn: 1 faces you, -1 shows the back.
        pub turn: f32,
        /// Sine of the turn, which way the front is swinging.
        pub side: f32,
        /// A spin about the upright axis, else a forward flip.
        pub spin: bool,
        /// Height of the jump in points.
        pub hop: f32,
        /// Positive squashes the body wide and short, negative stretches it.
        pub squash: f32,
    }

    const REST: Pose = Pose {
        turn: 1.,
        side: 0.,
        spin: true,
        hop: 0.,
        squash: 0.,
    };
    /// How tall the jump is, in points; the window leaves this much room.
    const AVATAR_HOP: f32 = 12.;
    /// How deep the cloud is front to back, as a share of its width, so it
    /// never turns paper-thin edge-on.
    const AVATAR_DEPTH: f32 = 0.42;
    /// How far the face slides around the head as it turns, in points.
    const FACE_TRAVEL: f32 = 15.;
    /// How far the darker side shows past the front as it turns, in points.
    const SIDE_DEPTH: f32 = 6.;

    /// Once a minute the avatar does a trick, alternating a spin about its
    /// upright axis with a forward flip: it crouches, jumps, turns all the
    /// way round in the air, and squashes as it lands.
    pub(super) fn trick(elapsed: f32) -> Pose {
        use std::f32::consts::{PI, TAU};
        const CROUCH: f32 = 0.16;
        const LAND: f32 = 0.86;
        let period = AVATAR_TRICK_EVERY.as_secs_f32();
        let length = AVATAR_TRICK.as_secs_f32();
        let cycle = elapsed % (period * 2.);
        let (start, spin) = if cycle < period {
            (period - length, true)
        } else {
            (period * 2. - length, false)
        };
        let progress = (cycle - start) / length;
        if !(0.0..1.0).contains(&progress) {
            return REST;
        }
        let air = ((progress - CROUCH) / (LAND - CROUCH)).clamp(0., 1.);
        let eased = 0.5 - (air * PI).cos() / 2.;
        let angle = eased * TAU;
        let squash = if progress < CROUCH {
            (progress / CROUCH * PI).sin()
        } else if progress > LAND {
            (((progress - LAND) / (1. - LAND)) * PI).sin() * 0.8
        } else {
            -(air * PI).sin() * 0.35
        };
        Pose {
            turn: angle.cos(),
            side: angle.sin(),
            spin,
            hop: (air * PI).sin() * AVATAR_HOP,
            squash,
        }
    }

    /// The floating character that opens the menu from any app.
    pub struct AvatarView {
        /// Bumped for each meeting alert; a new value starts a new shake.
        alerts: usize,
        /// The tricks run on their own clock, independent of the idle loop.
        born: Instant,
        pressed_at: Option<Point<Pixels>>,
        dragged: bool,
        palette_open: bool,
        _transparency: gpui::Subscription,
        _moved: gpui::Subscription,
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
                let display = window.display(cx).map(|display| display.id());
                cx.defer(move |cx| crate::capture_menu::toggle(anchor, display, cx));
            }
        }

        fn pick_color(&mut self, index: usize, cx: &mut Context<Self>) {
            let Some((name, _)) = AVATAR_COLORS.get(index) else {
                return;
            };
            AVATAR_COLOR.store(index, Ordering::Relaxed);
            std::fs::write(avatar_color_file(), name).log_err();
            self.palette_open = false;
            cx.notify();
        }
    }

    impl Render for AvatarView {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let selected = avatar_color_index();
            let color: Hsla = rgb(AVATAR_COLORS
                .get(selected)
                .map_or(AVATAR_LIGHT_INK, |(_, color)| *color))
            .into();
            let dark = color.l < 0.3;
            let mut shade = color;
            // A dark body gets a lighter rim, or its edge would vanish.
            shade.l = if dark { color.l + 0.12 } else { color.l * 0.82 };
            let ink: Hsla = rgb(if dark { AVATAR_LIGHT_INK } else { AVATAR_INK }).into();
            // The artwork is drawn on a 120 × 100 grid and shown at 0.65.
            let width = px(78.);
            let height = px(65.);
            let born = self.born;
            let character = div()
                .id("brainz-avatar")
                .relative()
                .w(px(84.))
                .h(px(74. + AVATAR_HOP))
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, cx.listener(Self::press))
                .on_mouse_move(cx.listener(Self::drag))
                .on_mouse_up(MouseButton::Left, cx.listener(Self::release))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _, _, cx| {
                        this.palette_open = !this.palette_open;
                        cx.notify();
                    }),
                )
                .with_animation(
                    "brainz-avatar-idle",
                    Animation::new(AVATAR_LOOP).repeat(),
                    move |this, delta| {
                        let time = delta * AVATAR_LOOP.as_secs_f32();
                        let pose = trick(born.elapsed().as_secs_f32());
                        // A turning solid keeps its depth edge-on; the face
                        // rides round the curve, squeezing towards the edge
                        // and fading out before the back comes round.
                        let silhouette =
                            pose.turn.abs() + pose.side.abs() * AVATAR_DEPTH;
                        let (stretch_x, stretch_y) =
                            (1. + 0.14 * pose.squash, 1. - 0.14 * pose.squash);
                        let (scale_x, scale_y) = if pose.spin {
                            (silhouette * stretch_x, stretch_y)
                        } else {
                            (stretch_x, silhouette * stretch_y)
                        };
                        let body_scale = Transformation::scale(size(scale_x, scale_y));
                        let (turn_x, turn_y) = if pose.spin {
                            (pose.turn.max(0.) * stretch_x, stretch_y)
                        } else {
                            (stretch_x, pose.turn.max(0.) * stretch_y)
                        };
                        let (slide_x, slide_y) = if pose.spin {
                            (pose.side * FACE_TRAVEL, 0.)
                        } else {
                            (0., -pose.side * FACE_TRAVEL * 0.8)
                        };
                        let (side_x, side_y) = if pose.spin {
                            (-pose.side * SIDE_DEPTH, 0.)
                        } else {
                            (0., pose.side * SIDE_DEPTH)
                        };
                        let facing = pose.turn > 0.08;
                        let face_alpha = ((pose.turn - 0.08) / 0.3).clamp(0., 1.);
                        let ink = ink.opacity(face_alpha);
                        // The back is a touch darker than the front.
                        let front = if pose.turn < 0. { shade } else { color };
                        let bob = (time / 7.5 * std::f32::consts::TAU).sin() * 0.8;
                        // Keep the feet planted while squashing, then lift.
                        let planted = (1. - stretch_y) * 65. / 2. - pose.hop;
                        let (look_x, look_y) = look(time);
                        let look = look_x * LOOK_X;
                        let look_y = look_y * LOOK_Y;
                        let open = eye_openness(time);
                        let (left_brow, right_brow) = brow_lift(time);
                        let top = AVATAR_HOP + 4. + bob + planted;
                        let middle_x = 3. + 39.;
                        let middle_y = top + 32.5;
                        let eye = |center: f32, open: f32| {
                            let eye_height = (13. * open).max(1.5) * turn_y;
                            let eye_width = 6. * turn_x;
                            let x = middle_x
                                + slide_x
                                + (3. + center * 0.65 + look - middle_x) * turn_x;
                            let y = middle_y
                                + slide_y
                                + (top + 62. * 0.65 + look_y - middle_y) * turn_y;
                            div()
                                .absolute()
                                .left(px(x - eye_width / 2.))
                                .top(px(y - eye_height / 2.))
                                .w(px(eye_width))
                                .h(px(eye_height))
                                .rounded_full()
                                .bg(ink)
                        };
                        this.child(
                            svg()
                                .path("icons/brainz/avatar-body.svg")
                                .absolute()
                                .left(px(3. + side_x))
                                .top(px(top + 3. + side_y))
                                .w(width)
                                .h(height)
                                .text_color(shade)
                                .with_transformation(body_scale),
                        )
                        .child(
                            svg()
                                .path("icons/brainz/avatar-body.svg")
                                .absolute()
                                .left(px(3.))
                                .top(px(top))
                                .w(width)
                                .h(height)
                                .text_color(front)
                                .with_transformation(body_scale),
                        )
                        .when(facing, |this| {
                            let brow = |path: &'static str, lift: f32| {
                                svg()
                                    .path(path)
                                    .absolute()
                                    .left(px(3. + slide_x + look * 0.6 * turn_x))
                                    .top(px(
                                        top + slide_y
                                            + (look_y * 0.5 - lift * BROW_LIFT) * turn_y,
                                    ))
                                    .w(width)
                                    .h(height)
                                    .text_color(ink)
                                    .with_transformation(Transformation::scale(size(
                                        turn_x.max(0.02),
                                        turn_y.max(0.02),
                                    )))
                            };
                            this.child(brow("icons/brainz/avatar-brow-left.svg", left_brow))
                                .child(brow("icons/brainz/avatar-brow-right.svg", right_brow))
                                .child(eye(45., open.min(1. - wink(time))))
                                .child(eye(75., open))
                        })
                    },
                );
            let alerts = self.alerts;
            // Always wrapped, so an alert never restarts the idle routine.
            let character = div().relative().child(character).with_animation(
                ("brainz-avatar-alert", alerts),
                Animation::new(AVATAR_SHAKE),
                move |this, delta| {
                    if alerts == 0 {
                        return this;
                    }
                    let shake = (delta * std::f32::consts::TAU * 5.).sin() * 4. * (1. - delta);
                    this.left(px(shake))
                },
            );
            v_flex()
                .id("brainz-avatar-window")
                .size_full()
                .items_center()
                .child(character)
                .child(
                    h_flex()
                        .w(px(86.))
                        .flex_wrap()
                        .gap_1()
                        .when(!self.palette_open, |this| this.invisible())
                        .children(
                            AVATAR_COLORS
                                .iter()
                                .enumerate()
                                .map(|(index, (_, swatch))| {
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
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.pick_color(index, cx)
                                        }))
                                }),
                        ),
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

    #[cfg(target_os = "macos")]
    #[test]
    fn avatar_spins_then_flips_once_a_minute() {
        use native::trick;
        let rest = trick(10.);
        assert!(rest.turn == 1. && rest.hop == 0. && rest.squash == 0.);
        // Mid-spin: back showing, high in the air.
        let spin = trick(59.35);
        assert!(spin.spin && spin.turn < -0.9 && spin.hop > 11., "{spin:?}");
        // The crouch squashes before the jump.
        assert!(trick(58.8).squash > 0.5);
        let flip = trick(119.35);
        assert!(!flip.spin && flip.turn < -0.9, "{flip:?}");
        assert_eq!(trick(121.), trick(10.));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn avatar_looks_around_the_room() {
        use native::look;
        assert_eq!(look(3.), (0., 0.));
        assert!(look(4.8).0 < -0.99);
        let (x, y) = look(8.6);
        assert!(x > 0.6 && y < -0.6, "{x} {y}");
        assert!(look(13.4).0 < -0.9 && look(14.4).0 > 0.9);
        assert!(look(17.8).0 > 0.99);
    }

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
