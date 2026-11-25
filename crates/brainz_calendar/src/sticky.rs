//! Brainz: stickies. Small notes that float over every app, like the avatar.
//! They are kept in `stickies.json` under the data directory, with their text,
//! where they sit, and whether they are shown. They only appear while the
//! avatar is on screen, and they take the avatar's colour, so changing the
//! avatar's colour recolours them too.

use std::path::PathBuf;

use gpui::{App, Global};
use serde::{Deserialize, Serialize};
use util::ResultExt as _;

/// Width and height a new sticky opens at, in points.
const DEFAULT_SIZE: (f32, f32) = (210., 176.);
/// How far each duplicate is nudged from the note it copies.
const DUPLICATE_OFFSET: f32 = 28.;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Sticky {
    pub id: u64,
    pub text: String,
    /// Whether its window should be on screen while the avatar is.
    pub shown: bool,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Serialize, Deserialize, Default, Debug, PartialEq)]
struct Saved {
    next_id: u64,
    stickies: Vec<Sticky>,
}

#[derive(Default)]
struct Stickies {
    saved: Saved,
}

impl Global for Stickies {}

fn file() -> PathBuf {
    paths::data_dir().join("stickies.json")
}

fn load() -> Saved {
    std::fs::read_to_string(file())
        .ok()
        .and_then(|text| serde_json::from_str(&text).log_err())
        .unwrap_or_default()
}

fn save(saved: &Saved) {
    match serde_json::to_string_pretty(saved) {
        Ok(text) => {
            std::fs::write(file(), text).log_err();
        }
        Err(error) => log::warn!("brainz: could not save stickies: {error}"),
    }
}

pub fn init(cx: &mut App) {
    cx.set_global(Stickies { saved: load() });
}

/// Every sticky, in the order they were made.
pub fn list(cx: &App) -> Vec<Sticky> {
    cx.try_global::<Stickies>()
        .map(|state| state.saved.stickies.clone())
        .unwrap_or_default()
}

/// Changes the saved notes, writes them to disk, and returns the result.
fn mutate<R>(cx: &mut App, change: impl FnOnce(&mut Saved) -> R) -> R {
    let state = cx.default_global::<Stickies>();
    let result = change(&mut state.saved);
    save(&state.saved);
    result
}

/// Makes a new sticky near the top of the primary screen, shown at once.
pub fn add(cx: &mut App) {
    let (x, y) = cx
        .primary_display()
        .map(|display| {
            let bounds = display.bounds();
            (
                f32::from(bounds.origin.x) + f32::from(bounds.size.width) - DEFAULT_SIZE.0 - 120.,
                f32::from(bounds.origin.y) + 120.,
            )
        })
        .unwrap_or((400., 120.));
    mutate(cx, |saved| {
        let offset = saved.stickies.len() as f32 * DUPLICATE_OFFSET;
        saved.next_id += 1;
        saved.stickies.push(Sticky {
            id: saved.next_id,
            text: String::new(),
            shown: true,
            x: x - offset,
            y: y + offset,
            width: DEFAULT_SIZE.0,
            height: DEFAULT_SIZE.1,
        });
    });
    cx.defer(sync);
}

/// Copies a sticky's text into a new note just beside it.
pub fn duplicate(id: u64, cx: &mut App) {
    mutate(cx, |saved| {
        let Some(original) = saved.stickies.iter().find(|sticky| sticky.id == id).cloned() else {
            return;
        };
        saved.next_id += 1;
        saved.stickies.push(Sticky {
            id: saved.next_id,
            shown: true,
            x: original.x + DUPLICATE_OFFSET,
            y: original.y + DUPLICATE_OFFSET,
            ..original
        });
    });
    cx.defer(sync);
}

/// Throws a sticky away, text and all.
pub fn delete(id: u64, cx: &mut App) {
    mutate(cx, |saved| saved.stickies.retain(|sticky| sticky.id != id));
    cx.defer(sync);
}

/// Shows or hides one sticky. A hidden sticky keeps its text and place.
pub fn set_shown(id: u64, shown: bool, cx: &mut App) {
    mutate(cx, |saved| {
        if let Some(sticky) = saved.stickies.iter_mut().find(|sticky| sticky.id == id) {
            sticky.shown = shown;
        }
    });
    cx.defer(sync);
}

fn set_text(id: u64, text: String, cx: &mut App) {
    mutate(cx, |saved| {
        if let Some(sticky) = saved.stickies.iter_mut().find(|sticky| sticky.id == id) {
            sticky.text = text;
        }
    });
}

