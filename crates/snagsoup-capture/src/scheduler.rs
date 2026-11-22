//! Timed capture scheduler supporting:
//! - `Delay` (1–60s countdown for Menu captures)
//! - `Interval` (periodic capture with ultra-fast `xxh3_64` `discard_identical` deduplication)
//! - `Scheduled` (target wall-clock `SystemTime`)
//! All modes support clean cooperative cancellation via `CancellationToken`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, SystemTime};
use xxhash_rust::xxh3::xxh3_64;

/// Thread-safe cooperative cancellation token for scheduled captures.
#[derive(Debug, Clone, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

/// Scheduling mode for timed captures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleMode {
    /// Countdown delay (1..=60 seconds) before capturing.
    Delay { seconds: u32 },
    /// Periodic interval capture with optional `xxh3_64` identical-frame deduplication.
    Interval {
        interval: Duration,
        max_captures: Option<usize>,
        discard_identical: bool,
    },
    /// Wait until a specific wall-clock `SystemTime` before capturing.
    Scheduled { target_time: SystemTime },
}

/// Ultra-fast 64-bit `XXH3` hash over a raw RGBA8 frame buffer for deduplication.
#[inline]
pub fn hash_frame_rgba(rgba: &[u8]) -> u64 {
    xxh3_64(rgba)
}

/// Stateful capture scheduler.
#[derive(Debug, Default)]
pub struct CaptureScheduler;

impl CaptureScheduler {
    pub fn new() -> Self {
        Self
    }

    /// Run a countdown delay (`1..=60s`), invoking `on_tick(remaining_seconds)` each second.
    /// Returns `true` if the countdown finished, or `false` if cancelled.
    pub fn run_delay<F>(
        &self,
        seconds: u32,
        cancel: &CancellationToken,
        mut on_tick: F,
    ) -> bool
    where
        F: FnMut(u32),
    {
        let clamped = seconds.min(60);
        for remaining in (1..=clamped).rev() {
            if cancel.is_cancelled() {
                return false;
            }
            on_tick(remaining);
            if !sleep_cancellable(Duration::from_secs(1), cancel) {
                return false;
            }
        }
        !cancel.is_cancelled()
    }

    /// Run an interval capture loop invoking `capture_fn()` up to `max_ticks` times.
    /// When `discard_identical` is `true`, consecutive frames with the same `xxh3_64` hash
    /// are dropped so only distinct frames are returned.
    pub fn run_interval<F, E>(
        &self,
        interval: Duration,
        max_ticks: usize,
        discard_identical: bool,
        cancel: &CancellationToken,
        mut capture_fn: F,
    ) -> Result<Vec<Vec<u8>>, E>
    where
        F: FnMut(usize) -> Result<Vec<u8>, E>,
    {
        let mut results = Vec::new();
        let mut last_hash: Option<u64> = None;

        for tick in 0..max_ticks {
            if cancel.is_cancelled() {
                break;
            }
            let frame = capture_fn(tick)?;
            if discard_identical {
                let h = hash_frame_rgba(&frame);
                if last_hash != Some(h) {
                    last_hash = Some(h);
                    results.push(frame);
                }
            } else {
                results.push(frame);
            }

            if tick + 1 < max_ticks && !interval.is_zero() {
                if !sleep_cancellable(interval, cancel) {
                    break;
                }
            }
        }

        Ok(results)
    }

    /// Sleep until `target_time` (checking `cancel` every 50 ms).
    /// Returns `true` when `target_time` is reached, or `false` if cancelled.
    pub fn wait_until_scheduled(
        &self,
        target_time: SystemTime,
        cancel: &CancellationToken,
    ) -> bool {
        loop {
            if cancel.is_cancelled() {
                return false;
            }
            match target_time.duration_since(SystemTime::now()) {
                Ok(remaining) if !remaining.is_zero() => {
                    let step = remaining.min(Duration::from_millis(50));
                    thread::sleep(step);
                }
                _ => return true,
            }
        }
    }
}

fn sleep_cancellable(duration: Duration, cancel: &CancellationToken) -> bool {
    let mut remaining = duration;
    let step = Duration::from_millis(25);
    while !remaining.is_zero() {
        if cancel.is_cancelled() {
            return false;
        }
        let nap = remaining.min(step);
        thread::sleep(nap);
        remaining = remaining.saturating_sub(nap);
    }
    !cancel.is_cancelled()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_discard_identical_filters_5_frames_to_3_unique() {
        let scheduler = CaptureScheduler::new();
        let cancel = CancellationToken::new();

        // 5 consecutive frames where frames 1 & 2 are identical to frame 0,
        // frame 3 is different, and frame 4 is different:
        // [A, A, A, B, C] -> exactly 3 unique frames emitted!
        let frames = vec![
            vec![10u8; 256],
            vec![10u8; 256],
            vec![10u8; 256],
            vec![20u8; 256],
            vec![30u8; 256],
        ];

        let kept = scheduler
            .run_interval(
                Duration::ZERO,
                frames.len(),
                true,
                &cancel,
                |idx| Ok::<Vec<u8>, ()>(frames[idx].clone()),
            )
            .expect("interval should succeed");

        assert_eq!(kept.len(), 3);
        assert_eq!(kept[0][0], 10);
        assert_eq!(kept[1][0], 20);
        assert_eq!(kept[2][0], 30);
    }

    #[test]
    fn interval_cancellation_stops_early() {
        let scheduler = CaptureScheduler::new();
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        let kept = scheduler
            .run_interval(Duration::ZERO, 10, false, &cancel, |idx| {
                if idx == 2 {
                    cancel_clone.cancel();
                }
                Ok::<Vec<u8>, ()>(vec![idx as u8; 16])
            })
            .expect("interval should succeed");

        assert_eq!(kept.len(), 3);
    }

    #[test]
    fn scheduled_past_time_triggers_immediately() {
        let scheduler = CaptureScheduler::new();
        let cancel = CancellationToken::new();
        let past = SystemTime::now() - Duration::from_secs(1);
        assert!(scheduler.wait_until_scheduled(past, &cancel));
    }
}
