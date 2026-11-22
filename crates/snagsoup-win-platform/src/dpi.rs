//! Per-Monitor DPI v2 initialization, live monitor enumeration, and
//! physical <-> logical coordinate mapping across mixed-DPI displays.

use crate::PlatformError;
use std::mem::size_of;
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::{BOOL, ERROR_ACCESS_DENIED, GetLastError, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW,
};
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, MDT_EFFECTIVE_DPI,
    SetProcessDpiAwarenessContext,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetCursorPos, MONITORINFOF_PRIMARY};

/// Standard Windows baseline DPI (100% scaling).
pub const BASE_DPI: u32 = 96;

/// A point in physical device pixels (Windows virtual screen space).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PhysicalPoint {
    pub x: i32,
    pub y: i32,
}

/// A rectangle in physical device pixels (left/top inclusive, right/bottom exclusive).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PhysicalRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl PhysicalRect {
    pub const fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    pub const fn width(&self) -> i32 {
        self.right - self.left
    }

    pub const fn height(&self) -> i32 {
        self.bottom - self.top
    }

    pub const fn contains_point(&self, pt: PhysicalPoint) -> bool {
        pt.x >= self.left && pt.x < self.right && pt.y >= self.top && pt.y < self.bottom
    }

    pub fn union(&self, other: &Self) -> Self {
        Self {
            left: self.left.min(other.left),
            top: self.top.min(other.top),
            right: self.right.max(other.right),
            bottom: self.bottom.max(other.bottom),
        }
    }
}

/// A point in DPI-independent logical coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LogicalPoint {
    pub x: f32,
    pub y: f32,
}

/// A rectangle in DPI-independent logical coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LogicalRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl LogicalRect {
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn contains_point(&self, pt: LogicalPoint) -> bool {
        pt.x >= self.x
            && pt.x < self.x + self.width
            && pt.y >= self.y
            && pt.y < self.y + self.height
    }
}

/// Metadata and geometry for a single display monitor.
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorInfo {
    /// Raw `HMONITOR` handle value (or synthetic ID in tests).
    pub id: isize,
    /// Raw Win32 `HMONITOR` handle value (same as `id`, exposed explicitly for WGC/Win32 interop).
    pub hmonitor: isize,
    /// Win32 device name (e.g. `\\.\DISPLAY1`).
    pub name: String,
    /// Win32 device name from `MONITORINFOEXW.szDevice` (e.g. `\\.\DISPLAY1`).
    pub device_name: String,
    /// Whether this is the primary Windows monitor.
    pub is_primary: bool,
    /// Full monitor bounds in physical pixels.
    pub physical_rect: PhysicalRect,
    /// Usable work area (excluding taskbar) in physical pixels.
    pub work_area: PhysicalRect,
    /// Effective horizontal DPI (e.g. 96, 144, 192).
    pub dpi_x: u32,
    /// Effective vertical DPI.
    pub dpi_y: u32,
    /// Scale factor (`dpi_x / 96.0`).
    pub scale_factor: f32,
    /// Logical bounds anchored at the monitor's physical origin with DPI-scaled dimensions.
    pub logical_rect: LogicalRect,
}

impl MonitorInfo {
    /// Construct a `MonitorInfo` from physical geometry and DPI values.
    pub fn from_geometry(
        id: isize,
        name: String,
        is_primary: bool,
        physical_rect: PhysicalRect,
        work_area: PhysicalRect,
        dpi_x: u32,
        dpi_y: u32,
    ) -> Self {
        let dpi_x = if dpi_x == 0 { BASE_DPI } else { dpi_x };
        let dpi_y = if dpi_y == 0 { BASE_DPI } else { dpi_y };
        let scale_factor = dpi_x as f32 / BASE_DPI as f32;
        let logical_rect = LogicalRect::new(
            physical_rect.left as f32,
            physical_rect.top as f32,
            physical_rect.width() as f32 / scale_factor,
            physical_rect.height() as f32 / scale_factor,
        );
        let device_name = name.clone();
        Self {
            id,
            hmonitor: id,
            name,
            device_name,
            is_primary,
            physical_rect,
            work_area,
            dpi_x,
            dpi_y,
            scale_factor,
            logical_rect,
        }
    }

