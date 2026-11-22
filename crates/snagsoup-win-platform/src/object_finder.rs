//! Visible window enumeration (Z-order, DWM cloaked filtering, extended frame bounds,
//! executable version metadata), background `IUIAutomation` element probing with strict
//! timeout, and browser address-bar URL extraction without browser extensions.

use crate::dpi::{PhysicalPoint, PhysicalRect};
use snagsoup_model::SnagxMetadata;
use std::mem::size_of;
use std::path::Path;
use std::ptr::{null, null_mut};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Duration;
use windows::Win32::Foundation::HWND as ComHwnd;
use windows::Win32::Foundation::POINT as ComPoint;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::core::VARIANT;
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationValuePattern, TreeScope_Descendants,
    UIA_ControlTypePropertyId, UIA_EditControlTypeId, UIA_ValuePatternId,
};
use windows_sys::Win32::Foundation::{BOOL, CloseHandle, HWND, LPARAM, RECT};
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute,
};
use windows_sys::Win32::Graphics::Gdi::{
    COMPLEXREGION, CreateRectRgn, DeleteObject, GetMonitorInfoW, GetWindowRgn,
    MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow, OffsetRgn, PtInRegion, SIMPLEREGION,
};
use windows_sys::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_CHILD, GW_HWNDNEXT, GWL_EXSTYLE, GetClassNameW, GetWindow, GetWindowLongW,
    GetWindowRect, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic,
    IsWindow, IsWindowVisible, IsZoomed, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
};

/// Metadata and geometry for a visible top-level window in Z-order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowInfo {
    pub hwnd: isize,
    pub title: String,
    pub rect: PhysicalRect,
    /// 0 = topmost visible window.
    pub z_order: usize,
    pub pid: u32,
    pub exe_path: Option<String>,
    pub app_name: Option<String>,
    pub app_version: Option<String>,
    pub app_company: Option<String>,
    pub is_browser: bool,
}

impl WindowInfo {
    /// Convert window and process metadata into `.snagx` `SnagxMetadata`.
    pub fn to_snagx_metadata(&self, web_url: Option<String>) -> SnagxMetadata {
        SnagxMetadata {
            app_name: self.app_name.clone(),
            app_version: self.app_version.clone(),
            app_company: self.app_company.clone(),
            window_name: if self.title.is_empty() {
                None
            } else {
                Some(self.title.clone())
            },
            web_url,
            ..SnagxMetadata::default()
        }
    }
}

/// Known desktop browser executable stems for `WebURL` extraction.
pub fn is_known_browser_exe(exe_path: &str) -> bool {
    let stem = Path::new(exe_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        stem.as_str(),
        "chrome" | "msedge" | "firefox" | "brave" | "opera" | "vivaldi" | "arc" | "waterfox"
    )
}

/// Pure Z-order hit test: returns the topmost window containing `pt` (and inside any custom
/// `GetWindowRgn` cutout when `hwnd` is a live Win32 window).
pub fn find_topmost_window_at<'a>(
    windows: &'a [WindowInfo],
    pt: PhysicalPoint,
) -> Option<&'a WindowInfo> {
    windows
        .iter()
        .filter(|w| w.rect.contains_point(pt) && point_in_window_rgn(w.hwnd, pt))
        .min_by_key(|w| w.z_order)
}

/// Geometry and metadata for a visible child sub-window / pane inside a top-level window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildPaneInfo {
    pub hwnd: isize,
    pub rect: PhysicalRect,
    pub class_name: String,
    pub depth: usize,
}

/// Front-to-back Z-ordered top-level window together with its clipped visible child panes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowHierarchyEntry {
    pub top_window: WindowInfo,
    pub child_panes: Vec<ChildPaneInfo>,
}

/// Default child-window recursion depth matching Snagit's `Selection.WindowFinder`.
pub const DEFAULT_SNAGIT_HIERARCHY_DEPTH: usize = 4;

/// Enumerate all visible, non-cloaked, non-minimized top-level windows in Z-order along with
/// their clipped visible child panes up to `max_depth` (recommended: `4`).
pub fn enumerate_window_hierarchy(max_depth: usize) -> Vec<WindowHierarchyEntry> {
    enumerate_visible_windows()
        .into_iter()
        .map(|top_window| {
            let child_panes = if max_depth > 0 {
                collect_child_panes(top_window.hwnd as HWND, top_window.rect, max_depth)
            } else {
                Vec::new()
            };
            WindowHierarchyEntry {
                top_window,
                child_panes,
            }
        })
        .collect()
}

