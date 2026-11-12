use std::time::Duration;

use gpui::{Animation, AnimationExt, Hsla};

use crate::prelude::*;

/// Brainz: the named colours offered wherever something can be tinted (chat
/// tabs, the floating avatar), tuned to the charcoal-and-amber theme.
pub const BRAINZ_TAB_COLORS: &[(&str, u32)] = &[
    ("Amber", 0xe3c568),
    ("Coral", 0xe8836f),
    ("Sage", 0x9cc48f),
    ("Sky", 0x7fb3d5),
    ("Lilac", 0xb59ad9),
    ("Rose", 0xd98fb0),
    ("Slate", 0x9aa5b1),
    ("Teal", 0x6fbfb4),
    ("Tangerine", 0xe9a35c),
    ("Indigo", 0x7c86d6),
    ("White", 0xf3f0e8),
    ("Black", 0x1c1b19),
];

/// Brainz: three dots bouncing in sequence. The one "working" indicator used
/// everywhere, so waiting always looks the same.
pub fn bouncing_dots(id: impl Into<SharedString>, color: Hsla) -> impl IntoElement {
    let id = id.into();
    // The row is tall enough for the full bounce, and the dots rest a little
    // below its middle so the peak lands in the top half rather than outside.
    h_flex()
        .h(px(16.))
        .items_center()
        .gap_1()
        .children((0..3usize).map(move |dot| {
            let dot_id = SharedString::from(format!("{id}-dot-{dot}"));
            div()
                .relative()
                .size(px(5.))
                .rounded_full()
                .bg(color)
                .with_animation(
                    dot_id,
                    Animation::new(Duration::from_millis(900)).repeat(),
                    move |element, delta| {
                        let phase = (delta + dot as f32 * 0.16) % 1.0;
                        let lift = (phase * std::f32::consts::PI * 2.0).sin().max(0.0);
                        element.top(px(2.0 - 3.5 * lift))
                    },
                )
        }))
}
