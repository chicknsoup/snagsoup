use gpui::{Context, IntoElement, ParentElement, Render, Styled, Window, div, px, rgb};
use snagsoup_library_export::{
    ExternalProgramConfig, ExternalProgramShare, FileAutoSave, RasterExportFormat,
    ShareDestination, ShareHistory, SharePayload, ShareReceipt, SystemPrinterShare,
    export_raster_bytes,
};
use snagsoup_win_platform::clipboard::{RgbaImage, write_image_to_clipboard};
use snagsoup_win_platform::drag_drop::start_ole_drag;
use snagsoup_win_platform::office_com::{
    OfficeTarget, send_file_via_email_mapi, send_image_to_office,
};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShareTargetKind {
    FileAutoSave,
    ClipboardPngAndDib,
    SystemPrinter(String),
    ExternalProgram(ExternalProgramConfig),
    OfficeWord,
    OfficePowerPoint,
    OfficeExcel,
    EmailMapi,
}

/// Local-First Share Menu & Recent Captures Tray OS Drag Controller (`W3-15`).
pub struct ShareMenuView {
    pub auto_save: FileAutoSave,
    pub external_programs: Vec<ExternalProgramConfig>,
    pub history: ShareHistory,
    pub history_path: Option<PathBuf>,
    pub last_status: String,
}

impl ShareMenuView {
    pub fn new(output_dir: impl Into<PathBuf>, history_path: Option<PathBuf>) -> Self {
        let history = history_path
            .as_ref()
            .and_then(|p| ShareHistory::load(p).ok())
            .unwrap_or_default();
        Self {
            auto_save: FileAutoSave::new(output_dir),
            external_programs: Vec::new(),
            history,
            history_path,
            last_status: String::new(),
        }
    }

    /// Dispatches a local-first share action and records it in `ShareHistory`.
    pub fn execute_share(
        &mut self,
        target: ShareTargetKind,
        width: u32,
        height: u32,
        rgba: &[u8],
        suggested_name: &str,
    ) -> Result<ShareReceipt, String> {
        let payload = SharePayload {
            width,
            height,
            rgba,
            suggested_name,
        };

        let receipt = match target {
            ShareTargetKind::FileAutoSave => self
                .auto_save
                .share(&payload)
                .map_err(|e| e.to_string())?,
            ShareTargetKind::ClipboardPngAndDib => {
                let img = RgbaImage::new(width, height, rgba.to_vec()).map_err(|e| e.to_string())?;
                write_image_to_clipboard(&img).map_err(|e| e.to_string())?;
                ShareReceipt {
                    destination: "Clipboard (PNG + DIB)".into(),
                    target_path: None,
                    timestamp: "now".into(),
                    summary: format!("Copied {width}x{height} image to Clipboard"),
                }
            }
            ShareTargetKind::SystemPrinter(printer_name) => {
                let mut printer = SystemPrinterShare { printer_name };
                printer.share(&payload).map_err(|e| e.to_string())?
            }
            ShareTargetKind::ExternalProgram(config) => {
                let mut ext = ExternalProgramShare {
                    config,
                    staging_dir: self.auto_save.output_dir.clone(),
                };
                ext.share(&payload).map_err(|e| e.to_string())?
            }
            ShareTargetKind::OfficeWord => {
                self.export_and_send_office(OfficeTarget::Word, &payload)?
            }
            ShareTargetKind::OfficePowerPoint => {
                self.export_and_send_office(OfficeTarget::PowerPoint, &payload)?
            }
            ShareTargetKind::OfficeExcel => {
                self.export_and_send_office(OfficeTarget::Excel, &payload)?
            }
            ShareTargetKind::EmailMapi => {
                let staged = self.stage_png(&payload)?;
                send_file_via_email_mapi(&staged, suggested_name).map_err(|e| e.to_string())?;
                ShareReceipt {
                    destination: "Email (MAPI)".into(),
                    target_path: Some(staged.to_string_lossy().into_owned()),
                    timestamp: "now".into(),
                    summary: format!("Opened Email compose with {}", staged.display()),
                }
            }
        };

        self.last_status = receipt.summary.clone();
        self.history.record(receipt.clone());
        if let Some(ref p) = self.history_path {
            let _ = self.history.save(p);
        }
        Ok(receipt)
    }

    fn stage_png(&self, payload: &SharePayload<'_>) -> Result<PathBuf, String> {
        std::fs::create_dir_all(&self.auto_save.output_dir).map_err(|e| e.to_string())?;
        let path = self
            .auto_save
            .output_dir
            .join(format!("{}.png", payload.suggested_name));
        let bytes = export_raster_bytes(
            payload.width,
            payload.height,
            payload.rgba,
            RasterExportFormat::Png,
        )
        .map_err(|e| e.to_string())?;
        std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
        Ok(path)
    }

    fn export_and_send_office(
        &self,
        office: OfficeTarget,
        payload: &SharePayload<'_>,
    ) -> Result<ShareReceipt, String> {
        let staged = self.stage_png(payload)?;
        send_image_to_office(office, &staged).map_err(|e| e.to_string())?;
        Ok(ShareReceipt {
            destination: format!("Microsoft {office:?}"),
            target_path: Some(staged.to_string_lossy().into_owned()),
            timestamp: "now".into(),
            summary: format!("Sent {} to {:?}", staged.display(), office),
        })
    }

    /// Initiates a native Windows OLE Drag-and-Drop (`CF_HDROP` + `"PNG"`) from the Recent Captures Tray!
    pub fn start_tray_os_drag(file_path: &Path, png_bytes: Option<Vec<u8>>) -> Result<u32, String> {
        start_ole_drag(vec![file_path.to_path_buf()], png_bytes).map_err(|e| e.to_string())
    }
}

impl Render for ShareMenuView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(280.0))
            .bg(rgb(0x1E2128))
            .text_color(rgb(0xF1F5F9))
            .p(px(12.0))
            .rounded(px(8.0))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(div().text_sm().child("Share (Local-First)"))
            .child(div().text_xs().child("• File Auto-Save"))
            .child(div().text_xs().child("• Copy to Clipboard (PNG + DIB)"))
            .child(div().text_xs().child("• System Printer"))
            .child(div().text_xs().child("• Microsoft Word / PowerPoint / Excel"))
            .child(div().text_xs().child("• Email (MAPI)"))
            .child(
                div()
                    .text_xs()
                    .child(format!("• External Programs ({})", self.external_programs.len())),
            )
            .child(
                div()
                    .pt(px(6.0))
                    .text_xs()
                    .text_color(rgb(0x94A3B8))
                    .child(format!(
                        "History: {} exports · {}",
                        self.history.entries.len(),
                        self.last_status
                    )),
            )
    }
}
