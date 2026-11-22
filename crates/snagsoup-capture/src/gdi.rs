//! Reliable GDI `BitBlt` screen capture fallback (for Remote Desktop, headless sessions,
//! or transient `DXGI_ERROR_ACCESS_LOST` recovery).

use crate::{CaptureBackend, CaptureError, CapturedFrame};
use snagsoup_win_platform::dpi::PhysicalRect;
use std::mem::size_of;
use std::ptr::{null, null_mut};
use std::time::Instant;
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleBitmap,
    CreateCompatibleDC, CreateDCW, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits,
    HDC, ReleaseDC, SRCCOPY, SelectObject,
};
use windows_sys::w;

/// Capture `rect` from the Windows virtual desktop via GDI `BitBlt`.
pub fn capture_rect_gdi(rect: PhysicalRect) -> Result<CapturedFrame, CaptureError> {
    let width = rect.width();
    let height = rect.height();
    if width <= 0 || height <= 0 {
        return Err(CaptureError::InvalidBounds);
    }
    let started = Instant::now();

    let screen_dc = unsafe { GetDC(null_mut()) };
    if screen_dc.is_null() {
        return Err(CaptureError::Platform(
            snagsoup_win_platform::PlatformError::Win32 {
                api: "GetDC",
                code: unsafe { GetLastError() },
            },
        ));
    }

    let result = capture_from_dc(screen_dc, rect.left, rect.top, width, height, rect, started, true);
    unsafe { ReleaseDC(null_mut(), screen_dc) };
    result
}

/// Capture a single monitor by Win32 device name (e.g. `\\.\DISPLAY1`) using
/// `CreateDCW("DISPLAY", device_name, ...)` + `BitBlt(0, 0, w, h, mon_dc, 0, 0, SRCCOPY)`.
pub fn capture_monitor_gdi_by_device(
    device_name: &str,
    bounds: PhysicalRect,
) -> Result<CapturedFrame, CaptureError> {
    capture_monitor_gdi_internal(device_name, bounds, true)
}

/// Capture a single monitor directly in **native `BGRA8`** (`0` R<->B swizzle) via `CreateDCW` + `BitBlt`.
pub fn capture_monitor_gdi_bgra_by_device(
    device_name: &str,
    bounds: PhysicalRect,
) -> Result<CapturedFrame, CaptureError> {
    capture_monitor_gdi_internal(device_name, bounds, false)
}

fn capture_monitor_gdi_internal(
    device_name: &str,
    bounds: PhysicalRect,
    swizzle_to_rgba: bool,
) -> Result<CapturedFrame, CaptureError> {
    let width = bounds.width();
    let height = bounds.height();
    if width <= 0 || height <= 0 || device_name.is_empty() {
        return Err(CaptureError::InvalidBounds);
    }
    let started = Instant::now();

    let wide_device: Vec<u16> = device_name.encode_utf16().chain(std::iter::once(0)).collect();
    let mon_dc = unsafe { CreateDCW(w!("DISPLAY"), wide_device.as_ptr(), null(), null()) };
    if mon_dc.is_null() {
        return Err(CaptureError::Platform(
            snagsoup_win_platform::PlatformError::Win32 {
                api: "CreateDCW",
                code: unsafe { GetLastError() },
            },
        ));
    }

    let result = capture_from_dc(mon_dc, 0, 0, width, height, bounds, started, swizzle_to_rgba);
    unsafe { DeleteDC(mon_dc) };
    result
}

fn capture_from_dc(
    src_dc: HDC,
    src_x: i32,
    src_y: i32,
    width: i32,
    height: i32,
    bounds: PhysicalRect,
    started: Instant,
    swizzle_to_rgba: bool,
) -> Result<CapturedFrame, CaptureError> {
    let mem_dc = unsafe { CreateCompatibleDC(src_dc) };
    if mem_dc.is_null() {
        let code = unsafe { GetLastError() };
        return Err(CaptureError::Platform(
            snagsoup_win_platform::PlatformError::Win32 {
                api: "CreateCompatibleDC",
                code,
            },
        ));
    }

    let hbm = unsafe { CreateCompatibleBitmap(src_dc, width, height) };
    if hbm.is_null() {
        let code = unsafe { GetLastError() };
        unsafe { DeleteDC(mem_dc) };
        return Err(CaptureError::Platform(
            snagsoup_win_platform::PlatformError::Win32 {
                api: "CreateCompatibleBitmap",
                code,
            },
        ));
    }

    let old_obj = unsafe { SelectObject(mem_dc, hbm) };
    let mut blt_ok = unsafe {
        BitBlt(
            mem_dc,
            0,
            0,
            width,
            height,
            src_dc,
            src_x,
            src_y,
            SRCCOPY | CAPTUREBLT,
        )
    };
    if blt_ok == 0 {
        // Retry without CAPTUREBLT (some protected/non-interactive desktops deny CAPTUREBLT).
        blt_ok = unsafe {
            BitBlt(
                mem_dc,
                0,
                0,
                width,
                height,
                src_dc,
                src_x,
                src_y,
                SRCCOPY,
            )
        };
    }

    let mut rgba = vec![0u8; (width as usize) * (height as usize) * 4];
    let mut bmi: BITMAPINFO = unsafe { std::mem::zeroed() };
    bmi.bmiHeader = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: width,
        biHeight: -height, // top-down
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        biSizeImage: 0,
        biXPelsPerMeter: 0,
        biYPelsPerMeter: 0,
        biClrUsed: 0,
        biClrImportant: 0,
    };

    let lines = if blt_ok != 0 {
        unsafe {
            GetDIBits(
                mem_dc,
                hbm,
                0,
                height as u32,
                rgba.as_mut_ptr() as *mut _,
                &mut bmi,
                DIB_RGB_COLORS,
            )
        }
    } else {
        0
    };

    let last_err = if blt_ok == 0 || lines == 0 {
        unsafe { GetLastError() }
    } else {
        0
    };

    unsafe {
        SelectObject(mem_dc, old_obj);
        DeleteObject(hbm);
        DeleteDC(mem_dc);
    }

    if blt_ok == 0 {
        // Locked / UAC secure / non-interactive desktop returns ERROR_ACCESS_DENIED (5)
        // or ERROR_INVALID_HANDLE (6) from screen BitBlt.
        if last_err == 5 || last_err == 6 {
            return Err(CaptureError::AccessDenied);
        }
        return Err(CaptureError::Platform(
            snagsoup_win_platform::PlatformError::Win32 {
                api: "BitBlt",
                code: last_err,
            },
        ));
    }
    if lines == 0 {
        return Err(CaptureError::Platform(
            snagsoup_win_platform::PlatformError::Win32 {
                api: "GetDIBits",
                code: last_err,
            },
        ));
    }

    if swizzle_to_rgba {
        // Swizzle BGRA -> opaque RGBA8
        for px in rgba.chunks_exact_mut(4) {
            px.swap(0, 2);
            px[3] = 255;
        }
    } else {
        // Keep native BGRA8 and force alpha = 255 for GPUI DirectComposition swapchains
        for px in rgba.chunks_exact_mut(4) {
            px[3] = 255;
        }
    }

    Ok(CapturedFrame {
        bounds,
        width: width as u32,
        height: height as u32,
        rgba,
        backend: CaptureBackend::GdiFallback,
        elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
    })
}