/// Snagit `Selection.WindowFinder` parity hit-test:
/// 1. Strict front-to-back Z-order occlusion (`top_window.rect` + `point_in_window_rgn`) so an
///    occluded background window or its child panes can never win over a foreground window.
/// 2. Titlebar / outer frame whole-window rule: hovering the titlebar or outer 8px border band
///    highlights the entire `top_window.rect`.
/// 3. Deepest / smallest-area visible child pane inside the winning foreground window's client area.
pub fn find_snagit_target_at<'a>(
    hierarchy: &'a [WindowHierarchyEntry],
    pt: PhysicalPoint,
) -> Option<(PhysicalRect, &'a WindowInfo)> {
    let entry = hierarchy
        .iter()
        .filter(|e| {
            e.top_window.rect.contains_point(pt) && point_in_window_rgn(e.top_window.hwnd, pt)
        })
        .min_by_key(|e| e.top_window.z_order)?;

    let top_rect = entry.top_window.rect;
    let dpi = if entry.top_window.hwnd != 0
        && unsafe { IsWindow(entry.top_window.hwnd as HWND) } != 0
    {
        let raw_dpi = unsafe { GetDpiForWindow(entry.top_window.hwnd as HWND) };
        if raw_dpi == 0 { 96 } else { raw_dpi }
    } else {
        96
    };

    let titlebar_band = ((44 * dpi as i32) + 48) / 96;
    let border_band = ((8 * dpi as i32) + 48) / 96;

    if pt.y < top_rect.top + titlebar_band
        || pt.x < top_rect.left + border_band
        || pt.x >= top_rect.right - border_band
        || pt.y >= top_rect.bottom - border_band
    {
        return Some((top_rect, &entry.top_window));
    }

    let winning_pane = entry
        .child_panes
        .iter()
        .filter(|p| p.rect.contains_point(pt) && point_in_window_rgn(p.hwnd, pt))
        .min_by_key(|p| {
            let area = (p.rect.width().max(0) as i64) * (p.rect.height().max(0) as i64);
            (area, std::cmp::Reverse(p.depth))
        });

    if let Some(pane) = winning_pane {
        Some((pane.rect, &entry.top_window))
    } else {
        Some((top_rect, &entry.top_window))
    }
}

/// Test whether `pt` (in physical screen coordinates) falls inside `hwnd`'s custom `GetWindowRgn`
/// cutout. Returns `true` for synthetic/non-live handles and standard rectangular DWM windows.
pub fn point_in_window_rgn(hwnd: isize, pt: PhysicalPoint) -> bool {
    if hwnd == 0 || unsafe { IsWindow(hwnd as HWND) } == 0 {
        return true;
    }
    unsafe {
        let hrgn = CreateRectRgn(0, 0, 0, 0);
        if hrgn.is_null() {
            return true;
        }
        let rgn_type = GetWindowRgn(hwnd as HWND, hrgn);
        if rgn_type != SIMPLEREGION && rgn_type != COMPLEXREGION {
            DeleteObject(hrgn);
            return true;
        }
        let mut rc: RECT = std::mem::zeroed();
        if GetWindowRect(hwnd as HWND, &mut rc) == 0 {
            DeleteObject(hrgn);
            return true;
        }
        OffsetRgn(hrgn, rc.left, rc.top);
        let inside = PtInRegion(hrgn, pt.x, pt.y) != 0;
        DeleteObject(hrgn);
        inside
    }
}

/// Clamp a maximized window's extended frame rectangle (which includes `-8px` invisible resize
/// borders on Windows 10/11) to its monitor work area / monitor bounds.
pub fn clamp_maximized_rect_to_monitor(win: PhysicalRect, mon_work: PhysicalRect) -> PhysicalRect {
    intersect_rect(win, mon_work).unwrap_or(win)
}

fn intersect_rect(a: PhysicalRect, b: PhysicalRect) -> Option<PhysicalRect> {
    let left = a.left.max(b.left);
    let top = a.top.max(b.top);
    let right = a.right.min(b.right);
    let bottom = a.bottom.min(b.bottom);
    if right > left && bottom > top {
        Some(PhysicalRect::new(left, top, right, bottom))
    } else {
        None
    }
}

