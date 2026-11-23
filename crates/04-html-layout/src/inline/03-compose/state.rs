//! Retained derived state for paragraph composition.
use super::hyphenation::DeferredHyphenationCache;
use super::wrapping::KpPlan;
use crate::inline::tokens::InlineTokens;

/// Derived line-breaking data retained independently of prepared token storage.
#[derive(Default)]
pub(in crate::inline) struct CompositionState {
    pub(in crate::inline) kp_plan: Option<KpPlan>,
    pub(in crate::inline) deferred_hyphens: Option<Box<DeferredHyphenationCache>>,
}

impl CompositionState {
    pub(in crate::inline) fn prepare_kp_plan(
        &mut self,
        tokens: &InlineTokens<'_>,
        punctuation_aware: bool,
    ) {
        if self.kp_plan.is_none() {
            self.kp_plan = Some(KpPlan::build(
                tokens.span(),
                &tokens.runs,
                punctuation_aware,
            ));
        }
    }
}
