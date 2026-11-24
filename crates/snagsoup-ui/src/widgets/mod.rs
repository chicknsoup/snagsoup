//! Reusable lightweight GPUI widgets for SnagSoup.
//!
//! Includes `Button`, `IconButton`, `NumericStepper`, `Slider`, `Popover`,
//! `ColorSwatchPicker`, and native GPUI `RichTooltip` (replacing WebView2).

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext, ClickEvent, Context, DragMoveEvent, ElementId, InteractiveElement,
    IntoElement, MouseButton, MouseDownEvent, ParentElement, Pixels, Render, RenderOnce,
    SharedString, StatefulInteractiveElement, Styled, Window, canvas, div, prelude::FluentBuilder,
    px, rgb,
};

use crate::icons::SnagIcon;
use crate::theme::{DEFAULT_SWATCHES, SnagTheme};

/// Visual style variant for [`Button`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonVariant {
    #[default]
    Secondary,
    Primary,
    Capture,
    Success,
    Ghost,
    Danger,
}

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// Native GPUI rich tooltip replacing Snagit's heavy WebView2 `EnhancedTooltips`.
pub struct RichTooltip {
    title: SharedString,
    description: Option<SharedString>,
    shortcut: Option<SharedString>,
}

impl RichTooltip {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            description: None,
            shortcut: None,
        }
    }

    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn shortcut(mut self, shortcut: impl Into<SharedString>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }
}

impl Render for RichTooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx
            .try_global::<SnagTheme>()
            .copied()
            .unwrap_or_else(SnagTheme::light);

        div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .max_w(px(260.0))
            .px(px(10.0))
            .py(px(8.0))
            .bg(theme.tooltip_bg)
            .border_1()
            .border_color(theme.tooltip_border)
            .rounded(theme.radius_md)
            .shadow_md()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.tooltip_text)
                            .child(self.title.clone()),
                    )
                    .when_some(self.shortcut.clone(), |el, shortcut| {
                        el.child(
                            div()
                                .px(px(5.0))
                                .py(px(1.0))
                                .rounded(theme.radius_sm)
                                .bg(theme.tooltip_border)
                                .text_xs()
                                .text_color(theme.tooltip_text)
                                .child(shortcut),
                        )
                    }),
            )
            .when_some(self.description.clone(), |el, desc| {
                el.child(
                    div()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child(desc),
                )
            })
    }
}

/// Reusable push button with label, optional icon, variant, and rich tooltip.
#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    label: SharedString,
    icon: Option<SnagIcon>,
    variant: ButtonVariant,
    disabled: bool,
    align_left: bool,
    tooltip: Option<(SharedString, Option<SharedString>)>,
    on_click: Option<ClickHandler>,
}

impl Button {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            icon: None,
            variant: ButtonVariant::Secondary,
            disabled: false,
            align_left: false,
            tooltip: None,
            on_click: None,
        }
    }

    pub fn icon(mut self, icon: SnagIcon) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn align_left(mut self) -> Self {
        self.align_left = true;
        self
    }

    pub fn tooltip(
        mut self,
        title: impl Into<SharedString>,
        description: Option<SharedString>,
    ) -> Self {
        self.tooltip = Some((title.into(), description));
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Button {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx
            .try_global::<SnagTheme>()
            .copied()
            .unwrap_or_else(SnagTheme::light);

        let (bg, hover_bg, fg, border_color) = match self.variant {
            ButtonVariant::Primary => (
                theme.accent_primary,
                theme.accent_primary_hover,
                theme.text_on_accent,
                theme.accent_primary,
            ),
            ButtonVariant::Capture => (
                theme.capture_red,
                theme.capture_red_hover,
                theme.text_on_accent,
                theme.capture_red,
            ),
            ButtonVariant::Success => (
                theme.success,
                gpui::Hsla::from(rgb(0x15803D)),
                theme.text_on_accent,
                theme.success,
            ),
            ButtonVariant::Secondary => (
                theme.surface_elevated,
                theme.surface_hover,
                theme.text_primary,
                theme.border,
            ),
            ButtonVariant::Ghost => (
                gpui::transparent_black(),
                theme.surface_hover,
                theme.text_primary,
                gpui::transparent_black(),
            ),
            ButtonVariant::Danger => (
                theme.danger,
                theme.capture_red_hover,
                theme.text_on_accent,
                theme.danger,
            ),
        };

        let mut base = div()
            .id(self.id)
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .h(px(30.0))
            .px(px(10.0))
            .rounded(theme.radius_md)
            .bg(bg)
            .border_1()
            .border_color(border_color)
            .text_sm()
            .text_color(fg);

        if self.align_left {
            base = base.w_full().justify_start();
        } else {
            base = base.justify_center();
        }

        if self.disabled {
            base = base.opacity(0.45);
        } else {
            base = base.cursor_pointer().hover(move |s| s.bg(hover_bg));
            if let Some(handler) = self.on_click {
                base = base.on_click(move |ev, window, cx| handler(ev, window, cx));
            }
        }

        if let Some((title, desc)) = self.tooltip {
            base = base.tooltip(move |_window, cx| {
                let title = title.clone();
                let desc = desc.clone();
                cx.new(move |_| {
                    let mut tip = RichTooltip::new(title);
                    if let Some(d) = desc {
                        tip = tip.description(d);
                    }
                    tip
                })
                .into()
            });
        }

        if let Some(icon) = self.icon {
            base = base.child(icon.render(px(15.0), fg));
        }

        base.child(self.label)
    }
}

