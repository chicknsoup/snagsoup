//! Stepwise `AutoScrollController`: sends scroll pulses (~60% viewport), waits for
//! smooth-scrolling animations to settle via fast frame-diff, and stops automatically
//! at end-of-page (2 consecutive zero-shift scrolls) or cancellation.

use crate::phase_correlator::PhaseCorrelator;
use crate::sticky_mask::StickyBorderMasker;
use crate::{RgbaFrame, StitchError};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

/// Configuration for stepwise automatic scrolling capture.
#[derive(Debug, Clone, Copy)]
pub struct AutoScrollConfig {
    /// Maximum scroll pulses before stopping safely.
    pub max_steps: usize,
    /// Mean absolute RGB diff below which smooth-scrolling is considered settled.
    pub settle_diff_threshold: f32,
    /// Delay between consecutive settle checks (e.g. 25 ms; `Duration::ZERO` in tests).
    pub settle_poll_interval: Duration,
    /// Maximum settle polls per scroll step.
    pub max_settle_checks: usize,
    /// Number of consecutive zero-shift scroll steps that indicate end-of-page.
    pub zero_shift_stop_count: usize,
}

impl Default for AutoScrollConfig {
    fn default() -> Self {
        Self {
            max_steps: 120,
            settle_diff_threshold: 0.75,
            settle_poll_interval: Duration::from_millis(30),
            max_settle_checks: 10,
            zero_shift_stop_count: 2,
        }
    }
}

/// Reason why `AutoScrollController` terminated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoScrollStopReason {
    EndOfPage,
    Cancelled,
    MaxStepsReached,
}

/// Stepwise scrolling capture controller.
pub struct AutoScrollController {
    pub config: AutoScrollConfig,
}

impl AutoScrollController {
    pub fn new(config: AutoScrollConfig) -> Self {
        Self { config }
    }

    /// Execute the stepwise auto-scroll loop using caller-supplied `capture_frame` and
    /// `pulse_scroll` callbacks (decoupled from Win32 for deterministic unit testing).
    pub fn run<C, S, E>(
        &self,
        cancel: &Arc<AtomicBool>,
        mut capture_frame: C,
        mut pulse_scroll: S,
    ) -> Result<(Vec<RgbaFrame>, AutoScrollStopReason), E>
    where
        C: FnMut() -> Result<RgbaFrame, E>,
        S: FnMut() -> Result<(), E>,
    {
        let mut frames = Vec::new();
        let initial = capture_frame()?;
        frames.push(initial);

        let mut correlator = PhaseCorrelator::new();
        let mut zero_shift_streak = 0usize;

        for _step in 0..self.config.max_steps {
            if cancel.load(Ordering::SeqCst) {
                return Ok((frames, AutoScrollStopReason::Cancelled));
            }

            pulse_scroll()?;

            // Wait for smooth-scrolling to settle.
            let settled = self.wait_until_settled(cancel, &mut capture_frame)?;
            if cancel.load(Ordering::SeqCst) {
                return Ok((frames, AutoScrollStopReason::Cancelled));
            }

            let prev = frames.last().unwrap();
            let sticky = StickyBorderMasker::detect_between(prev, &settled, 2);
            let is_zero_shift = if prev.mean_abs_diff(&settled) < self.config.settle_diff_threshold
            {
                true
            } else {
                match correlator.estimate(prev, &settled, sticky) {
                    Ok(est) => est.dx == 0 && est.dy == 0,
                    Err(StitchError::InsufficientUnmaskedArea) => true,
                    Err(_) => false,
                }
            };

            if is_zero_shift {
                zero_shift_streak += 1;
                if zero_shift_streak >= self.config.zero_shift_stop_count {
                    return Ok((frames, AutoScrollStopReason::EndOfPage));
                }
            } else {
                zero_shift_streak = 0;
                frames.push(settled);
            }
        }

        Ok((frames, AutoScrollStopReason::MaxStepsReached))
    }

    fn wait_until_settled<C, E>(
        &self,
        cancel: &Arc<AtomicBool>,
        capture_frame: &mut C,
    ) -> Result<RgbaFrame, E>
    where
        C: FnMut() -> Result<RgbaFrame, E>,
    {
        let mut prev = capture_frame()?;
        for _ in 0..self.config.max_settle_checks {
            if cancel.load(Ordering::SeqCst) {
                return Ok(prev);
            }
            if !self.config.settle_poll_interval.is_zero() {
                thread::sleep(self.config.settle_poll_interval);
            }
            let curr = capture_frame()?;
            let diff = prev.mean_abs_diff(&curr);
            prev = curr;
            if diff <= self.config.settle_diff_threshold {
                break;
            }
        }
        Ok(prev)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_frame_with_seed(w: u32, h: u32, scroll_y: u32) -> RgbaFrame {
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            let gy = y + scroll_y;
            for x in 0..w {
                let idx = ((y * w + x) * 4) as usize;
                let v = (x.wrapping_mul(13) ^ gy.wrapping_mul(29)) as u8;
                rgba[idx] = v;
                rgba[idx + 1] = v.wrapping_add((gy / 8) as u8);
                rgba[idx + 2] = 180;
                rgba[idx + 3] = 255;
            }
        }
        RgbaFrame::new(w, h, rgba).unwrap()
    }

    #[test]
    fn auto_scroll_stops_cleanly_at_end_of_page() {
        let controller = AutoScrollController::new(AutoScrollConfig {
            max_steps: 20,
            settle_diff_threshold: 0.5,
            settle_poll_interval: Duration::ZERO,
            max_settle_checks: 2,
            zero_shift_stop_count: 2,
        });

        let cancel = Arc::new(AtomicBool::new(false));
        let scroll_y = std::cell::Cell::new(0u32);
        let max_scroll_y = 300u32;

        let (frames, reason) = controller
            .run(
                &cancel,
                || Ok::<RgbaFrame, ()>(make_frame_with_seed(160, 200, scroll_y.get())),
                || {
                    scroll_y.set((scroll_y.get() + 100).min(max_scroll_y));
                    Ok::<(), ()>(())
                },
            )
            .expect("auto-scroll should succeed");

        assert_eq!(reason, AutoScrollStopReason::EndOfPage);
        // Distinct frames at scroll_y = 0, 100, 200, 300 -> 4 frames captured!
        assert_eq!(frames.len(), 4);
    }
}