    /// Convert a physical point on this monitor to monitor-local logical coordinates.
    pub fn physical_to_local_logical(&self, pt: PhysicalPoint) -> LogicalPoint {
        LogicalPoint {
            x: (pt.x - self.physical_rect.left) as f32 / self.scale_factor,
            y: (pt.y - self.physical_rect.top) as f32 / self.scale_factor,
        }
    }

    /// Convert monitor-local logical coordinates to a physical point.
    pub fn local_logical_to_physical(&self, pt: LogicalPoint) -> PhysicalPoint {
        PhysicalPoint {
            x: self.physical_rect.left + (pt.x * self.scale_factor).round() as i32,
            y: self.physical_rect.top + (pt.y * self.scale_factor).round() as i32,
        }
    }
}

/// Snapshot of the multi-monitor virtual desktop topology.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayTopology {
    /// Bounding union of all monitors in physical virtual-screen coordinates.
    pub virtual_bounds: PhysicalRect,
    /// All active monitors.
    pub monitors: Vec<MonitorInfo>,
}

impl DisplayTopology {
    /// Build a `DisplayTopology` from a list of monitors (useful for both live enumeration and tests).
    pub fn from_monitors(monitors: Vec<MonitorInfo>) -> Result<Self, PlatformError> {
        let first = monitors.first().ok_or(PlatformError::NoMonitorsFound)?;
        let mut virtual_bounds = first.physical_rect;
        for m in &monitors[1..] {
            virtual_bounds = virtual_bounds.union(&m.physical_rect);
        }
        Ok(Self {
            virtual_bounds,
            monitors,
        })
    }

    /// Return the primary monitor, falling back to the first monitor if none is marked primary.
    pub fn primary_monitor(&self) -> Option<&MonitorInfo> {
        self.monitors
            .iter()
            .find(|m| m.is_primary)
            .or_else(|| self.monitors.first())
    }

    /// Find the monitor containing `pt` in physical coordinates.
    pub fn monitor_at_physical(&self, pt: PhysicalPoint) -> Option<&MonitorInfo> {
        self.monitors
            .iter()
            .find(|m| m.physical_rect.contains_point(pt))
    }

    /// Find the monitor containing `pt` in logical coordinates.
    pub fn monitor_at_logical(&self, pt: LogicalPoint) -> Option<&MonitorInfo> {
        self.monitors
            .iter()
            .find(|m| m.logical_rect.contains_point(pt))
    }

    /// Convert a physical virtual-desktop point to topology logical coordinates.
    pub fn physical_to_logical(&self, pt: PhysicalPoint) -> LogicalPoint {
        if let Some(m) = self.monitor_at_physical(pt).or_else(|| self.primary_monitor()) {
            let local = m.physical_to_local_logical(pt);
            LogicalPoint {
                x: m.logical_rect.x + local.x,
                y: m.logical_rect.y + local.y,
            }
        } else {
            LogicalPoint {
                x: pt.x as f32,
                y: pt.y as f32,
            }
        }
    }

    /// Convert a topology logical point back to physical virtual-desktop coordinates.
    pub fn logical_to_physical(&self, pt: LogicalPoint) -> PhysicalPoint {
        if let Some(m) = self.monitor_at_logical(pt).or_else(|| self.primary_monitor()) {
            let local = LogicalPoint {
                x: pt.x - m.logical_rect.x,
                y: pt.y - m.logical_rect.y,
            };
            m.local_logical_to_physical(local)
        } else {
            PhysicalPoint {
                x: pt.x.round() as i32,
                y: pt.y.round() as i32,
            }
        }
    }

    /// Convert a physical rectangle to a logical rectangle using the scale of the monitor
    /// containing its top-left corner.
    pub fn physical_rect_to_logical(&self, rect: PhysicalRect) -> LogicalRect {
        let origin = PhysicalPoint {
            x: rect.left,
            y: rect.top,
        };
        let scale = self
            .monitor_at_physical(origin)
            .or_else(|| self.primary_monitor())
            .map(|m| m.scale_factor)
            .unwrap_or(1.0);
        let top_left = self.physical_to_logical(origin);
        LogicalRect::new(
            top_left.x,
            top_left.y,
            rect.width() as f32 / scale,
            rect.height() as f32 / scale,
        )
    }