fn set_place(id: u64, x: f32, y: f32, width: f32, height: f32, cx: &mut App) {
    mutate(cx, |saved| {
        if let Some(sticky) = saved.stickies.iter_mut().find(|sticky| sticky.id == id) {
            *sticky = Sticky {
                x,
                y,
                width,
                height,
                ..sticky.clone()
            };
        }
    });
}

/// Opens the windows for shown stickies while the avatar is up, and closes
/// the rest. Called whenever the avatar appears or goes, or a sticky changes.
pub fn sync(cx: &mut App) {
    #[cfg(target_os = "macos")]
    windows::sync(cx);
    #[cfg(not(target_os = "macos"))]
    let _ = cx;
}

/// Redraws every sticky, e.g. after the avatar's colour changes.
pub fn refresh(cx: &mut App) {
    #[cfg(target_os = "macos")]
    windows::refresh(cx);
    #[cfg(not(target_os = "macos"))]
    let _ = cx;
}

const BULLET: &str = "• ";

/// The byte offset where `new` gained one newline over `old`, if that is all
/// that changed. Typing the newline is the only way the text grows that way.
fn inserted_newline(old: &str, new: &str) -> Option<usize> {
    if new.len() != old.len() + 1 {
        return None;
    }
    let prefix = old
        .as_bytes()
        .iter()
        .zip(new.as_bytes())
        .take_while(|(left, right)| left == right)
        .count();
    let rest_matches = old.as_bytes()[prefix..] == new.as_bytes()[prefix + 1..];
    (new.as_bytes().get(prefix) == Some(&b'\n') && rest_matches).then_some(prefix)
}

/// Edits that turn typed "- " or "* " at the start of a line into a round
/// bullet, and carry a bullet onto the next line when Enter is pressed after
/// one. Pressing Enter on an empty bullet ends the list instead.
/// Each edit is `(start, end, replacement)` in byte offsets of `new`.
fn bullet_edits(old: &str, new: &str) -> Vec<(usize, usize, &'static str)> {
    let mut edits = Vec::new();
    let mut line_start = 0;
    for line in new.split('\n') {
        if line.starts_with("- ") || line.starts_with("* ") {
            edits.push((line_start, line_start + 1, "•"));
        }
        line_start += line.len() + 1;
    }
    if let Some(newline) = inserted_newline(old, new) {
        let before = &new[..newline];
        let previous_start = before.rfind('\n').map_or(0, |index| index + 1);
        let previous = &new[previous_start..newline];
        if previous == BULLET {
            edits.push((previous_start, newline, ""));
        } else if previous.starts_with(BULLET) {
            edits.push((newline + 1, newline + 1, BULLET));
        }
    }
    edits
}

#[cfg(test)]
mod tests {
    use super::bullet_edits;

    #[test]
    fn dash_and_star_become_bullets_only_at_the_start_of_a_line() {
        assert_eq!(
            bullet_edits("", "- a\n* b\nc - d"),
            vec![(0, 1, "•"), (4, 5, "•")]
        );
    }

    #[test]
    fn enter_after_a_bullet_starts_the_next_bullet() {
        assert_eq!(bullet_edits("• a", "• a\n"), vec![(6, 6, "• ")]);
    }

    #[test]
    fn enter_on_an_empty_bullet_ends_the_list() {
        assert_eq!(bullet_edits("• a\n• ", "• a\n• \n"), vec![(6, 10, "")]);
    }

    #[test]
    fn ordinary_typing_changes_nothing() {
        assert_eq!(bullet_edits("ab", "abc"), vec![]);
        assert_eq!(bullet_edits("plain", "plain\n"), vec![]);
    }
}

#[cfg(target_os = "macos")]
mod windows {
    use editor::{Editor, EditorEvent};
    use gpui::{
        AnyWindowHandle, App, AppContext as _, Bounds, Context, CursorStyle, Entity, Hsla,
        MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Point, Size, Subscription,
        TextStyleRefinement, Window, WindowBackgroundAppearance, WindowBounds, WindowKind,
        WindowOptions, point, px, rgb, size,
    };
    use settings::Settings as _;
    use ui::prelude::*;
    use util::ResultExt as _;

    use super::{Sticky, bullet_edits};

