use ironrdp::pdu::geometry::InclusiveRectangle;
use ironrdp_core::WriteBuf;

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct PresentationStats {
    pub(crate) attempted_regions: usize,
    pub(crate) failed_regions: usize,
    pub(crate) uploaded_bytes: u64,
}

/// Preserve a protocol/input failure when best-effort final presentation also fails.
pub(crate) fn finish_presentation<T, E>(outcome: Result<T, E>, flushed: Result<(), E>) -> Result<T, E> {
    let value = outcome?;
    flushed?;
    Ok(value)
}

/// Pending draws from the latest framebuffer, without adding pixels to their combined footprint.
///
/// Containment is preserved by the image extraction policy: widening depends only on increasing
/// width and height. Arbitrary bounding-box merges could instead widen a previously compact draw
/// and make untouched pixels opaque on a newly created or resized canvas.
#[derive(Default)]
pub(crate) struct PendingRegions {
    regions: Vec<InclusiveRectangle>,
}

impl PendingRegions {
    pub(crate) const CAPACITY: usize = 64;

    /// Adds a region, removing draws already covered by another pending region.
    ///
    /// Returns the unqueued region when the caller must flush first. On overflow, the queue stays
    /// unchanged; no region is discarded or widened to meet the storage limit.
    pub(crate) fn push(&mut self, region: InclusiveRectangle) -> Option<InclusiveRectangle> {
        if self.regions.iter().any(|pending| {
            pending.left <= region.left
                && pending.top <= region.top
                && region.right <= pending.right
                && region.bottom <= pending.bottom
        }) {
            return None;
        }

        self.regions.retain(|pending| {
            !(region.left <= pending.left
                && region.top <= pending.top
                && pending.right <= region.right
                && pending.bottom <= region.bottom)
        });

        if self.regions.len() == Self::CAPACITY {
            return Some(region);
        }

        self.regions.push(region);
        None
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    /// Flushes a full queue, then queues the new region even if drawing failed.
    ///
    /// The callback must drain the queue. Retaining the overflow region gives the final
    /// best-effort flush a chance to present it before the original error is returned.
    pub(crate) fn push_or_flush(
        &mut self,
        region: InclusiveRectangle,
        flush: &mut dyn FnMut(&mut Self) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        let Some(region) = self.push(region) else {
            return Ok(());
        };
        let result = flush(self);
        let overflow = self.push(region);
        assert!(overflow.is_none(), "flush must drain pending regions");
        result
    }

    /// Attempts every pending draw and returns the first error after draining the batch.
    /// The scratch buffer is cleared after successful and failed attempts alike.
    pub(crate) fn flush(
        &mut self,
        buffer: &mut WriteBuf,
        draw: &mut dyn FnMut(InclusiveRectangle, &mut WriteBuf) -> anyhow::Result<()>,
    ) -> (PresentationStats, anyhow::Result<()>) {
        let mut stats = PresentationStats::default();
        let mut first_error = None;
        buffer.clear();
        for region in self.drain() {
            stats.attempted_regions = stats.attempted_regions.saturating_add(1);
            match draw(region, buffer) {
                Ok(()) => {
                    stats.uploaded_bytes = stats
                        .uploaded_bytes
                        .saturating_add(u64::try_from(buffer.filled_len()).unwrap_or(u64::MAX));
                }
                Err(error) => {
                    stats.failed_regions = stats.failed_regions.saturating_add(1);
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                }
            }
            buffer.clear();
        }
        (stats, first_error.map_or(Ok(()), Err))
    }

    /// Removes the pending regions while retaining storage for subsequent updates.
    pub(crate) fn drain(&mut self) -> impl Iterator<Item = InclusiveRectangle> + '_ {
        self.regions.drain(..)
    }
}
