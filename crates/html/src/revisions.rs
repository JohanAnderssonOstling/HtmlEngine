//! Document product revisions, independent of pipeline implementation.
use crate::pipeline::EarliestStage;
pub use html_engine_model::DocumentRevisions;

pub(crate) fn invalidate_from(revisions: &mut DocumentRevisions, stage: EarliestStage) {
    match stage {
        EarliestStage::Parse
        | EarliestStage::Style
        | EarliestStage::Prepare
        | EarliestStage::Shape
        | EarliestStage::Layout => revisions.invalidate_layout(),
        EarliestStage::Paint => revisions.invalidate_paint(),
        EarliestStage::None => {}
    }
}