    /// How far the pointer must travel with the button down before a press
    /// on the grip becomes a drag.
    const DRAG_DISTANCE: f32 = 4.;
    /// The strip along the top that drags the note; the text never sits there.
    const GRIP_HEIGHT: f32 = 14.;
    /// The square in the bottom-right corner that resizes the note.
    const RESIZE_HANDLE: f32 = 16.;
    const MINIMUM_SIZE: Size<Pixels> = Size {
        width: px(140.),
        height: px(90.),
    };
    const INK: u32 = 0x2b2b2b;
    /// Ink for a body too dark for [`INK`].
    const LIGHT_INK: u32 = 0xf3f0e8;

    pub fn sync(cx: &mut App) {
        // Stickies live only while the avatar is on screen.
        let avatar_up = crate::quick_capture::avatar_showing(cx);
        let wanted: Vec<Sticky> = super::list(cx)
            .into_iter()
            .filter(|sticky| avatar_up && sticky.shown)
            .collect();
        let open = open_windows(cx);
        for (id, handle) in &open {
            if !wanted.iter().any(|sticky| sticky.id == *id) {
                handle
                    .update(cx, |_, window, _| window.remove_window())
                    .log_err();
            }
        }
        for sticky in wanted {
            if !open.iter().any(|(id, _)| *id == sticky.id) {
                open_sticky(sticky, cx);
            }
        }
    }

    pub fn refresh(cx: &mut App) {
        for (_, handle) in open_windows(cx) {
            handle.update(cx, |_, window, _| window.refresh()).log_err();
        }
    }

    fn open_windows(cx: &mut App) -> Vec<(u64, AnyWindowHandle)> {
        cx.windows()
            .into_iter()
            .filter_map(|window| {
                let handle = window.downcast::<StickyView>()?;
                let id = handle.read(cx).ok()?.id;
                Some((id, window))
            })
            .collect()
    }

