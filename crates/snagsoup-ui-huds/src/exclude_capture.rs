//! Win32 `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` wrapper so SnagSoup HUD
//! windows (Recording HUD, Countdown, Panoramic HUD) remain visible to the user but are
//! automatically excluded from screen captures and recorded MP4s.

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::mem::size_of;
use windows_sys::Win32::Foundation::{GetLastError, HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_TRANSITIONS_FORCEDISABLED, DwmFlush, DwmSetWindowAttribute,
};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetWindowDisplayAffinity, GetWindowRect, HWND_NOTOPMOST, HWND_TOP, HWND_TOPMOST,
    IsIconic, IsWindowVisible, SW_HIDE, SW_RESTORE, SW_SHOW, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SWP_SHOWWINDOW, SetForegroundWindow, SetWindowDisplayAffinity, SetWindowPos,
    ShowWindow,
};

pub const WDA_NONE: u32 = 0x0000_0000;
/// Windows 10 2004+ / Windows 11 flag excluding a top-level window from all screen captures.
pub const WDA_EXCLUDEFROMCAPTURE: u32 = 0x0000_0011;

/// Extract the underlying Win32 `HWND` (as `isize`) from a GPUI [`gpui::Window`].
pub fn extract_hwnd(window: &gpui::Window) -> Option<isize> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    match handle.as_raw() {
        RawWindowHandle::Win32(win32) => Some(win32.hwnd.get()),
        _ => None,
    }
}

/// Position and resize a Win32 `HWND` using exact physical screen coordinates `(x, y, w, h)`.
/// Optionally places the window `HWND_TOPMOST` and controls whether it takes focus.
pub fn position_window_physical(
    hwnd: isize,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    topmost: bool,
    activate: bool,
) -> bool {
    let insert_after = if topmost { HWND_TOPMOST } else { HWND_TOP };
    let mut flags = SWP_SHOWWINDOW;
    if !activate {
        flags |= SWP_NOACTIVATE;
    }
    let ok = unsafe { SetWindowPos(hwnd as HWND, insert_after, x, y, w.max(1), h.max(1), flags) };
    ok != 0
}

/// Immediately hide a Win32 `HWND` (`SW_HIDE`) with Windows 11 DWM fade-out transitions
/// force-disabled, drop any `HWND_TOPMOST` z-order, and call `DwmFlush()` so DWM composites
/// the clean desktop immediately without any half-faded SnagSoup window or stale Z-order.
pub fn hide_window_hwnd(hwnd: isize) -> bool {
    !hide_windows_batch(&[hwnd]).is_empty()
}

