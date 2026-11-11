//! Per-document resolver state. Helpers depend on this record, never on orchestration.
use super::context::ResolutionContext;
use super::local_matching::{LocalMatchCache, PseudoCandidates};
use super::plan::CachedRuleMatches;
use super::specified;
use super::style_sharing::StyleSharingCache;
use crate::style::compiled::{PreparedRuleSet, SelectorCandidate, SelectorIndex};
use crate::style::matching::BoundSelectors;
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
    pub(super) selectors: &'a BoundSelectors<'a>,
    pub(super) styles: &'a mut ComputedStylesBuilder,
    pub(super) reader_overrides: &'a ReaderStyleOverrides,
    pub(super) candidate_scratch: Vec<SelectorCandidate>,
    pub(super) pseudo_candidate_scratch: PseudoCandidates,
    pub(super) local_specificity_scratch: Vec<Option<u32>>,
    pub(super) local_match_cache: LocalMatchCache,
    pub(super) matched_rules: CachedRuleMatches,
    pub(super) cascade_scratch: specified::CascadeScratch<'css>,
    pub(super) style_sharing_cache: StyleSharingCache,
    pub(super) resolution: &'a mut ResolutionContext,
    pub(super) timings: &'a mut ResolveStyleTimings,
}

/// Browser timing crosses the host boundary; keep only whole-pass timings there.
pub(super) struct PhaseTimer(Option<web_time::Instant>);

impl PhaseTimer {
    pub(super) fn start() -> Self {
        Self(if cfg!(target_arch = "wasm32") {
            None
        } else {
            Some(web_time::Instant::now())
        })
    }
    pub(super) fn elapsed(&self) -> Duration {
        self.0.map_or(Duration::ZERO, |started| started.elapsed())
    }
}
