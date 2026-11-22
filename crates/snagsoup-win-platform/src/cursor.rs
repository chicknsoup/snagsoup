//! Cursor capture (`GetCursorInfo` + `GetIconInfoExW`) extracting exact RGBA8 pixels
//! (including monochrome AND/XOR `I-beam` masks) and hotspot coordinates so the cursor
//! can be stored as an independent, movable `CaptureObject` in `.snagx`.

use crate::PlatformError;
use snagsoup_model::{CaptureObject, PointF, ToolMode};
use std::mem::size_of;
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC,
    DeleteObject, GetDIBits, GetObjectW, HBITMAP,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CURSOR_SHOWING, CURSORINFO, GetCursorInfo, GetIconInfoExW, ICONINFOEXW,
};

/// A captured mouse cursor bitmap + hotspot + virtual-screen coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedCursor {
    /// Cursor hotspot X in virtual-screen physical pixels.
    pub screen_x: i32,
    /// Cursor hotspot Y in virtual-screen physical pixels.
    pub screen_y: i32,
    /// Hotspot X offset within the cursor bitmap.
    pub hotspot_x: i32,
    /// Hotspot Y offset within the cursor bitmap.
    pub hotspot_y: i32,
    /// Bitmap width in pixels.
    pub width: u32,
    /// Bitmap height in pixels.
    pub height: u32,
    /// Straight RGBA8 pixel data (`width * height * 4` bytes).
    pub rgba: Vec<u8>,
}

impl CapturedCursor {
    /// Top-left virtual-screen X where this cursor bitmap should be drawn.
    pub const fn top_left_x(&self) -> i32 {
        self.screen_x - self.hotspot_x
    }

    /// Top-left virtual-screen Y where this cursor bitmap should be drawn.
    pub const fn top_left_y(&self) -> i32 {
        self.screen_y - self.hotspot_y
    }

    /// Convert this captured cursor into a `.snagx` `CaptureObject` relative to the
    /// captured canvas origin `(canvas_origin_x, canvas_origin_y)`.
    pub fn to_capture_object(
        &self,
        object_id: impl Into<String>,
        canvas_origin_x: i32,
        canvas_origin_y: i32,
    ) -> CaptureObject {
        let x0 = (self.top_left_x() - canvas_origin_x) as f64;
        let y0 = (self.top_left_y() - canvas_origin_y) as f64;
        CaptureObject {
            object_id: Some(object_id.into()),
            tool_mode: Some(ToolMode::Cursor),
            start_point: Some(PointF::new(x0, y0)),
            end_point: Some(PointF::new(x0 + self.width as f64, y0 + self.height as f64)),
            ..CaptureObject::default()
        }
    }
}

/// Pure decoder for monochrome Win32 cursor masks (top half = AND mask, bottom half = XOR mask).
///
/// Win32 monochrome truth table:
/// - `AND=0, XOR=0` -> Black opaque `(0, 0, 0, 255)`
/// - `AND=0, XOR=1` -> White opaque `(255, 255, 255, 255)`
/// - `AND=1, XOR=0` -> Transparent `(0, 0, 0, 0)`
/// - `AND=1, XOR=1` -> Invert/XOR pixel (e.g. `I-beam` text cursor): rendered as high-contrast
///   opaque dark `(24, 24, 24, 255)` so it remains crisp and movable as a standalone layer.
pub fn decode_monochrome_cursor_masks(
    width: u32,
    height: u32,
    and_mask: &[bool],
    xor_mask: &[bool],
) -> Vec<u8> {
    let pixel_count = (width as usize) * (height as usize);
    let mut rgba = vec![0u8; pixel_count * 4];
    for i in 0..pixel_count {
        let and_bit = and_mask.get(i).copied().unwrap_or(true);
        let xor_bit = xor_mask.get(i).copied().unwrap_or(false);
        let dst = &mut rgba[i * 4..i * 4 + 4];
        match (and_bit, xor_bit) {
            (false, false) => dst.copy_from_slice(&[0, 0, 0, 255]),
            (false, true) => dst.copy_from_slice(&[255, 255, 255, 255]),
            (true, false) => dst.copy_from_slice(&[0, 0, 0, 0]),
            (true, true) => dst.copy_from_slice(&[24, 24, 24, 255]),
        }
    }
    rgba
}

