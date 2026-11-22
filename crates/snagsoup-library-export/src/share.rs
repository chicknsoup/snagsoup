use crate::codecs::{export_raster_bytes, RasterExportFormat};
use crate::error::{ExportError, Result};
use chrono::Local;
use quick_xml::events::Event;
use quick_xml::Reader;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Local-first export/share payload.
pub struct SharePayload<'a> {
    pub width: u32,
    pub height: u32,
    pub rgba: &'a [u8],
    pub suggested_name: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareReceipt {
    pub destination: String,
    pub target_path: Option<String>,
    pub timestamp: String,
    pub summary: String,
}

/// Local-first Share Destination trait (zero cloud/HTTP/TLS bloat!).
pub trait ShareDestination {
    fn name(&self) -> &str;
    fn share(&mut self, payload: &SharePayload<'_>) -> Result<ShareReceipt>;
}

/// 1. File Auto-Save destination supporting Snagit filename templates:
/// `<Prefix>`, `<AutoNum>`, `<YYYY-MM-DD>`, `<HH-MM-SS>`.
#[derive(Debug, Clone)]
pub struct FileAutoSave {
    pub output_dir: PathBuf,
    pub filename_template: String,
    pub prefix: String,
    pub next_auto_num: u32,
    pub format: RasterExportFormat,
}

impl FileAutoSave {
    pub fn new(output_dir: impl Into<PathBuf>) -> Self {
        Self {
            output_dir: output_dir.into(),
            filename_template: "<Prefix><AutoNum>_<YYYY-MM-DD>_<HH-MM-SS>".to_string(),
            prefix: "SnagSoup_".to_string(),
            next_auto_num: 1,
            format: RasterExportFormat::Png,
        }
    }

    pub fn format_filename(&self) -> String {
        let now = Local::now();
        let ext = match self.format {
            RasterExportFormat::Png => "png",
            RasterExportFormat::Jpeg { .. } => "jpg",
            RasterExportFormat::WebP => "webp",
            RasterExportFormat::Bmp => "bmp",
            RasterExportFormat::Tiff => "tiff",
            RasterExportFormat::Ico => "ico",
        };
        let stem = self
            .filename_template
            .replace("<Prefix>", &self.prefix)
            .replace("<AutoNum>", &format!("{:04}", self.next_auto_num))
            .replace("<YYYY-MM-DD>", &now.format("%Y-%m-%d").to_string())
            .replace("<HH-MM-SS>", &now.format("%H-%M-%S").to_string());
        format!("{stem}.{ext}")
    }
}

impl ShareDestination for FileAutoSave {
    fn name(&self) -> &str {
        "File Auto-Save"
    }

    fn share(&mut self, payload: &SharePayload<'_>) -> Result<ShareReceipt> {
        fs::create_dir_all(&self.output_dir)?;
        let fname = self.format_filename();
        self.next_auto_num += 1;
        let full_path = self.output_dir.join(fname);
        let bytes = export_raster_bytes(payload.width, payload.height, payload.rgba, self.format)?;
        fs::write(&full_path, bytes)?;
        let path_str = full_path.to_string_lossy().into_owned();
        Ok(ShareReceipt {
            destination: self.name().to_string(),
            target_path: Some(path_str.clone()),
            timestamp: Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            summary: format!("Saved to {path_str}"),
        })
    }
}

/// 2. External Program / CLI Script Hook (matches Snagit's `ProgramOutputs.xml`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalProgramConfig {
    pub display_name: String,
    pub executable_path: String,
    pub parameters: String,
}

/// Parses Snagit's `ProgramOutputs.xml` into `Vec<ExternalProgramConfig>`.
pub fn parse_program_outputs_xml(xml: &str) -> Result<Vec<ExternalProgramConfig>> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut out = Vec::new();

    let mut cur: Option<ExternalProgramConfig> = None;
    let mut cur_tag = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                if tag == "Program" || tag == "ProgramOutput" {
                    cur = Some(ExternalProgramConfig {
                        display_name: String::new(),
                        executable_path: String::new(),
                        parameters: String::new(),
                    });
                }
                cur_tag = tag;
            }
            Ok(Event::Text(ref e)) => {
                if let (Some(ref mut cfg), Ok(txt)) = (cur.as_mut(), e.unescape()) {
                    match cur_tag.as_str() {
                        "DisplayName" | "Name" => cfg.display_name = txt.into_owned(),
                        "ExecutablePath" | "Path" | "ProgramPath" => {
                            cfg.executable_path = txt.into_owned()
                        }
                        "Parameters" | "Arguments" => cfg.parameters = txt.into_owned(),
                        _ => {}
                    }
                }
            }
            Ok(Event::End(ref e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                if tag == "Program" || tag == "ProgramOutput" {
                    if let Some(cfg) = cur.take() {
                        if !cfg.executable_path.is_empty() {
                            out.push(cfg);
                        }
                    }
                }
                cur_tag.clear();
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(ExportError::InvalidInput(format!("XML parse error: {e}"))),
            _ => {}
        }
        buf.clear();
    }
    Ok(out)
}

pub struct ExternalProgramShare {
    pub config: ExternalProgramConfig,
    pub staging_dir: PathBuf,
}

impl ShareDestination for ExternalProgramShare {
    fn name(&self) -> &str {
        &self.config.display_name
    }

