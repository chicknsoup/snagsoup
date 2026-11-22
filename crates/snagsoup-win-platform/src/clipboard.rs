//! Multi-format Win32 Clipboard support:
//! - Writes BOTH registered `"PNG"` (preserving full Alpha channel for Slack/Teams/Figma/Word)
//!   AND `CF_DIBV5` (for broad Win32 compatibility) simultaneously.
//! - Reads RGBA8 images from Clipboard (`"PNG"` preferred, `CF_DIBV5`/`CF_DIB` fallback).

use crate::PlatformError;
use std::io::Cursor;
use std::mem::size_of;
use std::ptr::null_mut;
use std::thread;
use std::time::Duration;
use windows_sys::Win32::Foundation::{GetLastError, GlobalFree, HWND};
use windows_sys::Win32::Graphics::Gdi::{BI_BITFIELDS, BI_RGB, BITMAPINFOHEADER, BITMAPV5HEADER};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, HWND_MESSAGE};

const CF_DIB: u32 = 8;
const CF_DIBV5: u32 = 17;
const LCS_SRGB: u32 = 0x7352_4742; // 'sRGB'
const LCS_GM_IMAGES: u32 = 4;

/// Simple straight-alpha RGBA8 image buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl RgbaImage {
    pub fn new(width: u32, height: u32, data: Vec<u8>) -> Result<Self, PlatformError> {
        let expected = (width as usize) * (height as usize) * 4;
        if data.len() != expected {
            return Err(PlatformError::ImageCodec(format!(
                "expected {expected} RGBA bytes for {width}x{height}, got {}",
                data.len()
            )));
        }
        Ok(Self {
            width,
            height,
            data,
        })
    }
}

/// Encode an `RgbaImage` to PNG bytes (preserving full alpha).
pub fn encode_png(img: &RgbaImage) -> Result<Vec<u8>, PlatformError> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, img.width, img.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|e| PlatformError::ImageCodec(e.to_string()))?;
        writer
            .write_image_data(&img.data)
            .map_err(|e| PlatformError::ImageCodec(e.to_string()))?;
    }
    Ok(out)
}

/// Decode PNG bytes into straight RGBA8.
pub fn decode_png(bytes: &[u8]) -> Result<RgbaImage, PlatformError> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder
        .read_info()
        .map_err(|e| PlatformError::ImageCodec(e.to_string()))?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| PlatformError::ImageCodec(e.to_string()))?;
    let raw = &buf[..info.buffer_size()];

    let rgba = match info.color_type {
        png::ColorType::Rgba => raw.to_vec(),
        png::ColorType::Rgb => {
            let mut out = Vec::with_capacity((info.width * info.height * 4) as usize);
            for px in raw.chunks_exact(3) {
                out.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
            out
        }
        png::ColorType::GrayscaleAlpha => {
            let mut out = Vec::with_capacity((info.width * info.height * 4) as usize);
            for px in raw.chunks_exact(2) {
                out.extend_from_slice(&[px[0], px[0], px[0], px[1]]);
            }
            out
        }
        png::ColorType::Grayscale => {
            let mut out = Vec::with_capacity((info.width * info.height * 4) as usize);
            for &g in raw {
                out.extend_from_slice(&[g, g, g, 255]);
            }
            out
        }
        other => {
            return Err(PlatformError::ImageCodec(format!(
                "unsupported PNG color type: {other:?}"
            )));
        }
    };

    RgbaImage::new(info.width, info.height, rgba)
}

