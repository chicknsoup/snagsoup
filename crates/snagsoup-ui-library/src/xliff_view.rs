use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, Render, StatefulInteractiveElement,
    Styled, Window, div, px, rgb,
};
use snagsoup_cv_ocr::{TranslationIssue, export_snagx_to_xliff, import_xliff_to_snagx};
use snagsoup_model::SnagxArchive;

/// GPUI "Translate Callouts" & Import Translation Review View (`W3-15`) wired to `snagsoup_cv_ocr::xliff`.
pub struct XliffTranslationView {
    pub source_lang: String,
    pub target_lang: String,
    pub archive: SnagxArchive,
    pub last_exported_xliff: Option<String>,
    pub issues: Vec<TranslationIssue>,
    pub focused_object_id: Option<String>,
    pub status: String,
}

impl XliffTranslationView {
    pub fn new(archive: SnagxArchive, source_lang: impl Into<String>, target_lang: impl Into<String>) -> Self {
        Self {
            source_lang: source_lang.into(),
            target_lang: target_lang.into(),
            archive,
            last_exported_xliff: None,
            issues: Vec::new(),
            focused_object_id: None,
            status: String::new(),
        }
    }

    pub fn export_xliff(&mut self) -> snagsoup_cv_ocr::Result<String> {
        let xml = export_snagx_to_xliff(&self.archive, &self.source_lang, &self.target_lang)?;
        self.last_exported_xliff = Some(xml.clone());
        Ok(xml)
    }

    pub fn import_xliff(&mut self, xliff_xml: &str) -> snagsoup_cv_ocr::Result<usize> {
        let (localized, issues) = import_xliff_to_snagx(&self.archive, xliff_xml)?;
        self.archive = localized;
        let count = issues.len();
        self.issues = issues;
        Ok(count)
    }

    /// Click-to-focus helper for the Import Translation Review list.
    pub fn focus_issue(&mut self, issue_index: usize) -> Option<String> {
        let id = match self.issues.get(issue_index)? {
            TranslationIssue::TextOverflow { object_id, .. }
            | TranslationIssue::MissingTarget { object_id } => object_id.clone(),
        };
        self.focused_object_id = Some(id.clone());
        Some(id)
    }
}

impl Render for XliffTranslationView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut issue_list = div().flex().flex_col().gap(px(6.0));
        for (idx, issue) in self.issues.iter().enumerate() {
            let msg = match issue {
                TranslationIssue::TextOverflow {
                    object_id,
                    estimated_height_px,
                    box_height_px,
                    ..
                } => format!(
                    "[Overflow] {object_id}: est {estimated_height_px:.0}px > box {box_height_px:.0}px"
                ),
                TranslationIssue::MissingTarget { object_id } => {
                    format!("[Missing Target] {object_id}")
                }
            };
            issue_list = issue_list.child(
                div()
                    .id(gpui::SharedString::from(format!("xliff-issue-{idx}")))
                    .p(px(8.0))
                    .bg(rgb(0x3B2328))
                    .border_1()
                    .border_color(rgb(0xEF4444))
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .text_xs()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let _ = this.focus_issue(idx);
                        cx.notify();
                    }))
                    .child(msg),
            );
        }

        div()
            .size_full()
            .bg(rgb(0x181B22))
            .text_color(rgb(0xF1F5F9))
            .p(px(16.0))
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(format!(
                "Translate Callouts (XLIFF 1.2): {} -> {} · Issues: {}",
                self.source_lang,
                self.target_lang,
                self.issues.len()
            ))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(8.0))
                    .child(
                        div()
                            .id("xliff-export-btn")
                            .px(px(12.0))
                            .py(px(6.0))
                            .rounded(px(4.0))
                            .bg(rgb(0x2563EB))
                            .cursor_pointer()
                            .text_xs()
                            .on_click(cx.listener(|this, _, _, cx| {
                                match this.export_xliff() {
                                    Ok(xml) => {
                                        let out_dir = std::env::var("LOCALAPPDATA")
                                            .map(std::path::PathBuf::from)
                                            .unwrap_or_else(|_| std::env::temp_dir())
                                            .join("SnagSoup")
                                            .join("Exports");
                                        let _ = std::fs::create_dir_all(&out_dir);
                                        let out = out_dir.join(format!(
                                            "translation_{}_{}.xliff",
                                            this.source_lang, this.target_lang
                                        ));
                                        let _ = std::fs::write(&out, &xml);
                                        this.status = format!("Exported XLIFF to {}", out.display());
                                    }
                                    Err(e) => {
                                        this.status = format!("XLIFF export error: {e}");
                                    }
                                }
                                cx.notify();
                            }))
                            .child("⬇ Export .xliff"),
                    ),
            )
            .child(issue_list)
            .child(div().text_xs().text_color(rgb(0x94A3B8)).child(self.status.clone()))
    }
}
