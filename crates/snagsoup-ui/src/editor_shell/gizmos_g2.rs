//! Interactive Vector Gizmos Group 2 (`W3-08`):
//! - In-place Canvas Text & Callout editor implementing `gpui::EntityInputHandler`
//!   (full UTF-16 IME composition support for Vietnamese Telex/Unikey!)
//! - Multi-tail Callout yellow handles (360° drag + `+` add 2nd/3rd tails)
//! - Categorized Vector Stamp Gallery with instant keyword search (`StampSearcher`)

use std::ops::Range;

use gpui::{
    Bounds, Context, EntityInputHandler, Pixels, Point, UTF16Selection, Window, point, px, size,
};
use snagsoup_canvas::DocumentSession;
use snagsoup_model::{
    CaptureObject, ColorRgba, PointF, RichTextSpan, RichTextStyle, decode_rtf_base64,
    encode_rtf_base64, extract_plain_text, generate_rtf,
};

/// In-place rich text editor supporting Windows IME composition (Vietnamese Telex/Unikey).
#[derive(Debug, Clone)]
pub struct InlineTextEditor {
    pub content: String,
    /// Byte range of the active selection in `content`.
    pub selected_range: Range<usize>,
    /// Byte range of active IME composition (`marked text`).
    pub marked_range: Option<Range<usize>>,
    pub style: RichTextStyle,
    pub text_outline_width: f64,
    pub text_outline_color: Option<ColorRgba>,
    pub target_object_idx: Option<usize>,
}

impl Default for InlineTextEditor {
    fn default() -> Self {
        Self {
            content: String::new(),
            selected_range: 0..0,
            marked_range: None,
            style: RichTextStyle {
                font_family: Some("Segoe UI".to_string()),
                font_size_pt: Some(18.0),
                color: Some(ColorRgba::BLACK),
                bold: false,
                italic: false,
                underline: false,
            },
            text_outline_width: 0.0,
            text_outline_color: None,
            target_object_idx: None,
        }
    }
}

impl InlineTextEditor {
    /// Convert a UTF-16 code-unit offset into a valid UTF-8 byte offset in `self.content`.
    pub fn offset_from_utf16(&self, utf16_offset: usize) -> usize {
        let mut utf16_count = 0usize;
        for (byte_idx, ch) in self.content.char_indices() {
            if utf16_count >= utf16_offset {
                return byte_idx;
            }
            utf16_count += ch.len_utf16();
        }
        self.content.len()
    }

    /// Convert a UTF-8 byte offset into a UTF-16 code-unit offset.
    pub fn offset_to_utf16(&self, byte_offset: usize) -> usize {
        let clamped = byte_offset.min(self.content.len());
        self.content[..clamped].encode_utf16().count()
    }

    pub fn range_from_utf16(&self, range_utf16: Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    pub fn range_to_utf16(&self, range_utf8: Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range_utf8.start)..self.offset_to_utf16(range_utf8.end)
    }