/// Encode an `RgbaImage` into a Windows `CF_DIBV5` buffer (bottom-up 32-bit BGRA + `BITMAPV5HEADER`).
pub fn encode_dibv5(img: &RgbaImage) -> Vec<u8> {
    let header_size = size_of::<BITMAPV5HEADER>();
    let pixel_bytes = (img.width as usize) * (img.height as usize) * 4;
    let mut out = Vec::with_capacity(header_size + pixel_bytes);

    let mut hdr: BITMAPV5HEADER = unsafe { std::mem::zeroed() };
    hdr.bV5Size = header_size as u32;
    hdr.bV5Width = img.width as i32;
    hdr.bV5Height = img.height as i32; // positive = bottom-up (most compatible across Win32 apps)
    hdr.bV5Planes = 1;
    hdr.bV5BitCount = 32;
    hdr.bV5Compression = BI_BITFIELDS;
    hdr.bV5SizeImage = pixel_bytes as u32;
    hdr.bV5RedMask = 0x00FF_0000;
    hdr.bV5GreenMask = 0x0000_FF00;
    hdr.bV5BlueMask = 0x0000_00FF;
    hdr.bV5AlphaMask = 0xFF00_0000;
    hdr.bV5CSType = LCS_SRGB;
    hdr.bV5Intent = LCS_GM_IMAGES;

    let hdr_bytes =
        unsafe { std::slice::from_raw_parts(&hdr as *const _ as *const u8, header_size) };
    out.extend_from_slice(hdr_bytes);

    // Bottom-up BGRA rows
    let row_len = (img.width as usize) * 4;
    for y in (0..img.height as usize).rev() {
        let row = &img.data[y * row_len..(y + 1) * row_len];
        for px in row.chunks_exact(4) {
            out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
        }
    }
    out
}

/// Decode a `CF_DIBV5` or `CF_DIB` buffer (24-bit BGR or 32-bit BGRA) into `RgbaImage`.
pub fn decode_dib(bytes: &[u8]) -> Result<RgbaImage, PlatformError> {
    if bytes.len() < size_of::<BITMAPINFOHEADER>() {
        return Err(PlatformError::ImageCodec("DIB buffer too small".into()));
    }
    let hdr = unsafe { std::ptr::read_unaligned(bytes.as_ptr() as *const BITMAPINFOHEADER) };
    let width = hdr.biWidth.max(0) as u32;
    let height = hdr.biHeight.unsigned_abs();
    let top_down = hdr.biHeight < 0;
    if width == 0 || height == 0 {
        return Err(PlatformError::ImageCodec("DIB has zero dimensions".into()));
    }

    let mut offset = hdr.biSize as usize;
    // BI_BITFIELDS on a v1 BITMAPINFOHEADER (40 bytes) appends 3 DWORD color masks.
    if hdr.biSize == size_of::<BITMAPINFOHEADER>() as u32 && hdr.biCompression == BI_BITFIELDS {
        offset += 12;
    }
    if offset >= bytes.len() {
        return Err(PlatformError::ImageCodec("DIB pixel offset out of range".into()));
    }
    let pixels = &bytes[offset..];

    let mut rgba = vec![0u8; (width as usize) * (height as usize) * 4];
    match hdr.biBitCount {
        32 => {
            let stride = (width as usize) * 4;
            if pixels.len() < stride * (height as usize) {
                return Err(PlatformError::ImageCodec("truncated 32-bit DIB".into()));
            }
            for y in 0..height as usize {
                let src_y = if top_down { y } else { (height as usize) - 1 - y };
                let src_row = &pixels[src_y * stride..(src_y + 1) * stride];
                let dst_row = &mut rgba[y * stride..(y + 1) * stride];
                for (s, d) in src_row.chunks_exact(4).zip(dst_row.chunks_exact_mut(4)) {
                    d[0] = s[2];
                    d[1] = s[1];
                    d[2] = s[0];
                    d[3] = s[3];
                }
            }
            // If legacy 32-bit BI_RGB DIB has all-zero alpha, promote to opaque.
            if hdr.biCompression == BI_RGB && rgba.chunks_exact(4).all(|px| px[3] == 0) {
                for px in rgba.chunks_exact_mut(4) {
                    px[3] = 255;
                }
            }
        }
        24 => {
            let stride = ((width as usize) * 3 + 3) & !3;
            if pixels.len() < stride * (height as usize) {
                return Err(PlatformError::ImageCodec("truncated 24-bit DIB".into()));
            }
            for y in 0..height as usize {
                let src_y = if top_down { y } else { (height as usize) - 1 - y };
                let src_row = &pixels[src_y * stride..src_y * stride + (width as usize) * 3];
                let dst_row =
                    &mut rgba[y * (width as usize) * 4..(y + 1) * (width as usize) * 4];
                for (s, d) in src_row.chunks_exact(3).zip(dst_row.chunks_exact_mut(4)) {
                    d[0] = s[2];
                    d[1] = s[1];
                    d[2] = s[0];
                    d[3] = 255;
                }
            }
        }
        bpp => {
            return Err(PlatformError::ImageCodec(format!(
                "unsupported DIB bit depth: {bpp}"
            )));
        }
    }

    RgbaImage::new(width, height, rgba)
}