    /// Convert a logical rectangle back to a physical rectangle using the scale of the monitor
    /// containing its top-left corner.
    pub fn logical_rect_to_physical(&self, rect: LogicalRect) -> PhysicalRect {
        let origin = LogicalPoint {
            x: rect.x,
            y: rect.y,
        };
        let scale = self
            .monitor_at_logical(origin)
            .or_else(|| self.primary_monitor())
            .map(|m| m.scale_factor)
            .unwrap_or(1.0);
        let top_left = self.logical_to_physical(origin);
        let width = (rect.width * scale).round() as i32;
        let height = (rect.height * scale).round() as i32;
        PhysicalRect::new(
            top_left.x,
            top_left.y,
            top_left.x + width,
            top_left.y + height,
        )
    }
}

/// Enable Per-Monitor DPI Awareness v2 for the current process.
///
/// Returns `Ok(true)` if newly set, `Ok(false)` if DPI awareness was already configured
/// (e.g. via application manifest or an earlier call), or `Err` on unexpected Win32 failure.
pub fn init_per_monitor_dpi_v2() -> Result<bool, PlatformError> {
    // SAFETY: `SetProcessDpiAwarenessContext` is a process-wide Win32 API taking a constant handle.
    let ok = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    if ok != 0 {
        return Ok(true);
    }
    let err = unsafe { GetLastError() };
    if err == ERROR_ACCESS_DENIED {
        // Already configured for this process.
        return Ok(false);
    }
    Err(PlatformError::Win32 {
        api: "SetProcessDpiAwarenessContext",
        code: err,
    })
}

/// Enumerate all currently active display monitors and their effective DPIs.
pub fn enumerate_monitors() -> Result<DisplayTopology, PlatformError> {
    let mut monitors: Vec<MonitorInfo> = Vec::new();
    let lparam = (&mut monitors as *mut Vec<MonitorInfo>) as LPARAM;

    // SAFETY: `monitor_enum_proc` receives a valid pointer to `monitors` for the duration of
    // `EnumDisplayMonitors`, which runs synchronously on the calling thread.
    let ok = unsafe { EnumDisplayMonitors(null_mut(), null_mut(), Some(monitor_enum_proc), lparam) };
    if ok == 0 {
        let code = unsafe { GetLastError() };
        return Err(PlatformError::Win32 {
            api: "EnumDisplayMonitors",
            code,
        });
    }

    DisplayTopology::from_monitors(monitors)
}

/// Return the monitor currently containing the mouse cursor, falling back to the primary monitor.
pub fn monitor_at_cursor() -> Result<MonitorInfo, PlatformError> {
    let topology = enumerate_monitors()?;
    let mut pt = POINT { x: 0, y: 0 };
    if unsafe { GetCursorPos(&mut pt) } != 0 {
        if let Some(m) = topology.monitor_at_physical(PhysicalPoint { x: pt.x, y: pt.y }) {
            return Ok(m.clone());
        }
    }
    topology
        .primary_monitor()
        .cloned()
        .ok_or(PlatformError::NoMonitorsFound)
}