    /// Commit IME / plain text replacement (works identically in headless tests and GPUI).
    pub fn replace_utf16_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        mark_new_text: bool,
        new_selected_utf16: Option<Range<usize>>,
    ) {
        let range = range_utf16
            .map(|r| self.range_from_utf16(r))
            .or_else(|| self.marked_range.clone())
            .unwrap_or_else(|| self.selected_range.clone());

        let start = range.start.min(self.content.len());
        let end = range.end.min(self.content.len());
        self.content.replace_range(start..end, new_text);

        let inserted_end = start + new_text.len();
        if mark_new_text && !new_text.is_empty() {
            self.marked_range = Some(start..inserted_end);
        } else {
            self.marked_range = None;
        }

        if let Some(sel) = new_selected_utf16 {
            let base_utf16 = self.offset_to_utf16(start);
            let s = self.offset_from_utf16(base_utf16 + sel.start);
            let e = self.offset_from_utf16(base_utf16 + sel.end);
            self.selected_range = s..e;
        } else {
            self.selected_range = inserted_end..inserted_end;
        }
    }

    /// Load existing text & style from `obj` into the inline text editor.
    pub fn load_from_object(&mut self, obj_idx: usize, obj: &CaptureObject) {
        self.target_object_idx = Some(obj_idx);
        self.marked_range = None;
        let text = obj
            .rtf_encoded_text
            .as_deref()
            .and_then(|b64| decode_rtf_base64(b64).ok())
            .map(|rtf| extract_plain_text(&rtf))
            .filter(|s| !s.is_empty())
            .or_else(|| obj.placeholder_text.clone())
            .unwrap_or_default();
        self.content = text;
        let end = self.content.len();
        self.selected_range = end..end;
        if let Some(sz) = obj.font_size {
            self.style.font_size_pt = Some(sz as f32);
        }
        if let Some(fg) = obj.foreground_color {
            self.style.color = Some(fg);
        }
    }

    pub fn prev_char_boundary(&self, idx: usize) -> usize {
        let clamped = idx.min(self.content.len());
        self.content[..clamped]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(0)
    }

    pub fn next_char_boundary(&self, idx: usize) -> usize {
        let clamped = idx.min(self.content.len());
        let mut iter = self.content[clamped..].char_indices();
        let _ = iter.next();
        iter.next()
            .map(|(off, _)| clamped + off)
            .unwrap_or(self.content.len())
    }

    pub fn backspace(&mut self) -> bool {
        self.marked_range = None;
        if !self.selected_range.is_empty() {
            let s = self.selected_range.start.min(self.content.len());
            let e = self.selected_range.end.min(self.content.len());
            self.content.replace_range(s..e, "");
            self.selected_range = s..s;
            return true;
        }
        let cur = self.selected_range.start.min(self.content.len());
        if cur == 0 {
            return false;
        }
        let prev = self.prev_char_boundary(cur);
        self.content.replace_range(prev..cur, "");
        self.selected_range = prev..prev;
        true
    }

    pub fn delete_forward(&mut self) -> bool {
        self.marked_range = None;
        if !self.selected_range.is_empty() {
            let s = self.selected_range.start.min(self.content.len());
            let e = self.selected_range.end.min(self.content.len());
            self.content.replace_range(s..e, "");
            self.selected_range = s..s;
            return true;
        }
        let cur = self.selected_range.end.min(self.content.len());
        if cur >= self.content.len() {
            return false;
        }
        let next = self.next_char_boundary(cur);
        self.content.replace_range(cur..next, "");
        self.selected_range = cur..cur;
        true
    }

    pub fn move_left(&mut self) {
        let pos = if self.selected_range.is_empty() {
            self.prev_char_boundary(self.selected_range.start)
        } else {
            self.selected_range.start.min(self.content.len())
        };
        self.selected_range = pos..pos;
    }

    pub fn move_right(&mut self) {
        let pos = if self.selected_range.is_empty() {
            self.next_char_boundary(self.selected_range.end)
        } else {
            self.selected_range.end.min(self.content.len())
        };
        self.selected_range = pos..pos;
    }

    pub fn move_home(&mut self) {
        self.selected_range = 0..0;
    }

    pub fn move_end(&mut self) {
        let end = self.content.len();
        self.selected_range = end..end;
    }

    pub fn select_all(&mut self) {
        self.selected_range = 0..self.content.len();
    }

    pub fn insert_text(&mut self, text: &str) {
        self.replace_utf16_range(None, text, false, None);
    }

    /// Encode the current text & style to Base64 RTF and write back into `CaptureObject`.
    pub fn sync_to_object(&self, obj: &mut CaptureObject) {
        let rtf = generate_rtf(&[RichTextSpan {
            text: self.content.clone(),
            style: self.style.clone(),
        }]);
        obj.rtf_encoded_text = Some(encode_rtf_base64(&rtf));
        if let Some(sz) = self.style.font_size_pt {
            obj.font_size = Some(sz as f64);
        }
        if self.text_outline_width > 0.0 {
            obj.text_outline_width = Some(self.text_outline_width);
            obj.text_outline_color = self.text_outline_color;
        }
    }
}

