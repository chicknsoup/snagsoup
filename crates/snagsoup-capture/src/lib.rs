//! SnagSoup screen capture engine:
//! - Fast Multi-Monitor `ScreenFreezer` via `IDXGIOutputDuplication` with automatic GDI `BitBlt` fallback
//! - HDR `R16G16B16A16_FLOAT` -> sRGB `RGBA8` tonemapping
//! - Independent movable cursor capture via `snagsoup-win-platform::cursor`
//! - All 8 Snagit selection modes (`Region`, `FixedRegion`, `Window`, `Fullscreen`, `Freehand`, `MultipleArea`, `Menu`, `Clipboard`)
//! - Capture presets (`JSON` + Snagit `Presets4.xml` import) and timed/interval/scheduled capture engine

pub mod dxgi;
pub mod freezer;
pub mod gdi;
pub mod presets;
pub mod scheduler;
pub mod selection_modes;
pub mod tonemap;
pub mod wgc;

pub use freezer::{CaptureBackend, CapturedFrame, FrozenScreen, ScreenFreezer};
pub use presets::{PresetStore, import_snagit_presets4_xml};
pub use scheduler::{CancellationToken, CaptureScheduler, ScheduleMode, hash_frame_rgba};
pub use selection_modes::{
    GuidelineResolution, SelectionResult, SelectionSession, SelectionTarget,
};
pub use wgc::{prewarm_wgc_device_pool, prewarm_wgc_monitor_sessions};
use std::fmt;

/// Returns `true` if every pixel in an RGBA8/BGRA8 buffer has `R == 0 && G == 0 && B == 0`.
/// Fast-paths non-black screens in `< 0.002ms` by checking 512 evenly spaced probe pixels first!
pub fn is_frame_all_black(rgba: &[u8]) -> bool {
    let total_px = rgba.len() / 4;
    if total_px == 0 {
        return true;
    }
    let step = (total_px / 512).max(1);
    let mut idx = 0usize;
    while idx < total_px {
        let off = idx * 4;
        if (rgba[off] | rgba[off + 1] | rgba[off + 2]) != 0 {
            return false;
        }
        idx += step;
    }
    !rgba
        .chunks_exact(4)
        .any(|px| (px[0] | px[1] | px[2]) != 0)
}

/// Fraction (`0.0..=1.0`) of pixels in an RGBA8 buffer that are non-black (`R|G|B != 0`).
pub fn non_black_pixel_ratio(rgba: &[u8]) -> f64 {
    let total = rgba.len() / 4;
    if total == 0 {
        return 0.0;
    }
    let non_black = rgba
        .chunks_exact(4)
        .filter(|px| (px[0] | px[1] | px[2]) != 0)
        .count();
    non_black as f64 / total as f64
}

/// Error type for screen capture operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureError {
    Platform(snagsoup_win_platform::PlatformError),
    Dxgi { api: &'static str, hresult: i32 },
    AccessLost,
    /// Desktop capture denied by OS (e.g., locked screen, UAC secure desktop, or non-interactive session).
    AccessDenied,
    WaitTimeout,
    UnsupportedFormat(u32),
    InvalidBounds,
    PresetParse(String),
    Io(String),
}

impl From<snagsoup_win_platform::PlatformError> for CaptureError {
    fn from(e: snagsoup_win_platform::PlatformError) -> Self {
        Self::Platform(e)
    }
}

impl fmt::Display for CaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Platform(e) => write!(f, "platform error: {e}"),
            Self::Dxgi { api, hresult } => {
                write!(
                    f,
                    "DXGI/D3D11 `{api}` failed with HRESULT 0x{:08X}",
                    *hresult as u32
                )
            }
            Self::AccessLost => write!(f, "DXGI desktop duplication access lost"),
            Self::AccessDenied => {
                write!(f, "screen capture denied (locked or non-interactive desktop)")
            }
            Self::WaitTimeout => write!(f, "DXGI AcquireNextFrame timed out"),
            Self::UnsupportedFormat(fmt_id) => write!(f, "unsupported DXGI_FORMAT ({fmt_id})"),
            Self::InvalidBounds => write!(f, "capture bounds have non-positive width or height"),
            Self::PresetParse(msg) => write!(f, "preset parse error: {msg}"),
            Self::Io(msg) => write!(f, "I/O error: {msg}"),
        }
    }
}

impl std::error::Error for CaptureError {}
