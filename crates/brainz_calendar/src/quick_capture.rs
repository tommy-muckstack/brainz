//! Brainz: quick capture. Control-Option-N anywhere on the Mac opens a small
//! box over every app; Enter adds the thought to the brain's `Inbox.md` under
//! today's date. The shortcut is a Carbon hot key, so it works while Brainz is
//! in the background, and the box is a non-activating panel, so capturing
//! does not bring Brainz's windows forward.

use std::path::Path;

use anyhow::Result;
use chrono::{DateTime, Local};

const INBOX: &str = "Inbox.md";

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

fn save_capture(brain: &Path, text: &str) -> Result<()> {
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
    use gpui::{
        App, AppContext as _, Bounds, Entity, FocusHandle, Focusable, Size,
        WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions, point, px,
    };
    use ui::prelude::*;
    use util::ResultExt as _;

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
