//! Cursor capture (`GetCursorInfo` + `GetIconInfoExW`) extracting exact RGBA8 pixels
//! (including monochrome AND/XOR `I-beam` masks) and hotspot coordinates so the cursor
//! can be stored as an independent, movable `CaptureObject` in `.snagx`.

use crate::PlatformError;
use snagsoup_model::{CaptureObject, PointF, ToolMode};
use std::collections::HashMap;
use std::mem::size_of;
use std::ptr::{null, null_mut};
use std::sync::{Mutex, OnceLock};
use windows_sys::Win32::Foundation::{GetLastError, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateCompatibleDC,
    CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDIBits, GetObjectW, HBITMAP,
    ScreenToClient,
};
use windows_sys::Win32::UI::HiDpi::GetSystemMetricsForDpi;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetCapture;
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CURSOR_SHOWING, CURSORINFO, CreateIconIndirect, DestroyCursor, GetCursorInfo, GetCursorPos,
    GetIconInfoExW, HCURSOR, HTCLIENT, ICONINFO, ICONINFOEXW, SM_CXCURSOR, SendMessageW, SetCursor,
    WM_MOUSEMOVE, WM_NCDESTROY, WM_SETCURSOR, WM_USER,
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

/// Query the exact native Windows hardware cursor dimension (`SM_CXCURSOR` in physical pixels,
/// e.g. `32` at 100%, `40` at 125%, `48` at 150%, `64` at 200%) so custom cursors never undergo
/// nearest-neighbor resampling by `user32`.
pub fn query_system_cursor_size_for_scale(scale_factor: f32) -> u32 {
    let dpi = (scale_factor.clamp(1.0, 4.0) * 96.0).round() as u32;
    let sys_px = unsafe { GetSystemMetricsForDpi(SM_CXCURSOR, dpi) };
    if sys_px >= 16 {
        sys_px as u32
    } else {
        ((32 * dpi + 48) / 96).max(32)
    }
}

/// Create a native Win32 32-bpp `BI_RGB` alpha-blended hardware cursor (`HCURSOR` as `isize`)
/// from straight `RGBA8` pixel data (`width * height * 4` bytes) and `(hotspot_x, hotspot_y)`.
///
/// Matches Windows 11's native `aero_*.cur` format (`BITMAPINFOHEADER` `BI_RGB` 32-bpp with
/// straight unpremultiplied `BGRA8` so `win32k` performs single-pass per-pixel alpha blending
/// without double-premultiplying anti-aliased edges).
pub fn create_argb_hcursor(
    width: u32,
    height: u32,
    hotspot_x: u32,
    hotspot_y: u32,
    rgba: &[u8],
) -> Result<isize, PlatformError> {
    if width == 0 || height == 0 || rgba.len() != (width as usize) * (height as usize) * 4 {
        return Err(PlatformError::ImageCodec(
            "invalid cursor RGBA buffer dimensions".into(),
        ));
    }

    let mut bmi: BITMAPINFO = unsafe { std::mem::zeroed() };
    bmi.bmiHeader = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: width as i32,
        biHeight: -(height as i32), // top-down 32-bpp BI_RGB DIB
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        biSizeImage: 0,
        biXPelsPerMeter: 0,
        biYPelsPerMeter: 0,
        biClrUsed: 0,
        biClrImportant: 0,
    };

    let mut bits_ptr: *mut core::ffi::c_void = null_mut();
    let hbm_color = unsafe {
        CreateDIBSection(
            null_mut(),
            &bmi,
            DIB_RGB_COLORS,
            &mut bits_ptr,
            null_mut(),
            0,
        )
    };
    if hbm_color.is_null() || bits_ptr.is_null() {
        return Err(PlatformError::Win32 {
            api: "CreateDIBSection",
            code: unsafe { GetLastError() },
        });
    }

    // Write straight BGRA8 into the 32-bpp BI_RGB DIB section and build matching 1-bpp AND mask.
    let total_px = (width as usize) * (height as usize);
    let dst_bgra = unsafe { std::slice::from_raw_parts_mut(bits_ptr as *mut u8, total_px * 4) };
    let mask_row_bytes = (((width as usize) + 15) / 16) * 2;
    let mut mask_bits = vec![0xFFu8; mask_row_bytes * (height as usize)];

    for y in 0..height as usize {
        for x in 0..width as usize {
            let i = y * (width as usize) + x;
            let r = rgba[i * 4];
            let g = rgba[i * 4 + 1];
            let b = rgba[i * 4 + 2];
            let a = rgba[i * 4 + 3];

            if a == 0 {
                dst_bgra[i * 4..i * 4 + 4].copy_from_slice(&[0, 0, 0, 0]);
            } else {
                dst_bgra[i * 4] = b;
                dst_bgra[i * 4 + 1] = g;
                dst_bgra[i * 4 + 2] = r;
                dst_bgra[i * 4 + 3] = a;

                let byte_idx = y * mask_row_bytes + (x / 8);
                let bit_mask = 0x80u8 >> (x % 8);
                mask_bits[byte_idx] &= !bit_mask;
            }
        }
    }

    let hbm_mask = unsafe {
        CreateBitmap(
            width as i32,
            height as i32,
            1,
            1,
            mask_bits.as_ptr() as *const _,
        )
    };
    if hbm_mask.is_null() {
        unsafe {
            DeleteObject(hbm_color);
        }
        return Err(PlatformError::Win32 {
            api: "CreateBitmap",
            code: unsafe { GetLastError() },
        });
    }

    let icon_info = ICONINFO {
        fIcon: 0, // FALSE -> create HCURSOR with hotspot
        xHotspot: hotspot_x.min(width.saturating_sub(1)),
        yHotspot: hotspot_y.min(height.saturating_sub(1)),
        hbmMask: hbm_mask,
        hbmColor: hbm_color,
    };

    let hcursor = unsafe { CreateIconIndirect(&icon_info) };
    unsafe {
        DeleteObject(hbm_color);
        DeleteObject(hbm_mask);
    }

    if hcursor.is_null() {
        return Err(PlatformError::Win32 {
            api: "CreateIconIndirect",
            code: unsafe { GetLastError() },
        });
    }
    Ok(hcursor as isize)
}

