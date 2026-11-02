//! Document product revisions, separate from any host's page or overlay caches.
use crate::pipeline::EarliestStage;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DocumentRevisions {
    pub document: u64,
    pub style: u64,
    pub shape: u64,
    pub layout: u64,
    pub paint: u64,
    pub resources: u64,
}

impl Default for DocumentRevisions {
    fn default() -> Self {
        Self {
            document: 1,
            style: 1,
            shape: 1,
            layout: 1,
            paint: 1,
            resources: 1,
        }
    }
}

impl DocumentRevisions {
    fn next(value: &mut u64) {
        *value = value.wrapping_add(1).max(1);
    }

    pub fn invalidate_from(&mut self, stage: EarliestStage) {
        match stage {
            EarliestStage::Parse => {
                Self::next(&mut self.document);
                Self::next(&mut self.style);
                Self::next(&mut self.shape);
                Self::next(&mut self.layout);
            }
            EarliestStage::Style | EarliestStage::Prepare => {
                Self::next(&mut self.style);
                Self::next(&mut self.shape);
                Self::next(&mut self.layout);
            }
            EarliestStage::Shape => {
                Self::next(&mut self.shape);
                Self::next(&mut self.layout);
            }
            EarliestStage::Layout => Self::next(&mut self.layout),
            EarliestStage::Paint => {}
            EarliestStage::None => return,
        }
        Self::next(&mut self.paint);
    }

    pub fn invalidate_resources(&mut self) {
        Self::next(&mut self.resources);
        Self::next(&mut self.paint);
    }
}