fn png_clipboard_format() -> u32 {
    let name: Vec<u16> = "PNG\0".encode_utf16().collect();
    unsafe { RegisterClipboardFormatW(name.as_ptr()) }
}

static CLIPBOARD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct ClipboardGuard {
    owner: HWND,
    _lock: std::sync::MutexGuard<'static, ()>,
}
impl ClipboardGuard {
    fn open() -> Result<Self, PlatformError> {
        let lock = CLIPBOARD_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let static_class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        let owner = unsafe {
            CreateWindowExW(
                0,
                static_class.as_ptr(),
                std::ptr::null(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                null_mut(),
                null_mut(),
                std::ptr::null(),
            )
        };
        let mut last_err = 0u32;
        for attempt in 0..30 {
            let target_hwnd = if attempt < 20 { owner } else { null_mut() };
            if unsafe { OpenClipboard(target_hwnd) } != 0 {
                return Ok(Self { owner, _lock: lock });
            }
            last_err = unsafe { GetLastError() };
            thread::sleep(Duration::from_millis(10));
        }
        if !owner.is_null() {
            unsafe { DestroyWindow(owner) };
        }
        Err(PlatformError::Win32 {
            api: "OpenClipboard",
            code: last_err,
        })
    }
}
impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        unsafe { CloseClipboard() };
        if !self.owner.is_null() {
            unsafe { DestroyWindow(self.owner) };
        }
    }
}

fn set_clipboard_bytes(format: u32, data: &[u8]) -> Result<(), PlatformError> {
    let hmem = unsafe { GlobalAlloc(GMEM_MOVEABLE, data.len()) };
    if hmem.is_null() {
        let code = unsafe { GetLastError() };
        return Err(PlatformError::Win32 {
            api: "GlobalAlloc",
            code,
        });
    }
    let ptr = unsafe { GlobalLock(hmem) } as *mut u8;
    if ptr.is_null() {
        let code = unsafe { GetLastError() };
        unsafe { GlobalFree(hmem) };
        return Err(PlatformError::Win32 {
            api: "GlobalLock",
            code,
        });
    }
    unsafe {
        std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len());
        GlobalUnlock(hmem);
    }
    if unsafe { SetClipboardData(format, hmem) }.is_null() {
        let code = unsafe { GetLastError() };
        unsafe { GlobalFree(hmem) };
        return Err(PlatformError::Win32 {
            api: "SetClipboardData",
            code,
        });
    }
    Ok(())
}

fn get_clipboard_bytes(format: u32) -> Option<Vec<u8>> {
    if unsafe { IsClipboardFormatAvailable(format) } == 0 {
        return None;
    }
    let hmem = unsafe { GetClipboardData(format) };
    if hmem.is_null() {
        return None;
    }
    let size = unsafe { GlobalSize(hmem) };
    if size == 0 {
        return None;
    }
    let ptr = unsafe { GlobalLock(hmem) } as *const u8;
    if ptr.is_null() {
        return None;
    }
    let slice = unsafe { std::slice::from_raw_parts(ptr, size) };
    let out = slice.to_vec();
    unsafe { GlobalUnlock(hmem) };
    Some(out)
}

