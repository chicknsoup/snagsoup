//! Win32 platform layer for SnagSoup: Per-Monitor DPI v2, monitor topology,
//! global hotkeys, cursor bitmap extraction, window/UIAutomation probing,
//! system tray, multi-format clipboard, and process memory reclamation.

pub mod clipboard;
pub mod cursor;
pub mod dpi;
pub mod drag_drop;
pub mod hotkey;
pub mod memory;
pub mod object_finder;
pub mod office_com;
pub mod scroll;
pub mod startup;
pub mod tray;

use std::fmt;

/// Error type for Win32 platform operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlatformError {
    /// A Win32 API call failed with `GetLastError()` code.
    Win32 { api: &'static str, code: u32 },
    /// No display monitors were found.
    NoMonitorsFound,
    /// A background Win32 message loop thread exited unexpectedly.
    HotkeyThreadStopped,
    /// An image encoding/decoding error occurred (e.g. PNG clipboard).
    ImageCodec(String),
    /// A friendly COM Automation error (e.g. Office app not installed or busy).
    ComAutomation(String),
}

impl fmt::Display for PlatformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Win32 { api, code } => {
                write!(f, "Win32 API `{api}` failed with error code {code} (0x{code:08X})")
            }
            Self::NoMonitorsFound => write!(f, "no display monitors found"),
            Self::HotkeyThreadStopped => write!(f, "background Win32 thread is not running"),
            Self::ImageCodec(msg) => write!(f, "image codec error: {msg}"),
            Self::ComAutomation(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for PlatformError {}