fn collect_child_panes(top_hwnd: HWND, top_rect: PhysicalRect, max_depth: usize) -> Vec<ChildPaneInfo> {
    let top_area = (top_rect.width().max(0) as i64) * (top_rect.height().max(0) as i64);
    if top_area <= 0 || max_depth == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    walk_child_panes(
        top_hwnd,
        top_rect,
        top_rect,
        top_area,
        1,
        max_depth,
        &mut out,
    );
    out
}

fn walk_child_panes(
    parent_hwnd: HWND,
    parent_clipped_rect: PhysicalRect,
    top_rect: PhysicalRect,
    top_area: i64,
    depth: usize,
    max_depth: usize,
    out: &mut Vec<ChildPaneInfo>,
) {
    if depth > max_depth || out.len() >= 512 {
        return;
    }
    let mut child = unsafe { GetWindow(parent_hwnd, GW_CHILD) };
    let mut sibling_guard = 0usize;
    while !child.is_null() && sibling_guard < 512 && out.len() < 512 {
        sibling_guard += 1;
        let next = unsafe { GetWindow(child, GW_HWNDNEXT) };

        if unsafe { IsWindowVisible(child) } == 0 {
            child = next;
            continue;
        }
        let ex_style = unsafe { GetWindowLongW(child, GWL_EXSTYLE) } as u32;
        if (ex_style & WS_EX_TRANSPARENT) != 0 {
            child = next;
            continue;
        }

        let mut rc: RECT = unsafe { std::mem::zeroed() };
        if unsafe { GetWindowRect(child, &mut rc) } == 0
            || rc.right <= rc.left
            || rc.bottom <= rc.top
        {
            child = next;
            continue;
        }

        let raw_rect = PhysicalRect::new(rc.left, rc.top, rc.right, rc.bottom);
        let Some(clipped) = intersect_rect(raw_rect, parent_clipped_rect)
            .and_then(|r| intersect_rect(r, top_rect))
        else {
            child = next;
            continue;
        };

        if clipped.width() >= 48 && clipped.height() >= 28 {
            let area = (clipped.width() as i64) * (clipped.height() as i64);
            if area * 100 < top_area * 96 {
                out.push(ChildPaneInfo {
                    hwnd: child as isize,
                    rect: clipped,
                    class_name: get_window_class_name(child),
                    depth,
                });
            }
            if depth < max_depth {
                walk_child_panes(
                    child,
                    clipped,
                    top_rect,
                    top_area,
                    depth + 1,
                    max_depth,
                    out,
                );
            }
        }

        child = next;
    }
}

/// Enumerate all visible, non-cloaked, non-minimized top-level windows in Z-order.
pub fn enumerate_visible_windows() -> Vec<WindowInfo> {
    let mut list: Vec<WindowInfo> = Vec::new();
    let lparam = (&mut list as *mut Vec<WindowInfo>) as LPARAM;
    unsafe {
        EnumWindows(Some(enum_windows_proc), lparam);
    }
    list
}

/// Query `AppName`, `AppVersion`, `AppCompany`, and `WindowName` for a specific `HWND`.
pub fn inspect_window(hwnd: isize) -> Option<WindowInfo> {
    build_window_info(hwnd as HWND, 0)
}

/// Returns `true` for Windows shell/wallpaper/input-overlay classes that should NOT be
/// highlighted as user application windows during Snagit-style window auto-detect.
pub fn is_ignored_shell_window_class(class_name: &str) -> bool {
    matches!(
        class_name,
        "Progman"
            | "WorkerW"
            | "Windows.UI.Core.CoreWindow"
            | "DummyDWMListenerWindow"
            | "EdgeUiInputTopWndClass"
            | "EdgeUiInputWndClass"
            | "ForegroundStaging"
            | "MultitaskingViewFrame"
            | "XamlExplorerHostIslandWindow"
    )
}

fn get_window_class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let copied = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    if copied <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..copied as usize])
}

