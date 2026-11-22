//! Win32 scroll detection (`WS_VSCROLL`/`WS_HSCROLL`, `GetScrollInfo`, UIAutomation
//! `ScrollPattern`), elevated-window detection (`TokenElevation` for `--scroll-helper`),
//! and synthetic wheel scroll pulses (`SendInput`).

use crate::PlatformError;
use std::mem::size_of;
use std::ptr::null_mut;
use windows::Win32::Foundation::HWND as ComHwnd;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationScrollPattern, UIA_ScrollPatternId,
};
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, HWND};
use windows_sys::Win32::Security::{
    GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_WHEEL,
    MOUSEINPUT, SendInput, VK_ESCAPE,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GWL_STYLE, GetWindowLongW, GetWindowThreadProcessId, WHEEL_DELTA, WS_HSCROLL, WS_VSCROLL,
};

const SB_HORZ: i32 = 0;
const SB_VERT: i32 = 1;
const SIF_RANGE: u32 = 0x0001;
const SIF_PAGE: u32 = 0x0002;

#[repr(C)]
#[allow(non_snake_case)]
struct SCROLLINFO {
    cbSize: u32,
    fMask: u32,
    nMin: i32,
    nMax: i32,
    nPage: u32,
    nPos: i32,
    nTrackPos: i32,
}

#[link(name = "user32")]
unsafe extern "system" {
    fn GetScrollInfo(hwnd: HWND, nbar: i32, lpsi: *mut SCROLLINFO) -> i32;
}

/// Scrollable axis capabilities of a target window or UIA region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollDirection {
    None,
    Vertical,
    Horizontal,
    Both,
}

/// Detected scroll capabilities and UAC elevation status for a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScrollCapabilities {
    pub vertical: bool,
    pub horizontal: bool,
    /// `true` if the target window belongs to an elevated (Administrator) process,
    /// indicating `--scroll-helper` delegation is needed when SnagSoup runs unelevated.
    pub is_elevated: bool,
}

impl ScrollCapabilities {
    pub const fn direction(self) -> ScrollDirection {
        match (self.vertical, self.horizontal) {
            (true, true) => ScrollDirection::Both,
            (true, false) => ScrollDirection::Vertical,
            (false, true) => ScrollDirection::Horizontal,
            (false, false) => ScrollDirection::None,
        }
    }
}

/// Check whether `hwnd` belongs to an elevated (Administrator) process via `TokenElevation`.
pub fn is_window_elevated(hwnd: isize) -> bool {
    let mut pid: u32 = 0;
    unsafe {
        GetWindowThreadProcessId(hwnd as HWND, &mut pid);
    }
    if pid == 0 {
        return false;
    }

    let proc_handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if proc_handle.is_null() {
        // Access denied opening an elevated process from a lower-integrity process often
        // indicates the target is elevated/protected.
        return unsafe { GetLastError() } == 5;
    }

    let mut token: HANDLE = null_mut();
    let ok = unsafe { OpenProcessToken(proc_handle, TOKEN_QUERY, &mut token) };
    unsafe { CloseHandle(proc_handle) };
    if ok == 0 || token.is_null() {
        return false;
    }

    let mut elevation: TOKEN_ELEVATION = unsafe { std::mem::zeroed() };
    let mut ret_len: u32 = 0;
    let info_ok = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            &mut elevation as *mut _ as *mut _,
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut ret_len,
        )
    };
    unsafe { CloseHandle(token) };

    info_ok != 0 && elevation.TokenIsElevated != 0
}

/// Detect whether `hwnd` supports vertical and/or horizontal scrolling using Win32 styles,
/// `GetScrollInfo`, and a fallback UIAutomation `ScrollPattern` probe.
pub fn detect_scroll_capabilities(hwnd: isize) -> ScrollCapabilities {
    let h = hwnd as HWND;
    let style = unsafe { GetWindowLongW(h, GWL_STYLE) } as u32;
    let mut vertical = (style & WS_VSCROLL) != 0 || has_scrollable_bar(h, SB_VERT);
    let mut horizontal = (style & WS_HSCROLL) != 0 || has_scrollable_bar(h, SB_HORZ);

    // Modern browsers/Electron/WPF windows draw custom scrollbars without WS_VSCROLL.
    if !vertical && !horizontal {
        if let Some((uia_v, uia_h)) = probe_uia_scroll_pattern(hwnd) {
            vertical = uia_v;
            horizontal = uia_h;
        }
    }

    ScrollCapabilities {
        vertical,
        horizontal,
        is_elevated: is_window_elevated(hwnd),
    }
}

