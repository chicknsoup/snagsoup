use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, Render, StatefulInteractiveElement,
    Styled, Window, div, px, rgb,
};
use snagsoup_canvas::{CombineLayout, CombineOptions, TiledBitmap, combine_images};

/// GPUI "Combine Images" Dialog View (`W3-15`) wired to `snagsoup_canvas::combine_images`.
pub struct CombineImagesView {
    pub images: Vec<(String, TiledBitmap)>,
    pub options: CombineOptions,
    pub status: String,
}

impl CombineImagesView {
    pub fn new(images: Vec<(String, TiledBitmap)>) -> Self {
        Self {
            images,
            options: CombineOptions::default(),
            status: String::new(),
        }
    }

    pub fn set_layout(&mut self, layout: CombineLayout) {
        self.options.layout = layout;
    }

    pub fn set_spacing(&mut self, spacing: u32) {
        self.options.spacing = spacing;
    }

    pub fn move_item(&mut self, from: usize, to: usize) -> bool {
        if from >= self.images.len() || to >= self.images.len() || from == to {
            return false;
        }
        let item = self.images.remove(from);
        self.images.insert(to, item);
        true
    }

    pub fn build_combined(&self) -> TiledBitmap {
        let refs: Vec<&TiledBitmap> = self.images.iter().map(|(_, b)| b).collect();
        combine_images(&refs, &self.options)
    }
}

impl Render for CombineImagesView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut strip = div().flex().gap(px(8.0)).flex_wrap();
        for (idx, (label, bmp)) in self.images.iter().enumerate() {
            strip = strip.child(
                div()
                    .p(px(8.0))
                    .bg(rgb(0x272C38))
                    .rounded(px(6.0))
                    .text_xs()
                    .child(format!("#{} {} ({}x{})", idx + 1, label, bmp.width(), bmp.height())),
            );
        }

        let is_horiz = self.options.layout == CombineLayout::Horizontal;
        div()
            .size_full()
            .bg(rgb(0x181B22))
            .text_color(rgb(0xF1F5F9))
            .p(px(16.0))
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(format!(
                "Combine Images ({} items) — Layout: {:?} · Spacing: {}px",
                self.images.len(),
                self.options.layout,
                self.options.spacing
            ))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(8.0))
                    .child(
                        div()
                            .id("comb-horiz")
                            .px(px(10.0))
                            .py(px(6.0))
                            .rounded(px(4.0))
                            .bg(if is_horiz { rgb(0x2563EB) } else { rgb(0x334155) })
                            .cursor_pointer()
                            .text_xs()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_layout(CombineLayout::Horizontal);
                                cx.notify();
                            }))
                            .child("↔ Horizontal"),
                    )
                    .child(
                        div()
                            .id("comb-vert")
                            .px(px(10.0))
                            .py(px(6.0))
                            .rounded(px(4.0))
                            .bg(if !is_horiz { rgb(0x2563EB) } else { rgb(0x334155) })
                            .cursor_pointer()
                            .text_xs()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_layout(CombineLayout::Vertical);
                                cx.notify();
                            }))
                            .child("↕ Vertical"),
                    )
                    .child(
                        div()
                            .id("comb-dup")
                            .px(px(10.0))
                            .py(px(6.0))
                            .rounded(px(4.0))
                            .bg(rgb(0x334155))
                            .cursor_pointer()
                            .text_xs()
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some((_, first)) = this.images.first().cloned() {
                                    let n = this.images.len() + 1;
                                    this.images.push((format!("Capture #{n}"), first));
                                    cx.notify();
                                }
                            }))
                            .child("+ Add Copy"),
                    )
                    .child(
                        div()
                            .id("comb-export")
                            .px(px(12.0))
                            .py(px(6.0))
                            .rounded(px(4.0))
                            .bg(rgb(0x0D9488))
                            .cursor_pointer()
                            .text_xs()
                            .on_click(cx.listener(|this, _, _, cx| {
                                let combined = this.build_combined();
                                let out_dir = std::env::var("LOCALAPPDATA")
                                    .map(std::path::PathBuf::from)
                                    .unwrap_or_else(|_| std::env::temp_dir())
                                    .join("SnagSoup")
                                    .join("Captures");
                                let _ = std::fs::create_dir_all(&out_dir);
                                let ts = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .map(|d| d.as_secs())
                                    .unwrap_or(0);
                                let out = out_dir.join(format!("Combined_{ts}.png"));
                                let save_res = combined
                                    .to_png_bytes()
                                    .map_err(|e| e.to_string())
                                    .and_then(|b| std::fs::write(&out, b).map_err(|e| e.to_string()));
                                match save_res {
                                    Ok(()) => {
                                        this.status = format!(
                                            "Saved {}×{} combined PNG to {}",
                                            combined.width(),
                                            combined.height(),
                                            out.display()
                                        );
                                    }
                                    Err(e) => {
                                        this.status = format!("Combine save failed: {e}");
                                    }
                                }
                                cx.notify();
                            }))
                            .child("✓ Combine & Save PNG"),
                    ),
            )
            .child(strip)
            .child(div().text_xs().text_color(rgb(0x94A3B8)).child(self.status.clone()))
    }
}