/// Write an RGBA8 image to the Windows Clipboard in BOTH `"PNG"` (preserving alpha)
/// and `CF_DIBV5` formats simultaneously.
pub fn write_image_to_clipboard(img: &RgbaImage) -> Result<(), PlatformError> {
    let png_bytes = encode_png(img)?;
    let dib_bytes = encode_dibv5(img);
    let cf_png = png_clipboard_format();

    let _guard = ClipboardGuard::open()?;
    if unsafe { EmptyClipboard() } == 0 {
        let code = unsafe { GetLastError() };
        return Err(PlatformError::Win32 {
            api: "EmptyClipboard",
            code,
        });
    }
    if cf_png != 0 {
        set_clipboard_bytes(cf_png, &png_bytes)?;
    }
    set_clipboard_bytes(CF_DIBV5, &dib_bytes)?;
    Ok(())
}

/// Write UTF-16 Unicode text (`CF_UNICODETEXT = 13`) to the Windows Clipboard.
pub fn write_text_to_clipboard(text: &str) -> Result<(), PlatformError> {
    const CF_UNICODETEXT: u32 = 13;
    let mut utf16: Vec<u16> = text.encode_utf16().collect();
    utf16.push(0);
    let byte_slice =
        unsafe { std::slice::from_raw_parts(utf16.as_ptr() as *const u8, utf16.len() * 2) };
    let _guard = ClipboardGuard::open()?;
    if unsafe { EmptyClipboard() } == 0 {
        let code = unsafe { GetLastError() };
        return Err(PlatformError::Win32 {
            api: "EmptyClipboard",
            code,
        });
    }
    set_clipboard_bytes(CF_UNICODETEXT, byte_slice)?;
    Ok(())
}

/// Read an RGBA8 image from the Windows Clipboard (`"PNG"` preferred for alpha fidelity,
/// falling back to `CF_DIBV5` / `CF_DIB`).
pub fn read_image_from_clipboard() -> Result<Option<RgbaImage>, PlatformError> {
    let _guard = ClipboardGuard::open()?;
    let cf_png = png_clipboard_format();
    if cf_png != 0 {
        if let Some(png_bytes) = get_clipboard_bytes(cf_png) {
            return Ok(Some(decode_png(&png_bytes)?));
        }
    }
    if let Some(dib_bytes) = get_clipboard_bytes(CF_DIBV5) {
        return Ok(Some(decode_dib(&dib_bytes)?));
    }
    if let Some(dib_bytes) = get_clipboard_bytes(CF_DIB) {
        return Ok(Some(decode_dib(&dib_bytes)?));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_alpha_image() -> RgbaImage {
        // 4x2 image with transparent, semi-transparent, and opaque pixels.
        RgbaImage::new(
            4,
            2,
            vec![
                255, 0, 0, 255, // red opaque
                0, 255, 0, 128, // green 50% alpha
                0, 0, 255, 0,   // blue fully transparent
                255, 255, 0, 200, // yellow 78% alpha
                10, 20, 30, 255,
                40, 50, 60, 64,
                70, 80, 90, 0,
                250, 240, 230, 255,
            ],
        )
        .unwrap()
    }

    #[test]
    fn dibv5_and_png_codec_preserve_alpha() {
        let img = sample_alpha_image();

        let png_bytes = encode_png(&img).expect("png encode");
        assert_eq!(decode_png(&png_bytes).expect("png decode"), img);

        let dib_bytes = encode_dibv5(&img);
        assert_eq!(decode_dib(&dib_bytes).expect("dib decode"), img);
    }

    #[test]
    fn live_clipboard_roundtrip_preserves_alpha() {
        let img = sample_alpha_image();
        write_image_to_clipboard(&img).expect("write clipboard");
        let read_back = read_image_from_clipboard()
            .expect("read clipboard")
            .expect("clipboard should contain image");
        assert_eq!(read_back, img);
    }
}
