//! SnagSoup scrolling & panoramic image stitching engine:
//! - `sticky_mask`: detects stationary top sticky headers, bottom footers, and scrollbars
//! - `phase_correlator`: 2D FFT Phase Correlation (`rustfft`) + full-res 5x5 SAD refinement (`0 px` error)
//! - `auto_scroll`: stepwise smooth-scroll controller with end-of-page detection
//! - `panoramic`: sparse 256x256 2D tile grid (`HashMap<(i32, i32), ...>`) with `< 1 ms` HUD preview

pub mod auto_scroll;
pub mod panoramic;
pub mod phase_correlator;
pub mod sticky_mask;

pub use auto_scroll::{AutoScrollConfig, AutoScrollController, AutoScrollStopReason};
pub use panoramic::{PanoramicSession, PushOutcome};
pub use phase_correlator::{PhaseCorrelator, ShiftEstimate, stitch_scrolling_sequence};
pub use sticky_mask::{StickyBorderMasker, StickyBorders};
use std::fmt;

/// Straight RGBA8 image frame used across stitching pipelines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl RgbaFrame {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Result<Self, StitchError> {
        let expected = (width as usize) * (height as usize) * 4;
        if width == 0 || height == 0 || rgba.len() != expected {
            return Err(StitchError::InvalidFrame);
        }
        Ok(Self {
            width,
            height,
            rgba,
        })
    }

    pub fn filled(width: u32, height: u32, color: [u8; 4]) -> Self {
        let count = (width as usize) * (height as usize);
        let mut rgba = Vec::with_capacity(count * 4);
        for _ in 0..count {
            rgba.extend_from_slice(&color);
        }
        Self {
            width,
            height,
            rgba,
        }
    }

    #[inline]
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let idx = ((y as usize) * (self.width as usize) + (x as usize)) * 4;
        [
            self.rgba[idx],
            self.rgba[idx + 1],
            self.rgba[idx + 2],
            self.rgba[idx + 3],
        ]
    }

    /// Fast strided mean absolute RGB difference between two same-sized frames.
    pub fn mean_abs_diff(&self, other: &Self) -> f32 {
        if self.width != other.width || self.height != other.height {
            return 255.0;
        }
        let step_x = (self.width / 64).max(1) as usize;
        let step_y = (self.height / 64).max(1) as usize;
        let w = self.width as usize;
        let h = self.height as usize;
        let mut sum: u64 = 0;
        let mut samples: u64 = 0;

        let mut y = 0;
        while y < h {
            let mut x = 0;
            while x < w {
                let idx = (y * w + x) * 4;
                sum += self.rgba[idx].abs_diff(other.rgba[idx]) as u64;
                sum += self.rgba[idx + 1].abs_diff(other.rgba[idx + 1]) as u64;
                sum += self.rgba[idx + 2].abs_diff(other.rgba[idx + 2]) as u64;
                samples += 3;
                x += step_x;
            }
            y += step_y;
        }
        if samples == 0 {
            0.0
        } else {
            sum as f32 / samples as f32
        }
    }
}

/// Errors produced by stitching pipelines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StitchError {
    InvalidFrame,
    EmptySequence,
    DimensionMismatch,
    InsufficientUnmaskedArea,
}

impl fmt::Display for StitchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFrame => write!(f, "invalid frame dimensions or buffer length"),
            Self::EmptySequence => write!(f, "cannot stitch an empty sequence of frames"),
            Self::DimensionMismatch => write!(f, "frames in scrolling sequence must share dimensions"),
            Self::InsufficientUnmaskedArea => {
                write!(f, "unmasked interior area is too small for phase correlation")
            }
        }
    }
}

impl std::error::Error for StitchError {}