/// Hide multiple Win32 `HWND`s in one pass and execute at most **one** `DwmFlush()` across
/// the entire batch (`0` flushes when none of `hwnds` were visible), returning the subset of
/// `HWND`s that were actually visible and hidden.
pub fn hide_windows_batch(hwnds: &[isize]) -> Vec<isize> {
    let mut hidden = Vec::new();
    unsafe {
        for &hwnd in hwnds {
            if hwnd == 0 || hidden.contains(&hwnd) || IsWindowVisible(hwnd as HWND) == 0 {
                continue;
            }
            let _ = DwmSetWindowAttribute(
                hwnd as HWND,
                DWMWA_TRANSITIONS_FORCEDISABLED as u32,
                &1u32 as *const _ as *const _,
                size_of::<u32>() as u32,
            );
            let _ = SetWindowPos(
                hwnd as HWND,
                HWND_NOTOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
            if ShowWindow(hwnd as HWND, SW_HIDE) != 0 {
                hidden.push(hwnd);
            }
        }
        if !hidden.is_empty() {
            let _ = DwmFlush();
        }
    }
    hidden
}

/// Query whether a Win32 `HWND` is currently visible (`IsWindowVisible != 0`).
pub fn is_window_visible_hwnd(hwnd: isize) -> bool {
    if hwnd == 0 {
        return false;
    }
    unsafe { IsWindowVisible(hwnd as HWND) != 0 }
}

/// Restore (`SW_RESTORE` if minimized, otherwise `SW_SHOW`), raise to `HWND_TOP`, focus a
/// Win32 `HWND`, and re-enable normal DWM window transitions.
pub fn restore_and_focus_hwnd(hwnd: isize) -> bool {
    if hwnd == 0 {
        return false;
    }
    unsafe {
        let cmd = if IsIconic(hwnd as HWND) != 0 {
            SW_RESTORE
        } else {
            SW_SHOW
        };
        let _ = ShowWindow(hwnd as HWND, cmd);
        let _ = SetWindowPos(
            hwnd as HWND,
            HWND_TOP,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
        );
        let focused = SetForegroundWindow(hwnd as HWND) != 0;
        let _ = DwmSetWindowAttribute(
            hwnd as HWND,
            DWMWA_TRANSITIONS_FORCEDISABLED as u32,
            &0u32 as *const _ as *const _,
            size_of::<u32>() as u32,
        );
        focused
    }
}

/// Query `(left, top, width, height, dpi)` of `hwnd` in physical screen pixels via
/// `GetWindowRect` and `GetDpiForWindow`.
pub fn query_hwnd_physical_rect_and_dpi(hwnd: isize) -> Option<(i32, i32, i32, i32, u32)> {
    if hwnd == 0 {
        return None;
    }
    unsafe {
        let mut rc: RECT = std::mem::zeroed();
        if GetWindowRect(hwnd as HWND, &mut rc) == 0 {
            return None;
        }
        let width = rc.right - rc.left;
        let height = rc.bottom - rc.top;
        if width <= 0 || height <= 0 {
            return None;
        }
        let raw_dpi = GetDpiForWindow(hwnd as HWND);
        let dpi = if raw_dpi == 0 { 96 } else { raw_dpi };
        Some((rc.left, rc.top, width, height, dpi))
    }
}

/// Query the current cursor position in virtual-desktop physical screen coordinates.
pub fn get_cursor_physical_pos() -> Option<(i32, i32)> {
    let mut pt = POINT { x: 0, y: 0 };
    let ok = unsafe { GetCursorPos(&mut pt) };
    if ok != 0 {
        Some((pt.x, pt.y))
    } else {
        None
    }
}

/// Enable or disable capture exclusion (`WDA_EXCLUDEFROMCAPTURE`) on a top-level `HWND`.
pub fn set_window_exclude_from_capture(hwnd: isize, exclude: bool) -> Result<(), u32> {
    let affinity = if exclude {
        WDA_EXCLUDEFROMCAPTURE
    } else {
        WDA_NONE
    };
    let ok = unsafe { SetWindowDisplayAffinity(hwnd as HWND, affinity) };
    if ok != 0 {
        Ok(())
    } else {
        Err(unsafe { GetLastError() })
    }
}

/// Query whether `WDA_EXCLUDEFROMCAPTURE` is currently enabled on `hwnd`.
pub fn get_window_exclude_from_capture(hwnd: isize) -> Result<bool, u32> {
    let mut affinity: u32 = 0;
    let ok = unsafe { GetWindowDisplayAffinity(hwnd as HWND, &mut affinity) };
    if ok != 0 {
        Ok((affinity & WDA_EXCLUDEFROMCAPTURE) == WDA_EXCLUDEFROMCAPTURE)
    } else {
        Err(unsafe { GetLastError() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, WNDCLASSW, WS_POPUP,
    };

    #[test]
    fn wda_exclude_from_capture_roundtrip_on_top_level_hwnd() {
        let class_name: Vec<u16> = "SnagSoupWdaTestWindow\0".encode_utf16().collect();
        let hinst = unsafe { GetModuleHandleW(null()) };
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(DefWindowProcW),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: null_mut(),
            hCursor: null_mut(),
            hbrBackground: null_mut(),
            lpszMenuName: null(),
            lpszClassName: class_name.as_ptr(),
        };
        unsafe { RegisterClassW(&wc) };

        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class_name.as_ptr(),
                null(),
                WS_POPUP,
                0,
                0,
                100,
                40,
                null_mut(),
                null_mut(),
                hinst,
                null(),
            )
        };
        assert!(!hwnd.is_null(), "top-level test HWND should be created");

        set_window_exclude_from_capture(hwnd as isize, true)
            .expect("SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE) should succeed");
        assert!(
            get_window_exclude_from_capture(hwnd as isize)
                .expect("GetWindowDisplayAffinity should succeed")
        );

        set_window_exclude_from_capture(hwnd as isize, false)
            .expect("SetWindowDisplayAffinity(WDA_NONE) should succeed");
        assert!(
            !get_window_exclude_from_capture(hwnd as isize)
                .expect("GetWindowDisplayAffinity should succeed")
        );

        let (left, top, width, height, dpi) =
            query_hwnd_physical_rect_and_dpi(hwnd as isize)
                .expect("query_hwnd_physical_rect_and_dpi should succeed on valid HWND");
        assert_eq!((left, top, width, height), (0, 0, 100, 40));
        assert!(dpi >= 96);
        assert!(query_hwnd_physical_rect_and_dpi(0).is_none());

        let _ = restore_and_focus_hwnd(hwnd as isize);
        assert_ne!(
            unsafe { windows_sys::Win32::UI::WindowsAndMessaging::IsWindowVisible(hwnd) },
            0
        );
        let _ = hide_window_hwnd(hwnd as isize);
        assert_eq!(
            unsafe { windows_sys::Win32::UI::WindowsAndMessaging::IsWindowVisible(hwnd) },
            0
        );

        unsafe { DestroyWindow(hwnd) };
    }
}