/// Square icon-only toolbar button with active state and optional rich tooltip.
#[derive(IntoElement)]
pub struct IconButton {
    id: ElementId,
    icon: SnagIcon,
    size: Pixels,
    active: bool,
    disabled: bool,
    tooltip: Option<(SharedString, Option<SharedString>)>,
    on_click: Option<ClickHandler>,
}

impl IconButton {
    pub fn new(id: impl Into<ElementId>, icon: SnagIcon) -> Self {
        Self {
            id: id.into(),
            icon,
            size: px(32.0),
            active: false,
            disabled: false,
            tooltip: None,
            on_click: None,
        }
    }

    pub fn size(mut self, size: Pixels) -> Self {
        self.size = size;
        self
    }

    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn tooltip(
        mut self,
        title: impl Into<SharedString>,
        shortcut: Option<SharedString>,
    ) -> Self {
        self.tooltip = Some((title.into(), shortcut));
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for IconButton {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx
            .try_global::<SnagTheme>()
            .copied()
            .unwrap_or_else(SnagTheme::light);

        let bg = if self.active {
            theme.surface_active
        } else {
            gpui::transparent_black()
        };
        let fg = if self.active {
            theme.accent_primary
        } else {
            theme.text_primary
        };
        let border = if self.active {
            theme.accent_primary
        } else {
            gpui::transparent_black()
        };

        let mut el = div()
            .id(self.id)
            .flex()
            .items_center()
            .justify_center()
            .size(self.size)
            .rounded(theme.radius_md)
            .bg(bg)
            .border_1()
            .border_color(border)
            .child(self.icon.render(px(16.0), fg));

        if self.disabled {
            el = el.opacity(0.4);
        } else {
            let hover_bg = theme.surface_hover;
            el = el.cursor_pointer().hover(move |s| s.bg(hover_bg));
            if let Some(handler) = self.on_click {
                el = el.on_click(move |ev, window, cx| handler(ev, window, cx));
            }
        }

        if let Some((title, shortcut)) = self.tooltip {
            el = el.tooltip(move |_window, cx| {
                let title = title.clone();
                let shortcut = shortcut.clone();
                cx.new(move |_| {
                    let mut tip = RichTooltip::new(title);
                    if let Some(sc) = shortcut {
                        tip = tip.shortcut(sc);
                    }
                    tip
                })
                .into()
            });
        }

        el
    }
}

/// Compact numeric stepper (`- [ value unit ] +`) for tool widths, font sizes, and delays.
#[derive(IntoElement)]
pub struct NumericStepper {
    id: ElementId,
    label: Option<SharedString>,
    value: f32,
    min: f32,
    max: f32,
    step: f32,
    unit: SharedString,
    on_change: Option<Rc<dyn Fn(&f32, &mut Window, &mut App) + 'static>>,
}

impl NumericStepper {
    pub fn new(id: impl Into<ElementId>, value: f32, min: f32, max: f32) -> Self {
        Self {
            id: id.into(),
            label: None,
            value: value.clamp(min, max),
            min,
            max,
            step: 1.0,
            unit: SharedString::from("px"),
            on_change: None,
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn step(mut self, step: f32) -> Self {
        self.step = step.max(0.01);
        self
    }

    pub fn unit(mut self, unit: impl Into<SharedString>) -> Self {
        self.unit = unit.into();
        self
    }

    pub fn on_change(
        mut self,
        handler: impl Fn(&f32, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for NumericStepper {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx
            .try_global::<SnagTheme>()
            .copied()
            .unwrap_or_else(SnagTheme::light);

        let dec_val = (self.value - self.step).clamp(self.min, self.max);
        let inc_val = (self.value + self.step).clamp(self.min, self.max);
        let on_dec = self.on_change.clone();
        let on_inc = self.on_change;

        let dec_id = ElementId::Name(SharedString::from(format!("{:?}-dec", self.id)));
        let inc_id = ElementId::Name(SharedString::from(format!("{:?}-inc", self.id)));

        let value_text = if (self.value - self.value.round()).abs() < 0.001 {
            format!("{}{}", self.value.round() as i32, self.unit)
        } else {
            format!("{:.1}{}", self.value, self.unit)
        };

        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(8.0))
            .when_some(self.label, |el, label| {
                el.child(div().text_xs().text_color(theme.text_secondary).child(label))
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .h(px(28.0))
                    .rounded(theme.radius_md)
                    .bg(theme.input_bg)
                    .border_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .id(dec_id)
                            .flex()
                            .items_center()
                            .justify_center()
                            .w(px(26.0))
                            .h_full()
                            .cursor_pointer()
                            .hover(move |s| s.bg(theme.surface_hover))
                            .on_click(move |_, window, cx| {
                                if let Some(cb) = &on_dec {
                                    cb(&dec_val, window, cx);
                                }
                            })
                            .child(SnagIcon::Minus.render(px(12.0), theme.text_primary)),
                    )
                    .child(
                        div()
                            .min_w(px(52.0))
                            .px(px(6.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_xs()
                            .text_color(theme.text_primary)
                            .child(value_text),
                    )
                    .child(
                        div()
                            .id(inc_id)
                            .flex()
                            .items_center()
                            .justify_center()
                            .w(px(26.0))
                            .h_full()
                            .cursor_pointer()
                            .hover(move |s| s.bg(theme.surface_hover))
                            .on_click(move |_, window, cx| {
                                if let Some(cb) = &on_inc {
                                    cb(&inc_val, window, cx);
                                }
                            })
                            .child(SnagIcon::Plus.render(px(12.0), theme.text_primary)),
                    ),
            )
    }
}

#[derive(Clone)]
struct SliderDrag(ElementId);

struct InvisibleDragGhost;

impl Render for InvisibleDragGhost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size(px(0.0))
    }
}

/// Horizontal continuous slider supporting direct click-to-position, smooth window-wide thumb
/// dragging, and optional center-anchored bipolar mode (e.g., `100%` zoom at `50%` center).
#[derive(IntoElement)]
pub struct Slider {
    id: ElementId,
    label: Option<SharedString>,
    value: f32,
    min: f32,
    max: f32,
    center_anchor: Option<(f32, bool)>,
    width: Pixels,
    on_change: Option<Rc<dyn Fn(&f32, &mut Window, &mut App) + 'static>>,
}

impl Slider {
    pub fn new(id: impl Into<ElementId>, value: f32, min: f32, max: f32) -> Self {
        let safe_max = max.max(min + 0.001);
        Self {
            id: id.into(),
            label: None,
            value: value.clamp(min, safe_max),
            min,
            max: safe_max,
            center_anchor: None,
            width: px(160.0),
            on_change: None,
        }
    }

    /// Construct a center-anchored Zoom slider (`min..=max` with `mid` pinned at `50%` center,
    /// linear zoom-out on the left half and logarithmic `2^x` magnification on the right half).
    pub fn zoom_centered(
        id: impl Into<ElementId>,
        value: f32,
        min: f32,
        mid: f32,
        max: f32,
    ) -> Self {
        let safe_max = max.max(mid + 1.0);
        let safe_min = min.min(mid - 1.0);
        Self {
            id: id.into(),
            label: None,
            value: value.clamp(safe_min, safe_max),
            min: safe_min,
            max: safe_max,
            center_anchor: Some((mid, true)),
            width: px(124.0),
            on_change: None,
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn width(mut self, width: Pixels) -> Self {
        self.width = width;
        self
    }

    pub fn on_change(
        mut self,
        handler: impl Fn(&f32, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }

    /// Map `value` into `[0.0, 1.0]` track position.
    pub fn normalized(&self) -> f32 {
        Self::value_to_ratio(self.value, self.min, self.max, self.center_anchor)
    }

    /// Pure mapping from domain `value` to `[0.0, 1.0]` slider ratio.
    pub fn value_to_ratio(
        value: f32,
        min: f32,
        max: f32,
        center_anchor: Option<(f32, bool)>,
    ) -> f32 {
        let v = value.clamp(min, max);
        if let Some((mid, log_upper)) = center_anchor {
            if (v - mid).abs() < 1e-3 {
                return 0.5;
            } else if v < mid {
                let span = (mid - min).max(1e-4);
                return (((v - min) / span) * 0.5).clamp(0.0, 0.5);
            } else if log_upper && mid > 0.0 && max > mid {
                let log_span = (max / mid).ln().max(1e-4);
                let t = ((v / mid).ln() / log_span).clamp(0.0, 1.0);
                return (0.5 + 0.5 * t).clamp(0.5, 1.0);
            } else {
                let span = (max - mid).max(1e-4);
                return (0.5 + ((v - mid) / span) * 0.5).clamp(0.5, 1.0);
            }
        }
        ((v - min) / (max - min).max(1e-4)).clamp(0.0, 1.0)
    }

    /// Pure mapping from `[0.0, 1.0]` track ratio back to domain `value`
    /// (with a gentle magnetic snap around `0.5` when `center_anchor` is active).
    pub fn ratio_to_value(
        raw_ratio: f32,
        min: f32,
        max: f32,
        center_anchor: Option<(f32, bool)>,
    ) -> f32 {
        let r = raw_ratio.clamp(0.0, 1.0);
        if let Some((mid, log_upper)) = center_anchor {
            if (r - 0.5).abs() <= 0.025 {
                return mid;
            }
            if r < 0.5 {
                let t = (r / 0.5).clamp(0.0, 1.0);
                return (min + t * (mid - min)).clamp(min, mid);
            }
            let t = ((r - 0.5) / 0.5).clamp(0.0, 1.0);
            if log_upper && mid > 0.0 && max > mid {
                let ratio_hi = max / mid;
                return (mid * ratio_hi.powf(t)).clamp(mid, max);
            }
            return (mid + t * (max - mid)).clamp(mid, max);
        }
        (min + r * (max - min)).clamp(min, max)
    }
}

impl RenderOnce for Slider {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx
            .try_global::<SnagTheme>()
            .copied()
            .unwrap_or_else(SnagTheme::light);

        let ratio = self.normalized();
        let track_width: f32 = self.width.into();
        let center_anchor = self.center_anchor;
        let (fill_left, fill_width) = if center_anchor.is_some() {
            let mid_px = track_width * 0.5;
            let cur_px = track_width * ratio;
            if cur_px < mid_px {
                (px(cur_px), px((mid_px - cur_px).max(0.0)))
            } else {
                (px(mid_px), px((cur_px - mid_px).max(0.0)))
            }
        } else {
            (px(0.0), px(track_width * ratio))
        };
        let thumb_offset = px((track_width * ratio - 6.0).clamp(0.0, (track_width - 12.0).max(0.0)));
        let on_change_down = self.on_change.clone();
        let on_change_drag = self.on_change;
        let min = self.min;
        let max = self.max;
        let track_bounds_cell = Rc::new(Cell::new((0.0_f32, track_width.max(1.0))));
        let bounds_for_probe = Rc::clone(&track_bounds_cell);
        let bounds_for_down = Rc::clone(&track_bounds_cell);
        let drag_id = self.id.clone();
        let drag_id_check = self.id.clone();

        div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .when_some(self.label, |el, label| {
                el.child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_between()
                        .text_xs()
                        .text_color(theme.text_secondary)
                        .child(label)
                        .child(format!("{:.0}%", ratio * 100.0)),
                )
            })
            .child(
                div()
                    .id(self.id)
                    .relative()
                    .w(self.width)
                    .h(px(18.0))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, move |ev: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        if let Some(cb) = &on_change_down {
                            if ev.click_count >= 2
                                && let Some((mid, _)) = center_anchor
                            {
                                cb(&mid, window, cx);
                                return;
                            }
                            let (left_x, w) = bounds_for_down.get();
                            let mouse_x: f32 = ev.position.x.into();
                            let raw_r = ((mouse_x - left_x) / w.max(1.0)).clamp(0.0, 1.0);
                            let next = Slider::ratio_to_value(raw_r, min, max, center_anchor);
                            cb(&next, window, cx);
                        }
                    })
                    .on_drag(SliderDrag(drag_id), |_, _, _, cx| {
                        cx.new(|_| InvisibleDragGhost)
                    })
                    .on_drag_move(move |ev: &DragMoveEvent<SliderDrag>, window, cx| {
                        if ev.drag(cx).0 != drag_id_check {
                            return;
                        }
                        if let Some(cb) = &on_change_drag {
                            let left_x: f32 = ev.bounds.origin.x.into();
                            let w: f32 = ev.bounds.size.width.into();
                            let mouse_x: f32 = ev.event.position.x.into();
                            let raw_r = ((mouse_x - left_x) / w.max(1.0)).clamp(0.0, 1.0);
                            let next = Slider::ratio_to_value(raw_r, min, max, center_anchor);
                            cb(&next, window, cx);
                        }
                    })
                    .child(
                        canvas(
                            move |bounds, _window, _cx| {
                                bounds_for_probe.set((
                                    bounds.origin.x.into(),
                                    f32::from(bounds.size.width).max(1.0),
                                ));
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0(),
                    )
                    .child(
                        div()
                            .w_full()
                            .h(px(4.0))
                            .rounded(px(2.0))
                            .bg(theme.border),
                    )
                    .when(center_anchor.is_some(), |el| {
                        el.child(
                            div()
                                .absolute()
                                .left(px((track_width * 0.5 - 1.0).max(0.0)))
                                .w(px(2.0))
                                .h(px(10.0))
                                .rounded(px(1.0))
                                .bg(theme.border_strong),
                        )
                    })
                    .child(
                        div()
                            .absolute()
                            .left(fill_left)
                            .w(fill_width)
                            .h(px(4.0))
                            .rounded(px(2.0))
                            .bg(theme.accent_primary),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(thumb_offset)
                            .size(px(12.0))
                            .rounded(px(6.0))
                            .bg(theme.surface_elevated)
                            .border_2()
                            .border_color(theme.accent_primary)
                            .shadow_sm(),
                    ),
            )
    }
}

/// Floating elevated popover surface with optional header and arbitrary body element.
#[derive(IntoElement)]
pub struct Popover {
    title: Option<SharedString>,
    width: Pixels,
    content: AnyElement,
}

impl Popover {
    pub fn new(content: impl IntoElement) -> Self {
        Self {
            title: None,
            width: px(240.0),
            content: content.into_any_element(),
        }
    }

    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn width(mut self, width: Pixels) -> Self {
        self.width = width;
        self
    }
}

impl RenderOnce for Popover {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx
            .try_global::<SnagTheme>()
            .copied()
            .unwrap_or_else(SnagTheme::light);

        div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .w(self.width)
            .p(px(6.0))
            .bg(theme.surface_elevated)
            .border_1()
            .border_color(theme.border_strong)
            .rounded(theme.radius_md)
            .shadow_lg()
            .when_some(self.title, |el, title| {
                el.child(
                    div()
                        .px(px(8.0))
                        .pt(px(2.0))
                        .pb(px(5.0))
                        .border_b_1()
                        .border_color(theme.border)
                        .text_xs()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.text_secondary)
                        .child(title),
                )
            })
            .child(self.content)
    }
}

/// Swatch grid color picker with active ring indicator, hex readout, and optional Eyedropper trigger.
#[derive(IntoElement)]
pub struct ColorSwatchPicker {
    id_prefix: SharedString,
    selected_rgb: u32,
    swatches: Vec<u32>,
    on_select: Option<Rc<dyn Fn(&u32, &mut Window, &mut App) + 'static>>,
    on_eyedropper: Option<ClickHandler>,
}

impl ColorSwatchPicker {
    pub fn new(id_prefix: impl Into<SharedString>, selected_rgb: u32) -> Self {
        Self {
            id_prefix: id_prefix.into(),
            selected_rgb,
            swatches: DEFAULT_SWATCHES.to_vec(),
            on_select: None,
            on_eyedropper: None,
        }
    }

    pub fn swatches(mut self, swatches: Vec<u32>) -> Self {
        self.swatches = swatches;
        self
    }

    pub fn on_select(
        mut self,
        handler: impl Fn(&u32, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_select = Some(Rc::new(handler));
        self
    }

    pub fn on_eyedropper(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_eyedropper = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for ColorSwatchPicker {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx
            .try_global::<SnagTheme>()
            .copied()
            .unwrap_or_else(SnagTheme::light);

        let mut grid = div().flex().flex_row().flex_wrap().gap(px(6.0));
        for (idx, color_hex) in self.swatches.into_iter().enumerate() {
            let is_selected = color_hex == self.selected_rgb;
            let on_select = self.on_select.clone();
            let swatch_id = ElementId::Name(SharedString::from(format!(
                "{}-swatch-{}",
                self.id_prefix, idx
            )));

            let mut swatch = div()
                .id(swatch_id)
                .size(px(22.0))
                .rounded(theme.radius_sm)
                .bg(rgb(color_hex))
                .border_2()
                .border_color(if is_selected {
                    theme.focus_ring
                } else {
                    theme.border
                })
                .cursor_pointer();

            if let Some(cb) = on_select {
                swatch = swatch.on_click(move |_, window, cx| cb(&color_hex, window, cx));
            }
            grid = grid.child(swatch);
        }

        let hex_label = format!("#{:06X}", self.selected_rgb & 0x00FF_FFFF);
        let eyedropper_id =
            ElementId::Name(SharedString::from(format!("{}-eyedropper", self.id_prefix)));

        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(grid)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(px(6.0))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .size(px(16.0))
                                    .rounded(theme.radius_sm)
                                    .bg(rgb(self.selected_rgb))
                                    .border_1()
                                    .border_color(theme.border_strong),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.text_secondary)
                                    .child(hex_label),
                            ),
                    )
                    .when_some(self.on_eyedropper, |el, cb| {
                        el.child(
                            IconButton::new(eyedropper_id, SnagIcon::Pipette)
                                .size(px(26.0))
                                .tooltip("Eyedropper", Some("Pick color from screen".into()))
                                .on_click(move |ev, window, cx| cb(ev, window, cx)),
                        )
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slider_normalizes_clamped_values() {
        let s = Slider::new("opacity", 50.0, 0.0, 100.0);
        assert!((s.normalized() - 0.5).abs() < 1e-5);

        let low = Slider::new("low", -10.0, 0.0, 100.0);
        assert!((low.normalized() - 0.0).abs() < 1e-5);

        let high = Slider::new("high", 200.0, 0.0, 100.0);
        assert!((high.normalized() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn zoom_centered_slider_pins_100_percent_at_midpoint() {
        let anchor = Some((100.0, true));
        assert!((Slider::value_to_ratio(10.0, 10.0, 800.0, anchor) - 0.0).abs() < 1e-5);
        assert!((Slider::value_to_ratio(55.0, 10.0, 800.0, anchor) - 0.25).abs() < 1e-5);
        assert!((Slider::value_to_ratio(100.0, 10.0, 800.0, anchor) - 0.5).abs() < 1e-5);
        assert!((Slider::value_to_ratio(200.0, 10.0, 800.0, anchor) - (0.5 + 0.5 / 3.0)).abs() < 1e-4);
        assert!((Slider::value_to_ratio(400.0, 10.0, 800.0, anchor) - (0.5 + 1.0 / 3.0)).abs() < 1e-4);
        assert!((Slider::value_to_ratio(800.0, 10.0, 800.0, anchor) - 1.0).abs() < 1e-5);

        // Magnetic snap near center (`0.5 ± 0.025`) locks cleanly onto `100%`
        assert!((Slider::ratio_to_value(0.485, 10.0, 800.0, anchor) - 100.0).abs() < 1e-5);
        assert!((Slider::ratio_to_value(0.515, 10.0, 800.0, anchor) - 100.0).abs() < 1e-5);
        assert!((Slider::ratio_to_value(0.0, 10.0, 800.0, anchor) - 10.0).abs() < 1e-4);
        assert!((Slider::ratio_to_value(1.0, 10.0, 800.0, anchor) - 800.0).abs() < 1e-3);
    }
}