/// Destroy a custom Win32 `HCURSOR` previously created by [`create_argb_hcursor`].
pub fn destroy_hcursor(hcursor: isize) {
    if hcursor != 0 {
        unsafe {
            let _ = DestroyCursor(hcursor as HCURSOR);
        }
    }
}

const CANVAS_CURSOR_SUBCLASS_ID: usize = 0x534E_4147; // 'SNAG'

#[derive(Debug, Clone, Copy, Default)]
struct CanvasCursorEntry {
    /// Active canvas client rectangle `(left, top, right, bottom)` in physical pixels.
    rect_phys: Option<(i32, i32, i32, i32)>,
    /// Custom `HCURSOR` (`0` = inactive, delegate to GPUI's standard cursor).
    hcursor: isize,
    subclassed: bool,
}

impl CanvasCursorEntry {
    fn active_cursor_at(&self, client_x: i32, client_y: i32) -> Option<HCURSOR> {
        if self.hcursor == 0 {
            return None;
        }
        let (l, t, r, b) = self.rect_phys?;
        if client_x >= l && client_x < r && client_y >= t && client_y < b {
            Some(self.hcursor as HCURSOR)
        } else {
            None
        }
    }
}

fn canvas_cursor_map() -> &'static Mutex<HashMap<isize, CanvasCursorEntry>> {
    static MAP: OnceLock<Mutex<HashMap<isize, CanvasCursorEntry>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cursor_client_pos(hwnd: HWND) -> Option<POINT> {
    let mut pt = POINT { x: 0, y: 0 };
    unsafe {
        if GetCursorPos(&mut pt) == 0 {
            return None;
        }
        if ScreenToClient(hwnd, &mut pt) == 0 {
            return None;
        }
    }
    Some(pt)
}

unsafe extern "system" fn canvas_cursor_subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _uid_subclass: usize,
    _dw_ref_data: usize,
) -> LRESULT {
    if msg == WM_NCDESTROY {
        if let Ok(mut map) = canvas_cursor_map().lock() {
            map.remove(&(hwnd as isize));
        }
        unsafe {
            let _ = RemoveWindowSubclass(
                hwnd,
                Some(canvas_cursor_subclass_proc),
                CANVAS_CURSOR_SUBCLASS_ID,
            );
            return DefSubclassProc(hwnd, msg, wparam, lparam);
        }
    }

    if msg == WM_SETCURSOR && ((lparam & 0xFFFF) as u32 == HTCLIENT) {
        if let Some(pt) = cursor_client_pos(hwnd) {
            let custom = canvas_cursor_map()
                .lock()
                .ok()
                .and_then(|map| map.get(&(hwnd as isize)).and_then(|e| e.active_cursor_at(pt.x, pt.y)));
            if let Some(hcur) = custom {
                unsafe {
                    SetCursor(hcur);
                }
                return 1;
            }
        }
    }

    let res = unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };

    // Re-assert custom tool cursor if GPUI posted an internal cursor-style update or mouse-move
    // while the pointer is inside the active Editor canvas.
    if msg == WM_MOUSEMOVE || (WM_USER..=WM_USER + 16).contains(&msg) {
        let cap = unsafe { GetCapture() };
        if cap.is_null() || cap == hwnd {
            if let Some(pt) = cursor_client_pos(hwnd) {
                let custom = canvas_cursor_map()
                    .lock()
                    .ok()
                    .and_then(|map| map.get(&(hwnd as isize)).and_then(|e| e.active_cursor_at(pt.x, pt.y)));
                if let Some(hcur) = custom {
                    unsafe {
                        SetCursor(hcur);
                    }
                }
            }
        }
    }

    res
}

