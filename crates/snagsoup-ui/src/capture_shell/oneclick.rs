//! OneClick top-edge collapsible floating capture widget (`W3-02`).

use std::rc::Rc;

use gpui::{
    App, Context, InteractiveElement, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::icons::SnagIcon;
use crate::theme::SnagTheme;
use crate::widgets::{Button, ButtonVariant, IconButton};

/// Top-edge floating widget (`120x8 px` collapsed pill -> expands into quick capture bar).
pub struct OneClickWidget {
    pub expanded: bool,
    on_capture: Option<Rc<dyn Fn(&mut Window, &mut App) + 'static>>,
    on_open_editor: Option<Rc<dyn Fn(&mut Window, &mut App) + 'static>>,
}

impl Default for OneClickWidget {
    fn default() -> Self {
        Self::new()
    }
}

impl OneClickWidget {
    pub fn new() -> Self {
        Self {
            expanded: false,
            on_capture: None,
            on_open_editor: None,
        }
    }

    pub fn with_on_capture(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_capture = Some(Rc::new(f));
        self
    }

    pub fn with_on_open_editor(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_open_editor = Some(Rc::new(f));
        self
    }
}

impl Render for OneClickWidget {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx
            .try_global::<SnagTheme>()
            .copied()
            .unwrap_or_else(SnagTheme::light);

        if !self.expanded {
            return div()
                .id("oneclick-collapsed-pill")
                .w(px(120.0))
                .h(px(8.0))
                .rounded_b(px(6.0))
                .bg(theme.capture_red)
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.expanded = true;
                    cx.notify();
                }))
                .into_any_element();
        }

        let on_capture = self.on_capture.clone();
        let on_open_editor = self.on_open_editor.clone();

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .px(px(10.0))
            .py(px(6.0))
            .bg(theme.toolbar_bg)
            .border_1()
            .border_color(theme.border_strong)
            .rounded_b(theme.radius_lg)
            .shadow_lg()
            .child(
                Button::new("oneclick-capture", "Capture")
                    .icon(SnagIcon::Capture)
                    .variant(ButtonVariant::Capture)
                    .on_click(move |_, window, cx| {
                        if let Some(cb) = &on_capture {
                            cb(window, cx);
                        }
                    }),
            )
            .child(
                IconButton::new("oneclick-editor", SnagIcon::Editor)
                    .tooltip("Open Editor", None)
                    .on_click(move |_, window, cx| {
                        if let Some(cb) = &on_open_editor {
                            cb(window, cx);
                        }
                    }),
            )
            .child(
                IconButton::new("oneclick-collapse", SnagIcon::Close)
                    .size(px(24.0))
                    .tooltip("Collapse OneClick", None)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.expanded = false;
                        cx.notify();
                    })),
            )
            .into_any_element()
    }
}