impl EntityInputHandler for InlineTextEditor {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(range_utf16);
        *adjusted_range = Some(self.range_to_utf16(range.clone()));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(self.selected_range.clone()),
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range.clone().map(|r| self.range_to_utf16(r))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.replace_utf16_range(range_utf16, new_text, false, None);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.replace_utf16_range(range_utf16, new_text, true, new_selected_range_utf16);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(Bounds::new(
            point(element_bounds.origin.x, element_bounds.origin.y),
            size(px(2.0), px(22.0)),
        ))
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.offset_to_utf16(self.selected_range.end))
    }
}

/// Multi-tail Callout controller (360° drag + `+` add 2nd/3rd tails).
pub struct CalloutTailController;

impl CalloutTailController {
    /// Drag an existing tail tip to any 360° position around the callout box.
    pub fn drag_tail_tip(
        session: &mut DocumentSession,
        obj_idx: usize,
        tail_idx: usize,
        new_tip: PointF,
    ) -> bool {
        let Some(mut obj) = session.page.capture_objects.get(obj_idx).cloned() else {
            return false;
        };
        let tails = obj.tail_points.get_or_insert_with(Vec::new);
        if tail_idx < tails.len() {
            tails[tail_idx] = new_tip;
            session.update_object(obj_idx, obj)
        } else {
            false
        }
    }

    /// Add an additional tail pointer (2nd, 3rd, ...) to a Callout via the `+` handle.
    pub fn add_tail(session: &mut DocumentSession, obj_idx: usize, tip: PointF) -> Option<usize> {
        let mut obj = session.page.capture_objects.get(obj_idx).cloned()?;
        let tails = obj.tail_points.get_or_insert_with(Vec::new);
        tails.push(tip);
        let new_idx = tails.len() - 1;
        session.update_object(obj_idx, obj);
        Some(new_idx)
    }

    /// Remove a tail pointer by index.
    pub fn remove_tail(session: &mut DocumentSession, obj_idx: usize, tail_idx: usize) -> bool {
        let Some(mut obj) = session.page.capture_objects.get(obj_idx).cloned() else {
            return false;
        };
        if let Some(ref mut tails) = obj.tail_points {
            if tail_idx < tails.len() {
                tails.remove(tail_idx);
                return session.update_object(obj_idx, obj);
            }
        }
        false
    }
}

/// A vector stamp item in the Stamp Gallery.
#[derive(Debug, Clone)]
pub struct StampEntry {
    pub id: String,
    pub name: String,
    pub badge: &'static str,
    pub accent_hex: u32,
    pub category: String,
    pub keywords: Vec<String>,
    pub svg_markup: String,
}

/// Categorized Stamp Gallery with instant keyword & category search (`StampSearcher`).
#[derive(Debug, Clone)]
pub struct StampSearcher {
    pub stamps: Vec<StampEntry>,
}

impl Default for StampSearcher {
    fn default() -> Self {
        let mk = |id: &str, name: &str, badge: &'static str, accent_hex: u32, cat: &str, kw: &[&str]| {
            StampEntry {
                id: id.into(),
                name: name.into(),
                badge,
                accent_hex,
                category: cat.into(),
                keywords: kw.iter().map(|s| (*s).into()).collect(),
                svg_markup: format!(r#"<svg xmlns="http://www.w3.org/2000/svg" data-id="{id}"/>"#),
            }
        };
        Self {
            stamps: vec![
                mk("sym-check", "Checkmark", "✓", 0x16A34A, "Symbols", &["ok", "done", "pass", "tick"]),
                mk("sym-cross", "Cross", "✕", 0xDC2626, "Symbols", &["no", "fail", "error", "wrong"]),
                mk("sym-warn", "Warning", "⚠", 0xF59E0B, "Symbols", &["alert", "caution", "danger"]),
                mk("sym-info", "Info", "i", 0x2563EB, "Symbols", &["help", "note", "information"]),
                mk("sym-question", "Question", "?", 0x7C3AED, "Symbols", &["ask", "faq", "unknown"]),
                mk("sym-ban", "Prohibited", "⊘", 0xDC2626, "Symbols", &["ban", "stop", "forbidden"]),
                mk("sym-star", "Gold Star", "★", 0xEAB308, "Badges", &["favorite", "rating", "award"]),
                mk("sym-heart", "Heart", "♥", 0xE11D48, "Badges", &["love", "like"]),
                mk("badge-thumbsup", "Approved", "OK", 0x059669, "Badges", &["good", "approve", "yes"]),
                mk("badge-bolt", "Lightning", "⚡", 0xCA8A04, "Badges", &["fast", "power", "flash"]),
                mk("sym-bug", "Bug", "BUG", 0xBE123C, "Badges", &["issue", "defect", "fix"]),
                mk("sym-pin", "Pin", "📍", 0xEF4444, "Markers", &["location", "map", "here"]),
                mk("sym-flag", "Flag", "⚑", 0xEA580C, "Markers", &["milestone", "report", "mark"]),
                mk("sym-lock", "Lock", "🔒", 0x0F766E, "Markers", &["security", "private", "auth"]),
                mk("ui-cursor", "Pointer", "↖", 0x334155, "Cursors", &["mouse", "click", "arrow"]),
                mk("ui-hand", "Click Hand", "👆", 0x0284C7, "Cursors", &["tap", "press", "touch"]),
            ],
        }
    }
}