unsafe extern "system" fn monitor_enum_proc(
    hmonitor: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    let monitors = unsafe { &mut *(lparam as *mut Vec<MonitorInfo>) };

    let mut info: MONITORINFOEXW = unsafe { std::mem::zeroed() };
    info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;

    if unsafe { GetMonitorInfoW(hmonitor, &mut info.monitorInfo as *mut _ as *mut _) } != 0 {
        let mut dpi_x: u32 = BASE_DPI;
        let mut dpi_y: u32 = BASE_DPI;
        let hr = unsafe { GetDpiForMonitor(hmonitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
        if hr != 0 {
            dpi_x = BASE_DPI;
            dpi_y = BASE_DPI;
        }

        let name_len = info
            .szDevice
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(info.szDevice.len());
        let name = String::from_utf16_lossy(&info.szDevice[..name_len]);

        let rc = info.monitorInfo.rcMonitor;
        let work = info.monitorInfo.rcWork;
        let is_primary = (info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY) != 0;

        monitors.push(MonitorInfo::from_geometry(
            hmonitor as isize,
            name,
            is_primary,
            PhysicalRect::new(rc.left, rc.top, rc.right, rc.bottom),
            PhysicalRect::new(work.left, work.top, work.right, work.bottom),
            dpi_x,
            dpi_y,
        ));
    }

    1 // TRUE: continue enumeration
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mixed_dpi_topology() -> DisplayTopology {
        // Primary monitor: 1920x1080 @ 100% (96 DPI)
        let m1 = MonitorInfo::from_geometry(
            1,
            r"\\.\DISPLAY1".to_string(),
            true,
            PhysicalRect::new(0, 0, 1920, 1080),
            PhysicalRect::new(0, 0, 1920, 1040),
            96,
            96,
        );
        // Secondary monitor to the right: 2560x1440 @ 150% (144 DPI)
        let m2 = MonitorInfo::from_geometry(
            2,
            r"\\.\DISPLAY2".to_string(),
            false,
            PhysicalRect::new(1920, 0, 4480, 1440),
            PhysicalRect::new(1920, 0, 4480, 1400),
            144,
            144,
        );
        DisplayTopology::from_monitors(vec![m1, m2]).expect("valid topology")
    }

    #[test]
    fn mixed_dpi_virtual_bounds_and_scales() {
        let topo = mixed_dpi_topology();
        assert_eq!(topo.virtual_bounds, PhysicalRect::new(0, 0, 4480, 1440));
        assert_eq!(topo.monitors[0].scale_factor, 1.0);
        assert_eq!(topo.monitors[1].scale_factor, 1.5);
        assert_eq!(
            topo.monitors[0].logical_rect,
            LogicalRect::new(0.0, 0.0, 1920.0, 1080.0)
        );
        assert!((topo.monitors[1].logical_rect.width - 1706.6666).abs() < 0.01);
        assert!((topo.monitors[1].logical_rect.height - 960.0).abs() < 0.01);
    }

    #[test]
    fn mixed_dpi_point_roundtrip() {
        let topo = mixed_dpi_topology();

        // Point on 100% monitor
        let p1 = PhysicalPoint { x: 500, y: 300 };
        let l1 = topo.physical_to_logical(p1);
        assert_eq!(l1, LogicalPoint { x: 500.0, y: 300.0 });
        assert_eq!(topo.logical_to_physical(l1), p1);

        // Point on 150% monitor: +150px right, +300px down from monitor origin (1920, 0)
        let p2 = PhysicalPoint { x: 2070, y: 300 };
        let l2 = topo.physical_to_logical(p2);
        assert_eq!(l2, LogicalPoint { x: 2020.0, y: 200.0 });
        assert_eq!(topo.logical_to_physical(l2), p2);
    }

    #[test]
    fn mixed_dpi_rect_roundtrip() {
        let topo = mixed_dpi_topology();
        let phys = PhysicalRect::new(2070, 150, 2370, 450); // 300x300 physical on 150% display
        let logical = topo.physical_rect_to_logical(phys);
        assert_eq!(logical, LogicalRect::new(2020.0, 100.0, 200.0, 200.0));
        assert_eq!(topo.logical_rect_to_physical(logical), phys);
    }

    #[test]
    fn live_dpi_init_and_monitor_enumeration() {
        let _ = init_per_monitor_dpi_v2().expect("DPI init should succeed or report already set");
        let topo = enumerate_monitors().expect("should enumerate at least one live monitor");
        assert!(!topo.monitors.is_empty());
        assert!(topo.virtual_bounds.width() > 0);
        assert!(topo.virtual_bounds.height() > 0);
        for m in &topo.monitors {
            assert!(m.dpi_x >= BASE_DPI);
            assert!(m.scale_factor >= 1.0);
            assert_eq!(m.hmonitor, m.id);
            assert_ne!(m.hmonitor, 0);
            assert!(!m.device_name.is_empty());
            assert_eq!(m.device_name, m.name);
        }
        let cursor_mon = monitor_at_cursor().expect("monitor_at_cursor should succeed");
        assert!(topo.monitors.iter().any(|m| m.hmonitor == cursor_mon.hmonitor));
    }
}