    fn share(&mut self, payload: &SharePayload<'_>) -> Result<ShareReceipt> {
        fs::create_dir_all(&self.staging_dir)?;
        let file_path = self.staging_dir.join(format!("{}.png", payload.suggested_name));
        let bytes = export_raster_bytes(
            payload.width,
            payload.height,
            payload.rgba,
            RasterExportFormat::Png,
        )?;
        fs::write(&file_path, bytes)?;
        let path_str = file_path.to_string_lossy().into_owned();

        let mut cmd = Command::new(&self.config.executable_path);
        if !self.config.parameters.trim().is_empty() {
            let expanded = self.config.parameters.replace("%1", &path_str);
            for arg in expanded.split_whitespace() {
                cmd.arg(arg);
            }
        } else {
            cmd.arg(&path_str);
        }
        let _child = cmd.spawn()?;

        Ok(ShareReceipt {
            destination: self.config.display_name.clone(),
            target_path: Some(path_str.clone()),
            timestamp: Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            summary: format!("Launched {} with {path_str}", self.config.display_name),
        })
    }
}

/// 3. Win32 GDI System Printer destination (`CreateDCW` + `StartDocW` + `StretchDIBits`).
pub struct SystemPrinterShare {
    pub printer_name: String,
}

impl ShareDestination for SystemPrinterShare {
    fn name(&self) -> &str {
        "System Printer"
    }

    fn share(&mut self, payload: &SharePayload<'_>) -> Result<ShareReceipt> {
        use windows_sys::Win32::Graphics::Gdi::{
            CreateDCW, DeleteDC, GetDeviceCaps, StretchDIBits, BITMAPINFO, BITMAPINFOHEADER,
            BI_RGB, DIB_RGB_COLORS, HORZRES, SRCCOPY, VERTRES,
        };
        use windows_sys::Win32::Storage::Xps::{
            EndDoc, EndPage, StartDocW, StartPage, DOCINFOW,
        };

        let mut driver: Vec<u16> = "WINSPOOL\0".encode_utf16().collect();
        let mut device: Vec<u16> = format!("{}\0", self.printer_name).encode_utf16().collect();
        let mut doc_name: Vec<u16> = format!("{}\0", payload.suggested_name)
            .encode_utf16()
            .collect();

        // Convert RGBA -> BGRA bottom-up DIB
        let w = payload.width as i32;
        let h = payload.height as i32;
        let mut bgra = vec![0u8; (payload.width as usize) * (payload.height as usize) * 4];
        for (src, dst) in payload.rgba.chunks_exact(4).zip(bgra.chunks_exact_mut(4)) {
            dst[0] = src[2];
            dst[1] = src[1];
            dst[2] = src[0];
            dst[3] = src[3];
        }

        unsafe {
            let hdc = CreateDCW(
                driver.as_mut_ptr(),
                device.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
            );
            if hdc.is_null() {
                return Err(ExportError::InvalidInput(format!(
                    "Failed to open printer DC for '{}'",
                    self.printer_name
                )));
            }

            let doc_info = DOCINFOW {
                cbSize: std::mem::size_of::<DOCINFOW>() as i32,
                lpszDocName: doc_name.as_mut_ptr(),
                lpszOutput: std::ptr::null(),
                lpszDatatype: std::ptr::null(),
                fwType: 0,
            };

            let started = StartDocW(hdc, &doc_info);
            if started <= 0 {
                DeleteDC(hdc);
                return Err(ExportError::InvalidInput("StartDocW failed".into()));
            }
            StartPage(hdc);

            let page_w = GetDeviceCaps(hdc, HORZRES as i32).max(1);
            let page_h = GetDeviceCaps(hdc, VERTRES as i32).max(1);
            let scale = (page_w as f32 / w.max(1) as f32).min(page_h as f32 / h.max(1) as f32);
            let draw_w = (w as f32 * scale).round() as i32;
            let draw_h = (h as f32 * scale).round() as i32;
            let dx = (page_w - draw_w) / 2;
            let dy = (page_h - draw_h) / 2;

            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h, // top-down DIB
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB,
                    biSizeImage: 0,
                    biXPelsPerMeter: 0,
                    biYPelsPerMeter: 0,
                    biClrUsed: 0,
                    biClrImportant: 0,
                },
                bmiColors: [std::mem::zeroed()],
            };

            StretchDIBits(
                hdc,
                dx,
                dy,
                draw_w,
                draw_h,
                0,
                0,
                w,
                h,
                bgra.as_ptr() as *const _,
                &bmi,
                DIB_RGB_COLORS,
                SRCCOPY,
            );

            EndPage(hdc);
            EndDoc(hdc);
            DeleteDC(hdc);
        }

        Ok(ShareReceipt {
            destination: format!("Printer: {}", self.printer_name),
            target_path: None,
            timestamp: Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            summary: format!("Printed {} to {}", payload.suggested_name, self.printer_name),
        })
    }
}

/// 4. Persistent ShareHistory JSON store.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ShareHistory {
    pub entries: Vec<ShareReceipt>,
}

impl ShareHistory {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        if !path.as_ref().exists() {
            return Ok(Self::default());
        }
        let text = fs::read_to_string(path)?;
        Ok(serde_json::from_str(&text)?)
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        if let Some(parent) = path.as_ref().parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn record(&mut self, receipt: ShareReceipt) {
        self.entries.push(receipt);
    }
}