/// Capture the current system mouse cursor (if visible) as an RGBA8 bitmap + hotspot.
///
/// Returns `Ok(None)` if the cursor is currently hidden.
pub fn capture_cursor() -> Result<Option<CapturedCursor>, PlatformError> {
    let mut ci: CURSORINFO = unsafe { std::mem::zeroed() };
    ci.cbSize = size_of::<CURSORINFO>() as u32;
    if unsafe { GetCursorInfo(&mut ci) } == 0 {
        let code = unsafe { GetLastError() };
        // Locked / UAC secure / non-interactive desktop returns ERROR_ACCESS_DENIED (5).
        if code == 5 {
            return Ok(None);
        }
        return Err(PlatformError::Win32 {
            api: "GetCursorInfo",
            code,
        });
    }

    if (ci.flags & CURSOR_SHOWING) == 0 || ci.hCursor.is_null() {
        return Ok(None);
    }

    let mut ii: ICONINFOEXW = unsafe { std::mem::zeroed() };
    ii.cbSize = size_of::<ICONINFOEXW>() as u32;
    if unsafe { GetIconInfoExW(ci.hCursor, &mut ii) } == 0 {
        let code = unsafe { GetLastError() };
        return Err(PlatformError::Win32 {
            api: "GetIconInfoExW",
            code,
        });
    }

    // Ensure both HBITMAPs are always freed on exit.
    struct BitmapGuard(HBITMAP);
    impl Drop for BitmapGuard {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { DeleteObject(self.0) };
            }
        }
    }
    let _mask_guard = BitmapGuard(ii.hbmMask);
    let _color_guard = BitmapGuard(ii.hbmColor);

    let result = if !ii.hbmColor.is_null() {
        extract_color_cursor(ii.hbmColor, ii.hbmMask)
    } else if !ii.hbmMask.is_null() {
        extract_monochrome_cursor(ii.hbmMask)
    } else {
        return Ok(None);
    }?;

    let (width, height, rgba) = result;
    Ok(Some(CapturedCursor {
        screen_x: ci.ptScreenPos.x,
        screen_y: ci.ptScreenPos.y,
        hotspot_x: ii.xHotspot as i32,
        hotspot_y: ii.yHotspot as i32,
        width,
        height,
        rgba,
    }))
}

fn read_bitmap_bgra32(hbm: HBITMAP) -> Result<(u32, u32, Vec<u8>), PlatformError> {
    let mut bmp: BITMAP = unsafe { std::mem::zeroed() };
    if unsafe { GetObjectW(hbm, size_of::<BITMAP>() as i32, &mut bmp as *mut _ as *mut _) } == 0 {
        let code = unsafe { GetLastError() };
        return Err(PlatformError::Win32 {
            api: "GetObjectW",
            code,
        });
    }

    let width = bmp.bmWidth.max(0) as u32;
    let height = bmp.bmHeight.unsigned_abs();
    if width == 0 || height == 0 {
        return Err(PlatformError::ImageCodec("cursor bitmap has zero dimensions".into()));
    }

    let hdc = unsafe { CreateCompatibleDC(null_mut()) };
    if hdc.is_null() {
        let code = unsafe { GetLastError() };
        return Err(PlatformError::Win32 {
            api: "CreateCompatibleDC",
            code,
        });
    }

    let mut bmi: BITMAPINFO = unsafe { std::mem::zeroed() };
    bmi.bmiHeader = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: width as i32,
        biHeight: -(height as i32), // top-down DIB
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        biSizeImage: 0,
        biXPelsPerMeter: 0,
        biYPelsPerMeter: 0,
        biClrUsed: 0,
        biClrImportant: 0,
    };

    let mut bgra = vec![0u8; (width as usize) * (height as usize) * 4];
    let lines = unsafe {
        GetDIBits(
            hdc,
            hbm,
            0,
            height,
            bgra.as_mut_ptr() as *mut _,
            &mut bmi,
            DIB_RGB_COLORS,
        )
    };
    unsafe { DeleteDC(hdc) };

    if lines == 0 {
        let code = unsafe { GetLastError() };
        return Err(PlatformError::Win32 {
            api: "GetDIBits",
            code,
        });
    }

    Ok((width, height, bgra))
}