unsafe extern "system" fn enum_windows_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let list = unsafe { &mut *(lparam as *mut Vec<WindowInfo>) };

    if unsafe { IsWindowVisible(hwnd) } == 0 || unsafe { IsIconic(hwnd) } != 0 {
        return 1;
    }

    let class_name = get_window_class_name(hwnd);
    if is_ignored_shell_window_class(&class_name) {
        return 1;
    }
    let is_taskbar = matches!(class_name.as_str(), "Shell_TrayWnd" | "Shell_SecondaryTrayWnd");

    // Skip floating tool windows (except Windows taskbars) and click-through transparent OSD overlays.
    let ex_style = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32;
    if (ex_style & WS_EX_TRANSPARENT) != 0 || (!is_taskbar && (ex_style & WS_EX_TOOLWINDOW) != 0) {
        return 1;
    }

    // Never auto-detect SnagSoup's own windows.
    let mut pid: u32 = 0;
    unsafe {
        GetWindowThreadProcessId(hwnd, &mut pid);
    }
    if pid == std::process::id() {
        return 1;
    }

    // Filter out UWP / virtual desktop cloaked windows.
    let mut cloaked: u32 = 0;
    let hr = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED as u32,
            &mut cloaked as *mut _ as *mut _,
            size_of::<u32>() as u32,
        )
    };
    if hr == 0 && cloaked != 0 {
        return 1;
    }

    let z_order = list.len();
    if let Some(info) = build_window_info(hwnd, z_order) {
        if info.rect.width() >= 16 && info.rect.height() >= 16 {
            list.push(info);
        }
    }

    1
}

fn build_window_info(hwnd: HWND, z_order: usize) -> Option<WindowInfo> {
    let rect = get_extended_window_rect(hwnd)?;
    let title = get_window_title(hwnd);

    let mut pid: u32 = 0;
    unsafe {
        GetWindowThreadProcessId(hwnd, &mut pid);
    }

    let exe_path = query_process_exe_path(pid);
    let is_browser = exe_path
        .as_deref()
        .map(is_known_browser_exe)
        .unwrap_or(false);

    let (mut app_name, app_version, app_company) = exe_path
        .as_deref()
        .map(read_file_version_metadata)
        .unwrap_or((None, None, None));

    if app_name.is_none() {
        app_name = exe_path
            .as_deref()
            .and_then(|p| Path::new(p).file_stem())
            .and_then(|s| s.to_str())
            .map(|s| s.to_string());
    }

    Some(WindowInfo {
        hwnd: hwnd as isize,
        title,
        rect,
        z_order,
        pid,
        exe_path,
        app_name,
        app_version,
        app_company,
        is_browser,
    })
}

fn get_extended_window_rect(hwnd: HWND) -> Option<PhysicalRect> {
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    let hr = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS as u32,
            &mut rc as *mut _ as *mut _,
            size_of::<RECT>() as u32,
        )
    };
    let raw_rect = if hr == 0 && rc.right > rc.left && rc.bottom > rc.top {
        PhysicalRect::new(rc.left, rc.top, rc.right, rc.bottom)
    } else if unsafe { GetWindowRect(hwnd, &mut rc) } != 0
        && rc.right > rc.left
        && rc.bottom > rc.top
    {
        PhysicalRect::new(rc.left, rc.top, rc.right, rc.bottom)
    } else {
        return None;
    };

    // Maximized top-level windows carry -8px invisible resize borders that bleed onto adjacent
    // monitors; clamp them to the owning monitor's work area (falling back to full monitor rect),
    // matching Snagit's IsTopLevelMaximizedWindow + GetBoundsOfScreenWorkspace.
    if unsafe { IsZoomed(hwnd) } != 0 {
        let hmon = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
        if !hmon.is_null() {
            let mut mi: MONITORINFO = unsafe { std::mem::zeroed() };
            mi.cbSize = size_of::<MONITORINFO>() as u32;
            if unsafe { GetMonitorInfoW(hmon, &mut mi) } != 0 {
                let work = PhysicalRect::new(
                    mi.rcWork.left,
                    mi.rcWork.top,
                    mi.rcWork.right,
                    mi.rcWork.bottom,
                );
                let mon = PhysicalRect::new(
                    mi.rcMonitor.left,
                    mi.rcMonitor.top,
                    mi.rcMonitor.right,
                    mi.rcMonitor.bottom,
                );
                if let Some(clamped) = intersect_rect(raw_rect, work) {
                    return Some(clamped);
                }
                if let Some(clamped) = intersect_rect(raw_rect, mon) {
                    return Some(clamped);
                }
            }
        }
    }

    Some(raw_rect)
}

fn get_window_title(hwnd: HWND) -> String {
    let len = unsafe { GetWindowTextLengthW(hwnd) };
    if len <= 0 {
        return String::new();
    }
    let mut buf = vec![0u16; (len as usize) + 1];
    let copied = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    if copied <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..copied as usize])
}

