use ironrdp::pdu::geometry::InclusiveRectangle;

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

    /// Removes the pending regions while retaining storage for subsequent updates.
    pub(crate) fn drain(&mut self) -> impl Iterator<Item = InclusiveRectangle> + '_ {
        self.regions.drain(..)
    }
}