    fn open_sticky(sticky: Sticky, cx: &mut App) {
        let bounds = Bounds {
            origin: point(px(sticky.x), px(sticky.y)),
            size: size(px(sticky.width), px(sticky.height)),
        };
        let display = cx.primary_display().map(|display| display.id());
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: None,
                focus: false,
                show: true,
                kind: WindowKind::PopUp,
                is_movable: true,
                display_id: display,
                window_background: WindowBackgroundAppearance::Transparent,
                ..Default::default()
            },
            move |window, cx| cx.new(|cx| StickyView::new(sticky, window, cx)),
        )
        .log_err();
    }

    /// One sticky note: a grip to drag by, the text, and a corner to resize.
    pub struct StickyView {
        id: u64,
        editor: Entity<Editor>,
        grip_pressed_at: Option<Point<Pixels>>,
        /// The text as of the last edit, to tell an Enter from other typing.
        last_text: String,
        resize_from: Option<(Point<Pixels>, Size<Pixels>)>,
        _edits: Subscription,
        _moved: Subscription,
    }

    impl StickyView {
        fn new(sticky: Sticky, window: &mut Window, cx: &mut Context<Self>) -> Self {
            let id = sticky.id;
            let editor = cx.new(|cx| {
                // Auto-height keeps the background see-through and the text
                // soft-wrapped, with no gutter, line numbers or scrollbars.
                let mut editor = Editor::auto_height_unbounded(1, window, cx);
                editor.set_show_gutter(false, cx);
                editor.set_show_line_numbers(false, cx);
                editor.set_show_vertical_scrollbar(false, cx);
                editor.set_show_horizontal_scrollbar(false, cx);
                editor.set_show_indent_guides(false, cx);
                editor.set_show_git_diff_gutter(false, cx);
                editor.set_show_code_actions(false, cx);
                editor.set_show_runnables(false, cx);
                editor.set_show_breakpoints(false, cx);
                editor.set_show_wrap_guides(false, cx);
                editor.set_soft_wrap();
                editor.set_placeholder_text("Write a note…", window, cx);
                editor.set_text(sticky.text.as_str(), window, cx);
                editor
            });
            let edits = cx.subscribe_in(
                &editor,
                window,
                move |this, editor, event: &EditorEvent, _, cx| {
                    if matches!(event, EditorEvent::BufferEdited) {
                        let text = editor.read(cx).text(cx);
                        let previous = std::mem::replace(&mut this.last_text, text.clone());
                        super::set_text(id, text.clone(), cx);
                        let edits = bullet_edits(&previous, &text);
                        if edits.is_empty() {
                            return;
                        }
                        // Deferred: the edit raises BufferEdited itself, and
                        // this event is still inside the editor's update.
                        let editor = editor.clone();
                        cx.defer(move |cx| {
                            editor.update(cx, |editor, cx| {
                                editor.edit(
                                    edits.into_iter().rev().map(|(start, end, replacement)| {
                                        (
                                            editor::MultiBufferOffset(start)
                                                ..editor::MultiBufferOffset(end),
                                            replacement,
                                        )
                                    }),
                                    cx,
                                );
                            });
                        });
                    }
                },
            );
            // Where it is dragged to, and how it is sized, is remembered.
            let moved = cx.observe_window_bounds(window, move |_, window, cx| {
                let bounds = window.bounds();
                super::set_place(
                    id,
                    f32::from(bounds.origin.x),
                    f32::from(bounds.origin.y),
                    f32::from(bounds.size.width),
                    f32::from(bounds.size.height),
                    cx,
                );
            });
            Self {
                id,
                editor,
                grip_pressed_at: None,
                last_text: sticky.text.clone(),
                resize_from: None,
                _edits: edits,
                _moved: moved,
            }
        }

        fn grip_press(&mut self, event: &MouseDownEvent, _: &mut Window, _: &mut Context<Self>) {
            self.grip_pressed_at = Some(event.position);
        }

        fn grip_drag(&mut self, event: &MouseMoveEvent, window: &mut Window, _: &mut Context<Self>) {
            let Some(pressed_at) = self.grip_pressed_at else {
                return;
            };
            if event.pressed_button != Some(MouseButton::Left) {
                return;
            }
            let moved = event.position - pressed_at;
            if moved.x.abs() > px(DRAG_DISTANCE) || moved.y.abs() > px(DRAG_DISTANCE) {
                self.grip_pressed_at = None;
                window.start_window_move();
            }
        }

        fn resize_press(&mut self, event: &MouseDownEvent, window: &mut Window, _: &mut Context<Self>) {
            self.resize_from = Some((event.position, window.bounds().size));
        }

        /// gpui has no native resize on macOS, so the note is resized here as
        /// the corner is dragged.
        fn resize_drag(&mut self, event: &MouseMoveEvent, window: &mut Window, _: &mut Context<Self>) {
            let Some((pressed_at, start_size)) = self.resize_from else {
                return;
            };
            if event.pressed_button != Some(MouseButton::Left) {
                self.resize_from = None;
                return;
            }
            let delta = event.position - pressed_at;
            window.resize(Size {
                width: (start_size.width + delta.x).max(MINIMUM_SIZE.width),
                height: (start_size.height + delta.y).max(MINIMUM_SIZE.height),
            });
        }
    }

    impl Render for StickyView {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            // The avatar's colour, so the notes always match it.
            let color: Hsla = rgb(crate::quick_capture::avatar_swatch()).into();
            let dark = color.l < 0.3;
            let mut edge = color;
            edge.l = if dark { color.l + 0.12 } else { color.l * 0.82 };
            let ink: Hsla = rgb(if dark { LIGHT_INK } else { INK }).into();

            let ui_settings = theme_settings::ThemeSettings::get_global(cx);
            let font_family = ui_settings.ui_font.family.clone();
            let font_size = ui_settings.ui_font_size(cx);
            self.editor.update(cx, |editor, _| {
                editor.set_text_style_refinement(TextStyleRefinement {
                    font_family: Some(font_family),
                    font_size: Some(font_size.into()),
                    color: Some(ink),
                    ..Default::default()
                });
            });

            let grip = div()
                .id(("brainz-sticky-grip", self.id))
                .w_full()
                .h(px(GRIP_HEIGHT))
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, cx.listener(Self::grip_press))
                .on_mouse_move(cx.listener(Self::grip_drag))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _, _, _| this.grip_pressed_at = None),
                );

            let resize = div()
                .id(("brainz-sticky-resize", self.id))
                .absolute()
                .right_0()
                .bottom_0()
                .size(px(RESIZE_HANDLE))
                .cursor(CursorStyle::ResizeUpLeftDownRight)
                .on_mouse_down(MouseButton::Left, cx.listener(Self::resize_press))
                .on_mouse_move(cx.listener(Self::resize_drag))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _, _, _| this.resize_from = None),
                );

            v_flex()
                .size_full()
                .relative()
                .bg(color)
                .border_1()
                .border_color(edge)
                .rounded_md()
                .shadow_md()
                .child(grip)
                .child(
                    div()
                        .id(("brainz-sticky-body", self.id))
                        .flex_1()
                        .min_h_0()
                        .px_3()
                        .pb_3()
                        .overflow_y_scroll()
                        .child(self.editor.clone()),
                )
                .child(resize)
        }
    }
}
