use gpui::{
    AnyElement, App, AvailableSpace, Bounds, Element, ElementId, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, Pixels, Position, Style, Window, point, px,
    relative,
};

/// Brainz: pins a small control to the top-right corner of its parent, and
/// keeps it at the top of whatever part of the parent is currently visible
/// while the parent scrolls under it. The parent must be `relative()`.
pub struct StickyTopRight {
    child: AnyElement,
    inset: Pixels,
}

impl StickyTopRight {
    pub fn new(inset: impl Into<Pixels>, child: impl IntoElement) -> Self {
        Self {
            child: child.into_any_element(),
            inset: inset.into(),
        }
    }
}

impl IntoElement for StickyTopRight {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for StickyTopRight {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.position = Position::Absolute;
        style.inset.top = px(0.).into();
        style.inset.left = px(0.).into();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let child_size = self
            .child
            .layout_as_root(AvailableSpace::min_size(), window, cx);
        let visible_top = window.content_mask().bounds.top();
        let max_offset = (bounds.size.height - child_size.height - self.inset * 2.).max(px(0.));
        let offset = (visible_top - bounds.top()).clamp(px(0.), max_offset);
        let origin = point(
            bounds.right() - self.inset - child_size.width,
            bounds.top() + self.inset + offset,
        );
        self.child.prepaint_at(origin, window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}
