//! Per-document resolver state. Helpers depend on this record, never on orchestration.
use super::context::ResolutionContext;
use super::local_matching::LocalMatchCache;
use super::plan::MatchedRule;
use super::specified;
use super::state::WorkingStyle;
use super::style_sharing::StyleSharingCache;
use crate::style::compiled::{EffectiveRuleId, PreparedRuleSet, SelectorIndex};
use crate::style::matching::selectors::CandidateDeduper;
use html_dom::{Document, DomNodeId};
use html_style_model::{ComputedStylesBuilder, ReaderStyleOverrides};
use std::time::Duration;

/// Resolve styles for all DOM elements
/// This works with the DOM tree before boxes are created
#[derive(Clone, Debug, Default)]
pub(crate) struct ResolveStyleTimings {
    pub resolver_setup: Duration,
    pub selector_matching: Duration,
    pub cascade: Duration,
    pub style_store: Duration,
}

pub(super) struct StyleResolverContext<'a, 'css> {
    pub(super) doc: &'a Document,
    pub(super) prepared: &'a PreparedRuleSet<'css>,
    pub(super) author_roots: &'a [Option<DomNodeId>],
    pub(super) index: &'a SelectorIndex,
    pub(super) styles: &'a mut ComputedStylesBuilder,
    pub(super) reader_overrides: &'a ReaderStyleOverrides,
    pub(super) candidate_scratch: &'a mut Vec<EffectiveRuleId>,
    pub(super) candidate_seen: &'a mut CandidateDeduper,
    pub(super) local_specificity_scratch: &'a mut Vec<Option<u32>>,
    pub(super) local_match_cache: &'a mut LocalMatchCache,
    pub(super) matched_rule_scratch: &'a mut Vec<MatchedRule>,
    pub(super) important_rule_scratch: &'a mut Vec<MatchedRule>,
    pub(super) cascade_scratch: &'a mut specified::CascadeScratch<'a, 'css>,
    pub(super) style_sharing_cache: &'a mut StyleSharingCache,
    pub(super) declaration_style: &'a mut WorkingStyle,
    pub(super) resolution: &'a ResolutionContext,
    pub(super) timings: &'a mut ResolveStyleTimings,
}