impl StampSearcher {
    pub const CATEGORIES: &'static [&'static str] = &["All", "Symbols", "Badges", "Markers", "Cursors"];

    /// Instant case-insensitive search across name, category, and keywords.
    pub fn search(&self, query: &str) -> Vec<&StampEntry> {
        let q = query.trim().to_lowercase();
        if q.is_empty() || q == "all" {
            return self.stamps.iter().collect();
        }
        self.stamps
            .iter()
            .filter(|s| {
                s.name.to_lowercase().contains(&q)
                    || s.category.to_lowercase().contains(&q)
                    || s.keywords.iter().any(|k| k.to_lowercase().contains(&q))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use snagsoup_canvas::TiledBitmap;
    use snagsoup_model::{SnagxPage, ToolMode, decode_rtf_base64, extract_plain_text};

    #[test]
    fn vietnamese_ime_composition_and_multi_tail_callout() {
        let mut editor = InlineTextEditor::default();
        // Simulate Vietnamese Telex IME composition: "Tieng" -> marked "Tiếng" -> committed "Tiếng Việt"
        editor.replace_utf16_range(None, "Tieng", true, None);
        assert!(editor.marked_range.is_some());
        editor.replace_utf16_range(None, "Tiếng Việt", false, None);
        assert!(editor.marked_range.is_none());
        assert_eq!(editor.content, "Tiếng Việt");

        let mut callout = CaptureObject {
            tool_mode: Some(ToolMode::Callout),
            start_point: Some(PointF::new(40.0, 40.0)),
            end_point: Some(PointF::new(240.0, 120.0)),
            tail_points: Some(vec![PointF::new(140.0, 160.0)]),
            ..CaptureObject::default()
        };
        editor.sync_to_object(&mut callout);
        let decoded = decode_rtf_base64(callout.rtf_encoded_text.as_deref().unwrap()).unwrap();
        assert_eq!(extract_plain_text(&decoded), "Tiếng Việt");

        let mut session = DocumentSession::new(
            SnagxPage {
                capture_objects: vec![callout],
                ..SnagxPage::default()
            },
            TiledBitmap::new_filled(400, 300, ColorRgba::WHITE),
        );

        // Add 2nd tail pointing opposite direction
        let t2 = CalloutTailController::add_tail(&mut session, 0, PointF::new(280.0, 80.0)).unwrap();
        assert_eq!(t2, 1);
        assert!(CalloutTailController::drag_tail_tip(
            &mut session,
            0,
            t2,
            PointF::new(300.0, 20.0)
        ));
        assert_eq!(
            session.page.capture_objects[0].tail_points.as_ref().unwrap().len(),
            2
        );
    }

    #[test]
    fn stamp_searcher_matches_keywords_and_categories() {
        let searcher = StampSearcher::default();
        assert_eq!(searcher.search("caution").len(), 1);
        assert_eq!(searcher.search("Symbols").len(), 6);
        assert_eq!(searcher.search("").len(), 16);
    }
}