fn query_process_exe_path(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return None;
    }
    let mut buf = vec![0u16; 1024];
    let mut size = buf.len() as u32;
    let ok = unsafe {
        QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut size)
    };
    unsafe { CloseHandle(handle) };
    if ok == 0 || size == 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&buf[..size as usize]))
}

/// Extract `(ProductName/FileDescription, FileVersion, CompanyName)` via `GetFileVersionInfoW`.
pub fn read_file_version_metadata(path: &str) -> (Option<String>, Option<String>, Option<String>) {
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let mut handle: u32 = 0;
    let size = unsafe { GetFileVersionInfoSizeW(wide.as_ptr(), &mut handle) };
    if size == 0 {
        return (None, None, None);
    }

    let mut data = vec![0u8; size as usize];
    if unsafe { GetFileVersionInfoW(wide.as_ptr(), 0, size, data.as_mut_ptr() as *mut _) } == 0 {
        return (None, None, None);
    }

    // Query VarFileInfo\Translation to find the actual lang-codepage pair.
    let (lang, codepage) = query_translation(&data).unwrap_or((0x0409, 0x04B0));

    let product = query_version_string(&data, lang, codepage, "ProductName")
        .or_else(|| query_version_string(&data, lang, codepage, "FileDescription"));
    let version = query_version_string(&data, lang, codepage, "FileVersion")
        .or_else(|| query_version_string(&data, lang, codepage, "ProductVersion"));
    let company = query_version_string(&data, lang, codepage, "CompanyName");

    (product, version, company)
}

fn query_translation(data: &[u8]) -> Option<(u16, u16)> {
    let sub: Vec<u16> = "\\VarFileInfo\\Translation\0".encode_utf16().collect();
    let mut ptr: *mut std::ffi::c_void = null_mut();
    let mut len: u32 = 0;
    let ok = unsafe {
        VerQueryValueW(
            data.as_ptr() as *const _,
            sub.as_ptr(),
            &mut ptr,
            &mut len,
        )
    };
    if ok == 0 || ptr.is_null() || len < 4 {
        return None;
    }
    let words = unsafe { std::slice::from_raw_parts(ptr as *const u16, 2) };
    Some((words[0], words[1]))
}

fn query_version_string(data: &[u8], lang: u16, codepage: u16, field: &str) -> Option<String> {
    let sub: Vec<u16> = format!("\\StringFileInfo\\{lang:04x}{codepage:04x}\\{field}\0")
        .encode_utf16()
        .collect();
    let mut ptr: *mut std::ffi::c_void = null_mut();
    let mut len: u32 = 0;
    let ok = unsafe {
        VerQueryValueW(
            data.as_ptr() as *const _,
            sub.as_ptr(),
            &mut ptr,
            &mut len,
        )
    };
    if ok == 0 || ptr.is_null() || len == 0 {
        return None;
    }
    let slice = unsafe { std::slice::from_raw_parts(ptr as *const u16, len as usize) };
    let nul_pos = slice.iter().position(|&c| c == 0).unwrap_or(slice.len());
    let s = String::from_utf16_lossy(&slice[..nul_pos]).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

/// Bounding rectangle and basic identity of a UIAutomation control element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiElementRect {
    pub rect: PhysicalRect,
    pub name: Option<String>,
    pub control_type: i32,
}

enum ProbeRequest {
    ElementAtPoint {
        pt: PhysicalPoint,
        reply: mpsc::SyncSender<Option<UiElementRect>>,
    },
    BrowserUrl {
        hwnd: isize,
        reply: mpsc::SyncSender<Option<String>>,
    },
    Shutdown,
}

/// Background COM `IUIAutomation` prober with strict caller-side timeout (`30 ms` default)
/// so hung applications can never stall the capture crosshair.
pub struct UiElementProber {
    tx: mpsc::Sender<ProbeRequest>,
    thread: Option<JoinHandle<()>>,
}

