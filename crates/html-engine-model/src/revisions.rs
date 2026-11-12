//! Document product revisions, separate from any host's page or overlay caches.

/// An engine-issued snapshot. Compare snapshots for any paint change, or use
/// the product queries to decide which host caches need rebuilding.
/// Snapshots must come from the same engine instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DocumentRevisions {
    layout: u64,
    paint: u64,
    resources: u64,
}

impl DocumentRevisions {
    pub fn new() -> Self {
        Self {
            layout: 1,
            paint: 1,
            resources: 1,
        }
    }

    /// Whether document geometry changed since the earlier snapshot.
    pub fn layout_changed_since(self, previous: Self) -> bool {
        self.layout != previous.layout
    }

    /// Whether decoded resources changed since the earlier snapshot.
    pub fn resources_changed_since(self, previous: Self) -> bool {
        self.resources != previous.resources
    }

    fn next(value: &mut u64) {
        *value = value.wrapping_add(1).max(1);
    }

    /// Advance geometry and paint revisions after an engine layout update.
    pub fn invalidate_layout(&mut self) {
        Self::next(&mut self.layout);
        self.invalidate_paint();
    }

    pub fn invalidate_paint(&mut self) {
        Self::next(&mut self.paint);
    }

    pub fn invalidate_resources(&mut self) {
        Self::next(&mut self.resources);
        Self::next(&mut self.paint);
    }
}

impl Default for DocumentRevisions {
    fn default() -> Self {
        Self::new()
    }
}
