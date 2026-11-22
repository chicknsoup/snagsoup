use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, Render, StatefulInteractiveElement,
    Styled, Window, div, px, rgb,
};
use snagsoup_canvas::{DropZoneFitMode, TemplateEngine, TiledBitmap};
use snagsoup_model::{PointF, SnagTemplate};
use std::path::{Path, PathBuf};

/// GPUI "Create Image from Template" View (`W3-15`) wired to `snagsoup_canvas::TemplateEngine`.
pub struct TemplateBuilderView {
    pub gallery_paths: Vec<PathBuf>,
    pub active_template_name: String,
    pub engine: TemplateEngine,
    pub active_dropzone: usize,
    pub fit_mode: DropZoneFitMode,
    pub pan_offset: PointF,
    pub status: String,
}

impl TemplateBuilderView {
    pub fn new(template: SnagTemplate, name: impl Into<String>) -> Self {
        Self {
            gallery_paths: Vec::new(),
            active_template_name: name.into(),
            engine: TemplateEngine::from_template(template),
            active_dropzone: 0,
            fit_mode: DropZoneFitMode::Fill,
            pan_offset: PointF::new(0.0, 0.0),
            status: String::new(),
        }
    }

    pub fn scan_snagt_directory(&mut self, dir: &Path) -> usize {
        self.gallery_paths.clear();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.extension().and_then(|s| s.to_str()) == Some("snagt") {
                    self.gallery_paths.push(p);
                }
            }
        }
        self.gallery_paths.sort();
        self.gallery_paths.len()
    }

    pub fn load_template_file(&mut self, path: &Path) -> bool {
        match SnagTemplate::from_template_file(path) {
            Ok(tmpl) => {
                self.active_template_name = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Template")
                    .to_string();
                self.engine = TemplateEngine::from_template(tmpl);
                self.active_dropzone = 0;
                self.status = format!("Loaded {}", self.active_template_name);
                true
            }
            Err(e) => {
                self.status = format!("Template load error: {e}");
                false
            }
        }
    }

    pub fn assign_image(&mut self, dz_index: usize, bmp: &TiledBitmap) -> bool {
        match self
            .engine
            .assign_image_to_dropzone(dz_index, bmp, self.fit_mode, self.pan_offset)
        {
            Ok(ok) => {
                if ok {
                    self.status = format!("Assigned image to DropZone #{dz_index}");
                }
                ok
            }
            Err(e) => {
                self.status = format!("Assign failed: {e}");
                false
            }
        }
    }

    pub fn drag_to_swap(&mut self, idx_a: usize, idx_b: usize) -> bool {
        let ok = self.engine.swap_dropzones(idx_a, idx_b);
        if ok {
            self.status = format!("Swapped DropZone #{idx_a} <-> #{idx_b}");
        }
        ok
    }

    pub fn add_section(&mut self, clone_from: usize) -> bool {
        let ok = self.engine.duplicate_section(clone_from);
        if ok {
            self.status = "Added template section and renumbered steps".to_string();
        }
        ok
    }

    pub fn delete_section(&mut self, idx: usize) -> bool {
        let ok = self.engine.delete_section(idx).is_some();
        if ok {
            self.status = format!("Deleted section #{idx}");
        }
        ok
    }

    pub fn reorder_section(&mut self, from: usize, to: usize) -> bool {
        self.engine.reorder_section(from, to)
    }
}

impl Render for TemplateBuilderView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dz_count = self.engine.dropzone_count();
        let sec_count = self.engine.page.sections.as_ref().map_or(0, |s| s.len());

        div()
            .size_full()
            .bg(rgb(0x181B22))
            .text_color(rgb(0xF1F5F9))
            .p(px(16.0))
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .child(format!("Create Image from Template: {}", self.active_template_name))
                    .child(format!("Gallery: {} .snagt", self.gallery_paths.len())),
            )
            .child(
                div()
                    .p(px(12.0))
                    .bg(rgb(0x232733))
                    .rounded(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .child(format!("DropZones: {dz_count}"))
                    .child(format!("Sections: {sec_count}"))
                    .child(format!("FitMode: {:?}", self.fit_mode))
                    .child(
                        div()
                            .id("tmpl-add-sec")
                            .px(px(10.0))
                            .py(px(5.0))
                            .rounded(px(4.0))
                            .bg(rgb(0x2563EB))
                            .cursor_pointer()
                            .text_xs()
                            .on_click(cx.listener(|this, _, _, cx| {
                                if !this.add_section(0) {
                                    this.status = "Template has no sections to duplicate".into();
                                }
                                cx.notify();
                            }))
                            .child("+ Add Section"),
                    )
                    .child(
                        div()
                            .id("tmpl-swap-dz")
                            .px(px(10.0))
                            .py(px(5.0))
                            .rounded(px(4.0))
                            .bg(rgb(0x334155))
                            .cursor_pointer()
                            .text_xs()
                            .on_click(cx.listener(|this, _, _, cx| {
                                if !this.drag_to_swap(0, 1) {
                                    this.status = "Need >= 2 DropZones to swap".into();
                                }
                                cx.notify();
                            }))
                            .child("⇄ Swap DropZones"),
                    )
                    .child(
                        div()
                            .id("tmpl-toggle-fit")
                            .px(px(10.0))
                            .py(px(5.0))
                            .rounded(px(4.0))
                            .bg(rgb(0x334155))
                            .cursor_pointer()
                            .text_xs()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.fit_mode = match this.fit_mode {
                                    DropZoneFitMode::Fill => DropZoneFitMode::Fit,
                                    DropZoneFitMode::Fit => DropZoneFitMode::Fill,
                                };
                                this.status = format!("Fit mode -> {:?}", this.fit_mode);
                                cx.notify();
                            }))
                            .child("Toggle Fit/Fill"),
                    ),
            )
            .child(div().text_xs().text_color(rgb(0x94A3B8)).child(self.status.clone()))
    }
}