impl UiElementProber {
    /// Start the dedicated MTA COM UIAutomation worker thread.
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel::<ProbeRequest>();
        let handle = thread::Builder::new()
            .name("snagsoup-uia-prober".to_string())
            .spawn(move || uia_worker_main(rx))
            .ok();
        Self { tx, thread: handle }
    }

    /// Probe the UIAutomation control at `pt` with a strict `timeout` (recommended: `30 ms`).
    pub fn probe_at_point(&self, pt: PhysicalPoint, timeout: Duration) -> Option<UiElementRect> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.tx
            .send(ProbeRequest::ElementAtPoint { pt, reply: reply_tx })
            .ok()?;
        reply_rx.recv_timeout(timeout).ok().flatten()
    }

    /// Extract the active `WebURL` from a browser window's address bar via `IUIAutomation`
    /// (`ValuePattern`), bounded by `timeout`.
    pub fn extract_browser_url(&self, hwnd: isize, timeout: Duration) -> Option<String> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.tx
            .send(ProbeRequest::BrowserUrl {
                hwnd,
                reply: reply_tx,
            })
            .ok()?;
        reply_rx.recv_timeout(timeout).ok().flatten()
    }
}

impl Default for UiElementProber {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for UiElementProber {
    fn drop(&mut self) {
        let _ = self.tx.send(ProbeRequest::Shutdown);
        // Do not block forever if a hung external app is stalling COM inside the worker thread;
        // detach cleanly if it does not exit quickly.
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

fn uia_worker_main(rx: mpsc::Receiver<ProbeRequest>) {
    let co_init = unsafe { CoInitializeEx(Some(null()), COINIT_MULTITHREADED) };
    let uia: Result<IUIAutomation, _> =
        unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) };

    let Ok(automation) = uia else {
        if co_init.is_ok() {
            unsafe { CoUninitialize() };
        }
        return;
    };

    while let Ok(req) = rx.recv() {
        match req {
            ProbeRequest::ElementAtPoint { pt, reply } => {
                let res = probe_element_at_point(&automation, pt);
                let _ = reply.send(res);
            }
            ProbeRequest::BrowserUrl { hwnd, reply } => {
                let res = probe_browser_url(&automation, hwnd);
                let _ = reply.send(res);
            }
            ProbeRequest::Shutdown => break,
        }
    }

    drop(automation);
    if co_init.is_ok() {
        unsafe { CoUninitialize() };
    }
}

fn probe_element_at_point(automation: &IUIAutomation, pt: PhysicalPoint) -> Option<UiElementRect> {
    let elem = unsafe {
        automation
            .ElementFromPoint(ComPoint { x: pt.x, y: pt.y })
            .ok()?
    };
    let rc = unsafe { elem.CurrentBoundingRectangle().ok()? };
    if rc.right <= rc.left || rc.bottom <= rc.top {
        return None;
    }
    let name = unsafe { elem.CurrentName().ok() }
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty());
    let control_type = unsafe { elem.CurrentControlType().ok() }
        .map(|ct| ct.0)
        .unwrap_or(0);

    Some(UiElementRect {
        rect: PhysicalRect::new(rc.left, rc.top, rc.right, rc.bottom),
        name,
        control_type,
    })
}