fn has_scrollable_bar(hwnd: HWND, bar: i32) -> bool {
    let mut si: SCROLLINFO = unsafe { std::mem::zeroed() };
    si.cbSize = size_of::<SCROLLINFO>() as u32;
    si.fMask = SIF_RANGE | SIF_PAGE;
    if unsafe { GetScrollInfo(hwnd, bar, &mut si) } == 0 {
        return false;
    }
    let span = si.nMax.saturating_sub(si.nMin) + 1;
    span > 1 && (si.nPage == 0 || (si.nPage as i32) < span)
}

fn probe_uia_scroll_pattern(hwnd: isize) -> Option<(bool, bool)> {
    let co_init = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let res = (|| -> Option<(bool, bool)> {
        let uia: IUIAutomation =
            unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()? };
        let elem = unsafe { uia.ElementFromHandle(ComHwnd(hwnd as *mut _)).ok()? };
        let pat = unsafe {
            elem.GetCurrentPatternAs::<IUIAutomationScrollPattern>(UIA_ScrollPatternId)
                .ok()?
        };
        let v = unsafe { pat.CurrentVerticallyScrollable().ok()? }.as_bool();
        let h = unsafe { pat.CurrentHorizontallyScrollable().ok()? }.as_bool();
        Some((v, h))
    })();
    if co_init.is_ok() {
        unsafe { CoUninitialize() };
    }
    res
}

/// Send synthetic mouse wheel scroll notches (`vertical_notches < 0` scrolls down;
/// `horizontal_notches > 0` scrolls right).
pub fn send_scroll_wheel(vertical_notches: i32, horizontal_notches: i32) -> Result<(), PlatformError> {
    let mut inputs: Vec<INPUT> = Vec::new();
    if vertical_notches != 0 {
        inputs.push(INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: (vertical_notches * WHEEL_DELTA as i32) as u32,
                    dwFlags: MOUSEEVENTF_WHEEL,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        });
    }
    if horizontal_notches != 0 {
        inputs.push(INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: (horizontal_notches * WHEEL_DELTA as i32) as u32,
                    dwFlags: MOUSEEVENTF_HWHEEL,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        });
    }
    if inputs.is_empty() {
        return Ok(());
    }

    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            size_of::<INPUT>() as i32,
        )
    };
    if sent != inputs.len() as u32 {
        let code = unsafe { GetLastError() };
        return Err(PlatformError::Win32 {
            api: "SendInput",
            code,
        });
    }
    Ok(())
}

/// Non-blocking check for whether the user is holding `Esc` to abort scrolling capture.
pub fn is_escape_pressed() -> bool {
    let state = unsafe { GetAsyncKeyState(VK_ESCAPE as i32) };
    (state as u16 & 0x8000) != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_capabilities_direction_mapping() {
        assert_eq!(
            ScrollCapabilities {
                vertical: false,
                horizontal: false,
                is_elevated: false
            }
            .direction(),
            ScrollDirection::None
        );
        assert_eq!(
            ScrollCapabilities {
                vertical: true,
                horizontal: false,
                is_elevated: false
            }
            .direction(),
            ScrollDirection::Vertical
        );
        assert_eq!(
            ScrollCapabilities {
                vertical: false,
                horizontal: true,
                is_elevated: false
            }
            .direction(),
            ScrollDirection::Horizontal
        );
        assert_eq!(
            ScrollCapabilities {
                vertical: true,
                horizontal: true,
                is_elevated: true
            }
            .direction(),
            ScrollDirection::Both
        );
    }

    #[test]
    fn zero_scroll_wheel_is_noop() {
        send_scroll_wheel(0, 0).expect("zero scroll should succeed as no-op");
    }
}
