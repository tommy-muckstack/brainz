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

#[cfg(target_os = "macos")]
mod windows {
    use editor::{Editor, EditorEvent};
    use gpui::{
        AnyWindowHandle, App, AppContext as _, Bounds, Context, Entity, Hsla, MouseButton,
        MouseDownEvent, MouseMoveEvent, Pixels, Point, Subscription, Window, WindowBackgroundAppearance,
        WindowBounds, WindowKind, WindowOptions, point, px, rgb, size,
    };
    use ui::prelude::*;
    use util::ResultExt as _;

    use super::Sticky;

    /// How far the pointer must travel with the button down before a press
    /// on the header becomes a drag.
    const DRAG_DISTANCE: f32 = 4.;
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
                window_background: WindowBackgroundAppearance::Opaque,
                ..Default::default()
            },
            move |window, cx| cx.new(|cx| StickyView::new(sticky, window, cx)),
        )
        .log_err();
    }

    /// One sticky note: a header to drag by and buttons, then the text.
    pub struct StickyView {
        id: u64,
        editor: Entity<Editor>,
        pressed_at: Option<Point<Pixels>>,
        _edits: Subscription,
        _moved: Subscription,
    }

    impl StickyView {
        fn new(sticky: Sticky, window: &mut Window, cx: &mut Context<Self>) -> Self {
            let id = sticky.id;
            let editor = cx.new(|cx| {
                let mut editor = Editor::multi_line(window, cx);
                editor.set_placeholder_text("Write a note…", window, cx);
                editor.set_text(sticky.text.as_str(), window, cx);
                editor
            });
            let edits = cx.subscribe_in(
                &editor,
                window,
                move |_, editor, event: &EditorEvent, _, cx| {
                    if matches!(event, EditorEvent::BufferEdited) {
                        let text = editor.read(cx).text(cx);
                        super::set_text(id, text, cx);
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
                pressed_at: None,
                _edits: edits,
                _moved: moved,
            }
        }

        fn press(&mut self, event: &MouseDownEvent, _: &mut Window, _: &mut Context<Self>) {
            self.pressed_at = Some(event.position);
        }

        fn drag(&mut self, event: &MouseMoveEvent, window: &mut Window, _: &mut Context<Self>) {
            let Some(pressed_at) = self.pressed_at else {
                return;
            };
            if event.pressed_button != Some(MouseButton::Left) {
                return;
            }
            let moved = event.position - pressed_at;
            if moved.x.abs() > px(DRAG_DISTANCE) || moved.y.abs() > px(DRAG_DISTANCE) {
                self.pressed_at = None;
                window.start_window_move();
            }
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
            let id = self.id;

            let button = |label: &'static str, key: &'static str| {
                div()
                    .id((key, id))
                    .px_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .text_color(ink)
                    .hover(|style| style.bg(ink.opacity(0.12)))
                    .child(label)
            };

            let header = h_flex()
                .id(("brainz-sticky-header", id))
                .w_full()
                .justify_between()
                .px_2()
                .py_1()
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, cx.listener(Self::press))
                .on_mouse_move(cx.listener(Self::drag))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _, _, _| this.pressed_at = None),
                )
                .child(div().text_color(ink).child("Sticky"))
                .child(
                    h_flex()
                        .gap_1()
                        .child(
                            button("⧉", "brainz-sticky-copy").on_click(cx.listener(
                                move |_, _, _, cx| super::duplicate(id, cx),
                            )),
                        )
                        .child(
                            button("–", "brainz-sticky-hide").on_click(cx.listener(
                                move |_, _, _, cx| super::set_shown(id, false, cx),
                            )),
                        )
                        .child(
                            button("✕", "brainz-sticky-delete").on_click(cx.listener(
                                move |_, _, _, cx| super::delete(id, cx),
                            )),
                        ),
                );

            v_flex()
                .size_full()
                .bg(color)
                .border_1()
                .border_color(edge)
                .rounded_md()
                .shadow_md()
                .text_color(ink)
                .child(header)
                .child(div().flex_1().p_2().child(self.editor.clone()))
        }
    }
}