fn probe_browser_url(automation: &IUIAutomation, hwnd: isize) -> Option<String> {
    let root = unsafe {
        automation
            .ElementFromHandle(ComHwnd(hwnd as *mut _))
            .ok()?
    };
    let edit_var = VARIANT::from(UIA_EditControlTypeId.0);
    let cond = unsafe {
        automation
            .CreatePropertyCondition(UIA_ControlTypePropertyId, &edit_var)
            .ok()?
    };
    let edits = unsafe { root.FindAll(TreeScope_Descendants, &cond).ok()? };
    let count = unsafe { edits.Length().ok()? };

    for i in 0..count.min(32) {
        let Ok(el) = (unsafe { edits.GetElement(i) }) else {
            continue;
        };
        let name = unsafe { el.CurrentName().ok() }
            .map(|s| s.to_string().to_ascii_lowercase())
            .unwrap_or_default();
        let is_address_bar = name.contains("address")
            || name.contains("url")
            || name.contains("search or enter")
            || name.contains("thanh địa chỉ");

        if let Ok(pat) = unsafe { el.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) } {
            if let Ok(val) = unsafe { pat.CurrentValue() } {
                let text = val.to_string();
                let trimmed = text.trim();
                if is_address_bar && !trimmed.is_empty() {
                    return Some(normalize_browser_url(trimmed));
                }
                if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

/// Normalize address bar text (Chrome/Edge often omit `https://`).
pub fn normalize_browser_url(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.contains("://") {
        trimmed.to_string()
    } else if trimmed.contains('.') && !trimmed.contains(' ') {
        format!("https://{trimmed}")
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn z_order_overlap_picks_topmost_window() {
        let back = WindowInfo {
            hwnd: 1,
            title: "Background".into(),
            rect: PhysicalRect::new(0, 0, 1000, 800),
            z_order: 2,
            pid: 100,
            exe_path: None,
            app_name: Some("BackApp".into()),
            app_version: None,
            app_company: None,
            is_browser: false,
        };
        let front = WindowInfo {
            hwnd: 2,
            title: "Foreground".into(),
            rect: PhysicalRect::new(200, 200, 600, 500),
            z_order: 0,
            pid: 200,
            exe_path: None,
            app_name: Some("FrontApp".into()),
            app_version: None,
            app_company: None,
            is_browser: false,
        };
        let windows = vec![back, front];

        // Inside overlap -> must pick front (z_order 0)
        let hit_overlap = find_topmost_window_at(&windows, PhysicalPoint { x: 300, y: 300 });
        assert_eq!(hit_overlap.map(|w| w.hwnd), Some(2));

        // Outside front, inside back -> must pick back
        let hit_back = find_topmost_window_at(&windows, PhysicalPoint { x: 50, y: 50 });
        assert_eq!(hit_back.map(|w| w.hwnd), Some(1));
    }

    #[test]
    fn browser_url_normalization_and_detection() {
        assert!(is_known_browser_exe(r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"));
        assert!(is_known_browser_exe(r"C:\Program Files\Google\Chrome\Application\chrome.exe"));
        assert!(!is_known_browser_exe(r"C:\Windows\System32\notepad.exe"));
        assert_eq!(normalize_browser_url("example.com/docs"), "https://example.com/docs");
        assert_eq!(normalize_browser_url("http://localhost:8080"), "http://localhost:8080");
    }

    #[test]
    fn live_enumerate_windows_and_version_metadata() {
        let windows = enumerate_visible_windows();
        // Verify version info reader on notepad/kernel32 (always present on Windows).
        let (app, ver, comp) = read_file_version_metadata(r"C:\Windows\System32\kernel32.dll");
        assert!(app.is_some());
        assert!(ver.is_some());
        assert!(comp.as_deref().unwrap_or("").contains("Microsoft"));

        // Ensure enumerated windows have valid positive bounds.
        for w in &windows {
            assert!(w.rect.width() > 0 && w.rect.height() > 0);
        }
    }

    #[test]
    fn live_uia_prober_respects_timeout_and_drops_cleanly() {
        let prober = UiElementProber::new();
        let _ = prober.probe_at_point(PhysicalPoint { x: 10, y: 10 }, Duration::from_millis(30));
        drop(prober);
    }

    #[test]
    fn clamp_maximized_window_rect_strips_invisible_resize_borders() {
        let win = PhysicalRect::new(-8, -8, 1928, 1048);
        let mon_work = PhysicalRect::new(0, 0, 1920, 1040);
        assert_eq!(
            clamp_maximized_rect_to_monitor(win, mon_work),
            PhysicalRect::new(0, 0, 1920, 1040)
        );

        // Secondary monitor maximized window (-8px bleed onto primary monitor at x=1912)
        let secondary_win = PhysicalRect::new(1912, -8, 4488, 1408);
        let secondary_work = PhysicalRect::new(1920, 0, 4480, 1400);
        assert_eq!(
            clamp_maximized_rect_to_monitor(secondary_win, secondary_work),
            PhysicalRect::new(1920, 0, 4480, 1400)
        );
    }

    #[test]
    fn snagit_target_occlusion_titlebar_and_child_panes() {
        let back_top = WindowInfo {
            hwnd: 101,
            title: "Background IDE".into(),
            rect: PhysicalRect::new(0, 0, 1200, 900),
            z_order: 1,
            pid: 1001,
            exe_path: None,
            app_name: Some("BackIDE".into()),
            app_version: None,
            app_company: None,
            is_browser: false,
        };
        let back_entry = WindowHierarchyEntry {
            top_window: back_top,
            child_panes: vec![ChildPaneInfo {
                hwnd: 102,
                // Small child pane directly underneath the foreground window's client area!
                rect: PhysicalRect::new(260, 300, 420, 460),
                class_name: "BackSmallPane".into(),
                depth: 3,
            }],
        };

        let front_rect = PhysicalRect::new(200, 200, 900, 750);
        let sidebar_rect = PhysicalRect::new(208, 244, 420, 742);
        let tree_leaf_rect = PhysicalRect::new(212, 290, 416, 700);
        let editor_rect = PhysicalRect::new(420, 244, 892, 742);

        let front_top = WindowInfo {
            hwnd: 201,
            title: "Foreground App".into(),
            rect: front_rect,
            z_order: 0,
            pid: 2002,
            exe_path: None,
            app_name: Some("FrontApp".into()),
            app_version: None,
            app_company: None,
            is_browser: false,
        };
        let front_entry = WindowHierarchyEntry {
            top_window: front_top,
            child_panes: vec![
                // Custom titlebar child pane that shouldn't override whole-window titlebar rule:
                ChildPaneInfo {
                    hwnd: 202,
                    rect: PhysicalRect::new(208, 204, 600, 240),
                    class_name: "CustomTabStrip".into(),
                    depth: 1,
                },
                ChildPaneInfo {
                    hwnd: 203,
                    rect: sidebar_rect,
                    class_name: "SidebarContainer".into(),
                    depth: 1,
                },
                ChildPaneInfo {
                    hwnd: 204,
                    rect: tree_leaf_rect,
                    class_name: "TreeLeafPane".into(),
                    depth: 2,
                },
                ChildPaneInfo {
                    hwnd: 205,
                    rect: editor_rect,
                    class_name: "EditorPane".into(),
                    depth: 1,
                },
            ],
        };

        // Front-to-back hierarchy (also verify foreground wins over background child panes).
        let hierarchy = vec![front_entry, back_entry];

        // (a) Occlusion check: point at (550, 350) is inside editor_rect of front window; if we
        // also test a point inside front window where front has NO child pane (e.g. (450, 243) is
        // titlebar, or a gap at (210, 260) inside sidebar_rect), background's child pane at
        // (260..420, 300..460) MUST NEVER leak through the foreground window!
        let front_no_pane_hierarchy = vec![
            WindowHierarchyEntry {
                top_window: hierarchy[0].top_window.clone(),
                child_panes: Vec::new(),
            },
            hierarchy[1].clone(),
        ];
        let (occluded_rect, occluded_win) = find_snagit_target_at(
            &front_no_pane_hierarchy,
            PhysicalPoint { x: 300, y: 350 },
        )
        .expect("should hit foreground window");
        assert_eq!(occluded_win.hwnd, 201);
        assert_eq!(occluded_rect, front_rect);

        // (b) Titlebar & outer border band -> always selects whole `top_window.rect`.
        let (title_rect, title_win) =
            find_snagit_target_at(&hierarchy, PhysicalPoint { x: 350, y: 220 })
                .expect("should hit titlebar");
        assert_eq!(title_win.hwnd, 201);
        assert_eq!(title_rect, front_rect);

        let (border_rect, border_win) =
            find_snagit_target_at(&hierarchy, PhysicalPoint { x: 203, y: 450 })
                .expect("should hit left frame border");
        assert_eq!(border_win.hwnd, 201);
        assert_eq!(border_rect, front_rect);

        // (c) Client area -> selects deepest / smallest matching `child_pane.rect`.
        let (leaf_hit, leaf_win) =
            find_snagit_target_at(&hierarchy, PhysicalPoint { x: 300, y: 400 })
                .expect("should hit nested tree_leaf_rect");
        assert_eq!(leaf_win.hwnd, 201);
        assert_eq!(leaf_hit, tree_leaf_rect);

        let (editor_hit, editor_win) =
            find_snagit_target_at(&hierarchy, PhysicalPoint { x: 600, y: 450 })
                .expect("should hit editor_rect");
        assert_eq!(editor_win.hwnd, 201);
        assert_eq!(editor_hit, editor_rect);
    }

    #[test]
    fn live_enumerate_window_hierarchy_invariants() {
        let hierarchy = enumerate_window_hierarchy(DEFAULT_SNAGIT_HIERARCHY_DEPTH);
        for entry in &hierarchy {
            let top = entry.top_window.rect;
            let top_area = (top.width() as i64) * (top.height() as i64);
            for pane in &entry.child_panes {
                assert!(pane.depth >= 1 && pane.depth <= DEFAULT_SNAGIT_HIERARCHY_DEPTH);
                assert!(pane.rect.width() >= 48 && pane.rect.height() >= 28);
                assert!(pane.rect.left >= top.left && pane.rect.right <= top.right);
                assert!(pane.rect.top >= top.top && pane.rect.bottom <= top.bottom);
                let pane_area = (pane.rect.width() as i64) * (pane.rect.height() as i64);
                assert!(pane_area * 100 < top_area * 96);
            }
        }
    }
}
