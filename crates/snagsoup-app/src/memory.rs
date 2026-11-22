//! Process memory measurement (`K32GetProcessMemoryInfo`) and active
//! working-set reclamation for SnagSoup's Tiered Single-Process Daemon.

use std::mem::size_of;

use windows_sys::Win32::System::ProcessStatus::{
    K32EmptyWorkingSet, K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    PROCESS_MEMORY_COUNTERS_EX,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

/// Snapshot of process memory usage from `K32GetProcessMemoryInfo`.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessMemoryStats {
    /// Resident working set in bytes (`WorkingSetSize`).
    pub working_set_bytes: usize,
    /// Peak resident working set in bytes (`PeakWorkingSetSize`).
    pub peak_working_set_bytes: usize,
    /// Private committed bytes (`PrivateUsage`).
    pub private_usage_bytes: usize,
}

impl ProcessMemoryStats {
    pub fn working_set_mb(self) -> f64 {
        self.working_set_bytes as f64 / (1024.0 * 1024.0)
    }

    pub fn peak_working_set_mb(self) -> f64 {
        self.peak_working_set_bytes as f64 / (1024.0 * 1024.0)
    }

    pub fn private_usage_mb(self) -> f64 {
        self.private_usage_bytes as f64 / (1024.0 * 1024.0)
    }
}

/// Query the current process's memory counters via `K32GetProcessMemoryInfo`.
pub fn query_process_memory() -> Result<ProcessMemoryStats, u32> {
    let mut counters = PROCESS_MEMORY_COUNTERS_EX {
        cb: size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        PageFaultCount: 0,
        PeakWorkingSetSize: 0,
        WorkingSetSize: 0,
        QuotaPeakPagedPoolUsage: 0,
        QuotaPagedPoolUsage: 0,
        QuotaPeakNonPagedPoolUsage: 0,
        QuotaNonPagedPoolUsage: 0,
        PagefileUsage: 0,
        PeakPagefileUsage: 0,
        PrivateUsage: 0,
    };

    // SAFETY: `GetCurrentProcess()` returns a valid pseudo-handle; `counters` is properly
    // sized and aligned for `K32GetProcessMemoryInfo` with `PROCESS_MEMORY_COUNTERS_EX`.
    let ok = unsafe {
        K32GetProcessMemoryInfo(
            GetCurrentProcess(),
            (&mut counters as *mut PROCESS_MEMORY_COUNTERS_EX).cast::<PROCESS_MEMORY_COUNTERS>(),
            counters.cb,
        )
    };

    if ok == 0 {
        return Err(unsafe { windows_sys::Win32::Foundation::GetLastError() });
    }

    Ok(ProcessMemoryStats {
        working_set_bytes: counters.WorkingSetSize,
        peak_working_set_bytes: counters.PeakWorkingSetSize,
        private_usage_bytes: counters.PrivateUsage,
    })
}

/// Actively trim the process working set when returning to Tier 0 idle tray mode.
///
/// Drops unused pages back to the OS so idle working set stays `< 15 MB`.
pub fn reclaim_process_memory() -> bool {
    // SAFETY: `GetCurrentProcess()` is always a valid pseudo-handle.
    unsafe { K32EmptyWorkingSet(GetCurrentProcess()) != 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_memory_returns_nonzero_working_set() {
        let stats = query_process_memory().expect("K32GetProcessMemoryInfo should succeed");
        assert!(stats.working_set_bytes > 0);
        assert!(stats.private_usage_bytes > 0);
    }
}