fn extract_color_cursor(
    hbm_color: HBITMAP,
    hbm_mask: HBITMAP,
) -> Result<(u32, u32, Vec<u8>), PlatformError> {
    let (width, height, mut bgra) = read_bitmap_bgra32(hbm_color)?;
    let has_alpha = bgra.chunks_exact(4).any(|px| px[3] != 0);

    if !has_alpha && !hbm_mask.is_null() {
        if let Ok((mw, mh, mask_bgra)) = read_bitmap_bgra32(hbm_mask) {
            if mw == width && mh >= height {
                for (dst, mpx) in bgra.chunks_exact_mut(4).zip(mask_bgra.chunks_exact(4)) {
                    // White in AND mask means transparent; black means opaque.
                    dst[3] = if mpx[0] == 0 { 255 } else { 0 };
                }
            }
        }
    }

    // Swizzle BGRA -> RGBA in place.
    for px in bgra.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    Ok((width, height, bgra))
}

fn extract_monochrome_cursor(hbm_mask: HBITMAP) -> Result<(u32, u32, Vec<u8>), PlatformError> {
    let (width, full_height, mask_bgra) = read_bitmap_bgra32(hbm_mask)?;
    let height = (full_height / 2).max(1);
    let pixel_count = (width as usize) * (height as usize);
    let mut and_mask = Vec::with_capacity(pixel_count);
    let mut xor_mask = Vec::with_capacity(pixel_count);

    for i in 0..pixel_count {
        let and_px = mask_bgra[i * 4];
        and_mask.push(and_px != 0);
    }
    let xor_offset = pixel_count * 4;
    for i in 0..pixel_count {
        let xor_px = mask_bgra.get(xor_offset + i * 4).copied().unwrap_or(0);
        xor_mask.push(xor_px != 0);
    }

    let rgba = decode_monochrome_cursor_masks(width, height, &and_mask, &xor_mask);
    Ok((width, height, rgba))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monochrome_ibeam_xor_mask_decodes_properly() {
        // 2x2 synthetic mask covering all 4 Win32 (AND, XOR) combinations:
        // (0,0)=black, (0,1)=white, (1,0)=transparent, (1,1)=XOR I-beam visible dark
        let and_mask = [false, false, true, true];
        let xor_mask = [false, true, false, true];
        let rgba = decode_monochrome_cursor_masks(2, 2, &and_mask, &xor_mask);
        assert_eq!(&rgba[0..4], &[0, 0, 0, 255]);
        assert_eq!(&rgba[4..8], &[255, 255, 255, 255]);
        assert_eq!(&rgba[8..12], &[0, 0, 0, 0]);
        assert_eq!(&rgba[12..16], &[24, 24, 24, 255]);
    }

    #[test]
    fn cursor_converts_to_snagx_capture_object() {
        let cursor = CapturedCursor {
            screen_x: 500,
            screen_y: 300,
            hotspot_x: 4,
            hotspot_y: 2,
            width: 32,
            height: 32,
            rgba: vec![255; 32 * 32 * 4],
        };
        let obj = cursor.to_capture_object("cursor-1", 100, 50);
        assert_eq!(obj.tool_mode, Some(ToolMode::Cursor));
        assert_eq!(obj.start_point, Some(PointF::new(396.0, 248.0)));
        assert_eq!(obj.end_point, Some(PointF::new(428.0, 280.0)));
    }

    #[test]
    fn live_capture_cursor_succeeds() {
        // On an interactive desktop this returns Some(cursor); on a headless session it may return None.
        let res = capture_cursor().expect("capture_cursor should not error");
        if let Some(c) = res {
            assert!(c.width > 0 && c.height > 0);
            assert_eq!(c.rgba.len(), (c.width as usize) * (c.height as usize) * 4);
        }
    }
}
