//! Process memory telemetry and active working-set reclamation (`IDXGIDevice3::Trim()`
//! + `K32EmptyWorkingSet` / `SetProcessWorkingSetSizeEx`) for returning from Editor to
//! `< 15 MB` idle tray footprint.

use crate::PlatformError;
use std::mem::size_of;
use windows::Win32::Graphics::Direct3D11::ID3D11Device;
use windows::Win32::Graphics::Dxgi::IDXGIDevice3;
use windows::core::Interface;
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::System::Memory::SetProcessWorkingSetSizeEx;
use windows_sys::Win32::System::ProcessStatus::{
    K32EmptyWorkingSet, K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

/// Snapshot of current process memory usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessMemoryStats {
    /// Current working set in bytes (`WorkingSetSize`).
    pub working_set_bytes: usize,
    /// Peak working set in bytes (`PeakWorkingSetSize`).
    pub peak_working_set_bytes: usize,
    /// Commit charge / pagefile usage in bytes (`PagefileUsage`).
    pub pagefile_usage_bytes: usize,
}

impl ProcessMemoryStats {
    pub fn working_set_mb(&self) -> f64 {
        self.working_set_bytes as f64 / (1024.0 * 1024.0)
    }
}

/// Query current process working-set and commit memory via `K32GetProcessMemoryInfo`.
pub fn current_process_memory() -> Result<ProcessMemoryStats, PlatformError> {
    let mut pmc: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    pmc.cb = size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut pmc, pmc.cb) };
    if ok == 0 {
        let code = unsafe { GetLastError() };
        return Err(PlatformError::Win32 {
            api: "K32GetProcessMemoryInfo",
            code,
        });
    }
    Ok(ProcessMemoryStats {
        working_set_bytes: pmc.WorkingSetSize,
        peak_working_set_bytes: pmc.PeakWorkingSetSize,
        pagefile_usage_bytes: pmc.PagefileUsage,
    })
}

/// Reclaim GPU driver staging caches (`IDXGIDevice3::Trim()`) and trim the OS working set
/// (`SetProcessWorkingSetSizeEx` + `K32EmptyWorkingSet`) when returning to idle tray mode.
pub fn reclaim_process_memory(d3d_device: Option<&ID3D11Device>) -> Result<(), PlatformError> {
    if let Some(dev) = d3d_device {
        if let Ok(dxgi_dev) = dev.cast::<IDXGIDevice3>() {
            unsafe { dxgi_dev.Trim() };
        }
    }

    let proc_handle = unsafe { GetCurrentProcess() };
    unsafe {
        let _ = SetProcessWorkingSetSizeEx(proc_handle, usize::MAX, usize::MAX, 0);
    }
    let ok = unsafe { K32EmptyWorkingSet(proc_handle) };
    if ok == 0 {
        let code = unsafe { GetLastError() };
        return Err(PlatformError::Win32 {
            api: "K32EmptyWorkingSet",
            code,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_stats_and_reclaim_succeed() {
        let before = current_process_memory().expect("memory stats");
        assert!(before.working_set_bytes > 0);
        reclaim_process_memory(None).expect("reclaim should succeed");
        let after = current_process_memory().expect("memory stats after trim");
        assert!(after.working_set_bytes > 0);
    }
}