/// Synchronize the custom hardware `HCURSOR` scoped to `client_rect_phys` `(left, top, right, bottom)`
/// inside `hwnd`. Pass `None` for `client_rect_phys` or `hcursor` when standard cursors should take over.
pub fn sync_hwnd_canvas_cursor(
    hwnd: isize,
    client_rect_phys: Option<(i32, i32, i32, i32)>,
    hcursor: Option<isize>,
) {
    if hwnd == 0 {
        return;
    }
    let next_hcur = hcursor.unwrap_or(0);
    let mut changed = false;
    let mut need_subclass = false;

    if let Ok(mut map) = canvas_cursor_map().lock() {
        let entry = map.entry(hwnd).or_default();
        if entry.rect_phys != client_rect_phys || entry.hcursor != next_hcur {
            entry.rect_phys = client_rect_phys;
            entry.hcursor = next_hcur;
            changed = true;
        }
        if !entry.subclassed && next_hcur != 0 {
            entry.subclassed = true;
            need_subclass = true;
        }
    }

    if need_subclass {
        unsafe {
            let _ = SetWindowSubclass(
                hwnd as HWND,
                Some(canvas_cursor_subclass_proc),
                CANVAS_CURSOR_SUBCLASS_ID,
                0,
            );
        }
    }

    // Immediately refresh hardware cursor if the tool changed while the mouse is resting inside the canvas.
    if changed && next_hcur != 0 {
        if let Some(pt) = cursor_client_pos(hwnd as HWND) {
            if let Some((l, t, r, b)) = client_rect_phys {
                if pt.x >= l && pt.x < r && pt.y >= t && pt.y < b {
                    unsafe {
                        SetCursor(next_hcur as HCURSOR);
                    }
                }
            }
        }
    } else if changed && next_hcur == 0 {
        let _ = null::<core::ffi::c_void>();
        unsafe {
            let lparam = ((WM_MOUSEMOVE as isize) << 16) | (HTCLIENT as isize);
            let _ = SendMessageW(hwnd as HWND, WM_SETCURSOR, hwnd as WPARAM, lparam);
        }
    }
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

    #[test]
    fn create_argb_hcursor_roundtrips_hotspot_and_destroys_cleanly() {
        let sz_100 = query_system_cursor_size_for_scale(1.0);
        let sz_125 = query_system_cursor_size_for_scale(1.25);
        assert!(sz_100 >= 32 && sz_125 >= sz_100);

        let mut rgba = vec![0u8; 32 * 32 * 4];
        // Opaque pixel at (7, 7) + semi-transparent anti-aliased white pixel at (8, 7)
        let idx = (7 * 32 + 7) * 4;
        rgba[idx..idx + 4].copy_from_slice(&[17, 24, 39, 255]);
        let aa_idx = (7 * 32 + 8) * 4;
        rgba[aa_idx..aa_idx + 4].copy_from_slice(&[255, 255, 255, 96]);

        let hcur = create_argb_hcursor(32, 32, 7, 7, &rgba).expect("create_argb_hcursor succeeds");
        assert_ne!(hcur, 0);

        let mut info: ICONINFOEXW = unsafe { std::mem::zeroed() };
        info.cbSize = size_of::<ICONINFOEXW>() as u32;
        let ok = unsafe { GetIconInfoExW(hcur as HCURSOR, &mut info) };
        assert_ne!(ok, 0);
        assert_eq!(info.fIcon, 0, "must be a cursor (fIcon == FALSE), not an icon");
        assert_eq!((info.xHotspot, info.yHotspot), (7, 7));

        let (rw, rh, readback_rgba) =
            extract_color_cursor(info.hbmColor, info.hbmMask).expect("readback 32-bpp BI_RGB");
        assert_eq!((rw, rh), (32, 32));
        assert_eq!(&readback_rgba[idx..idx + 4], &[17, 24, 39, 255]);
        assert_eq!(
            &readback_rgba[aa_idx..aa_idx + 4],
            &[255, 255, 255, 96],
            "8-bit straight alpha channel must survive CreateIconIndirect without 1-bit quantization"
        );

        unsafe {
            if !info.hbmColor.is_null() {
                DeleteObject(info.hbmColor);
            }
            if !info.hbmMask.is_null() {
                DeleteObject(info.hbmMask);
            }
        }
        destroy_hcursor(hcur);
    }
}
