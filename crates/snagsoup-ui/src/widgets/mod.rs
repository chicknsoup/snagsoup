//! Reusable lightweight GPUI widgets for SnagSoup.
//!
//! Includes `Button`, `IconButton`, `NumericStepper`, `Slider`, `Popover`,
//! `ColorSwatchPicker`, and native GPUI `RichTooltip` (replacing WebView2).

use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext, ClickEvent, Context, ElementId, InteractiveElement, IntoElement,
    ParentElement, Pixels, Render, RenderOnce, SharedString, StatefulInteractiveElement, Styled,
    Window, div, prelude::FluentBuilder, px, rgb,
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

/// Horizontal continuous slider with stepped click stops for opacity, blur, tolerance, and zoom.
#[derive(IntoElement)]
pub struct Slider {
    id: ElementId,
    label: Option<SharedString>,
    value: f32,
    min: f32,
    max: f32,
    width: Pixels,
    on_change: Option<Rc<dyn Fn(&f32, &mut Window, &mut App) + 'static>>,
}

impl Slider {
    pub fn new(id: impl Into<ElementId>, value: f32, min: f32, max: f32) -> Self {
        Self {
            id: id.into(),
            label: None,
            value: value.clamp(min, max),
            min,
            max: max.max(min + 0.001),
            width: px(160.0),
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

    pub fn normalized(&self) -> f32 {
        ((self.value - self.min) / (self.max - self.min)).clamp(0.0, 1.0)
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
        let fill_width = px(track_width * ratio);
        let thumb_offset = px((track_width * ratio - 6.0).clamp(0.0, track_width - 12.0));
        let on_change = self.on_change;
        let min = self.min;
        let max = self.max;
        let current = self.value;

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
                    .on_click(move |_ev, window, cx| {
                        // Cycle by +10% on direct click as a lightweight keyboard/click fallback.
                        if let Some(cb) = &on_change {
                            let step = (max - min) * 0.1;
                            let next = if current + step > max {
                                min
                            } else {
                                (current + step).clamp(min, max)
                            };
                            cb(&next, window, cx);
                        }
                    })
                    .child(
                        div()
                            .w_full()
                            .h(px(4.0))
                            .rounded(px(2.0))
                            .bg(theme.border),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(px(0.0))
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
}
