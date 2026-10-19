use core::time::Duration;

/// Limits presentation frequency without postponing the deadline when more updates arrive.
///
/// Callers supply finite, monotonic milliseconds from the same clock, and only wait when dirty
/// regions are pending. This state does not schedule idle wakeups or own the browser timer.
pub(crate) struct FramePacing {
    interval_ms: f64,
    next_presentation_ms: Option<f64>,
}

impl Default for FramePacing {
    fn default() -> Self {
        // Round a 60 FPS interval upward to the nearest nanosecond.
        Self::new(Duration::from_nanos(16_666_667))
    }
}

impl FramePacing {
    pub(crate) fn new(interval: Duration) -> Self {
        Self {
            interval_ms: interval.as_secs_f64() * 1_000.0,
            next_presentation_ms: None,
        }
    }

    /// Returns zero when a pending update can be drawn immediately.
    ///
    /// Positive delays round upward for a millisecond browser timer. Querying the deadline never
    /// advances it, so a continuous stream of updates cannot indefinitely postpone presentation.
    pub(crate) fn delay_ms(&self, now_ms: f64) -> u32 {
        let Some(deadline) = self.next_presentation_ms else {
            return 0;
        };
        let remaining_ms = deadline - now_ms;
        if remaining_ms <= 0.0 {
            return 0;
        }

        let delay = Duration::try_from_secs_f64(remaining_ms / 1_000.0).unwrap_or(Duration::MAX);
        // The minimum also covers a positive delay smaller than Duration's nanosecond resolution.
        u32::try_from(delay.as_nanos().div_ceil(1_000_000))
            .unwrap_or(u32::MAX)
            .max(1)
    }

    /// Records the actual start of presentation, including a late or forced flush.
    ///
    /// Anchoring to the actual time prevents catch-up bursts after a delayed browser callback.
    pub(crate) fn presented(&mut self, now_ms: f64) {
        self.next_presentation_ms = Some(now_ms + self.interval_ms);
    }

    /// Makes the first update after a framebuffer replacement immediately drawable.
    pub(crate) fn reset(&mut self) {
        self.next_presentation_ms = None;
    }
}
