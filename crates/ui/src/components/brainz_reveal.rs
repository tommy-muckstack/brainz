use std::time::Duration;

use gpui::{Animation, AnimationExt, ElementId, ease_out_quint};

use crate::prelude::*;

/// How long one item takes to settle.
const REVEAL_MS: u64 = 320;
/// Each later item in a list starts this much later.
const STAGGER_MS: u64 = 36;
/// Items past this position all arrive together, so a long list never
/// takes seconds to finish.
const MAX_STAGGERED: usize = 10;

/// Brainz: fades and lifts an element into place the first time it is
/// shown, with later items in a list arriving a beat after earlier ones.
/// The one entrance used everywhere, so the app always moves the same way.
pub fn reveal(id: impl Into<ElementId>, index: usize, child: impl IntoElement) -> impl IntoElement {
    let position = index.min(MAX_STAGGERED) as u64;
    let delay_ms = position * STAGGER_MS;
    let total_ms = REVEAL_MS + delay_ms;
    let settle_at = delay_ms as f32 / total_ms as f32;
    div().relative().w_full().child(child).with_animation(
        id,
        Animation::new(Duration::from_millis(total_ms)),
        move |element, delta| {
            let progress = if settle_at >= 1.0 {
                1.0
            } else {
                ((delta - settle_at) / (1.0 - settle_at)).clamp(0.0, 1.0)
            };
            let eased = ease_out_quint()(progress);
            element.opacity(eased).top(px(10.0 * (1.0 - eased)))
        },
    )
}
