//! Normalize boundary and whitespace facts, then summarize the token columns.
use crate::inline::boundaries::{NORMAL_AFTER, NORMAL_BEFORE};
use crate::inline::justification::classify_space;
use crate::inline::{
    BreakKind, INLINE_SUMMARY_BLOCK_TOKENS, InlineSummaryBlock, InlineToken, InlineTokenKind,
    InlineTokenMetrics, InlineTokens, LayoutReadContext, MetricsView, PreparedTokenFeatures, Range,
    ReplacedToken, TokenSpan, WhiteSpace, token_is_collapsible_line_edge_space,
};
use html_engine_model::{InlineItemKind, WhitespaceWrapOverride};

impl InlineTokens<'_> {
    pub(in crate::inline) fn push_replaced(
        &mut self,
        payload: ReplacedToken,
        kind: impl FnOnce(u32) -> InlineTokenKind,
        mut token: InlineToken,
        metrics: InlineTokenMetrics,
        allow_wrap: bool,
    ) {
        if allow_wrap {
            token.mark_break_before();
            token.mark_break_after();
        }
        let payload_idx = u32::try_from(self.replaced.len())
            .expect("inline replaced-token arena capacity exhausted");
        self.replaced.push(payload);
        token.set_kind(kind(payload_idx));
        self.push(token, metrics);
    }

    /// Resolve object boundary flags against their actual logical
    /// neighbours, including styles outside transparent inline wrappers.
    pub(in crate::inline) fn resolve_atomic_break_opportunities(
        &mut self,
        inputs: &LayoutReadContext<'_>,
    ) {
        let atomic = |token: InlineToken| {
            matches!(
                token.kind(),
                InlineTokenKind::Image { .. } | InlineTokenKind::AtomicBox { .. }
            )
        };
        let neighbour = |token: InlineToken| {
            let character = match token.kind() {
                InlineTokenKind::Glyph { glyph_idx } => inputs.text.character(glyph_idx),
                InlineTokenKind::Image { .. } | InlineTokenKind::AtomicBox { .. } => '\u{fffc}',
                InlineTokenKind::Break { .. } => '\n',
                _ => return None,
            };
            Some((
                token.run_metrics(&self.runs).owner_box_idx as usize,
                character,
            ))
        };
        for index in 0..self.data.len() {
            if !atomic(self.data.span().token_at(index)) {
                continue;
            }
            let current = neighbour(self.data.span().token_at(index)).expect("atomic source item");
            let left = self
                .data
                .span()
                .slice(..index)
                .iter()
                .rev()
                .find_map(neighbour);
            let right = self
                .data
                .span()
                .slice(index + 1..)
                .iter()
                .find_map(neighbour);
            let allows = |left: (usize, char), right: (usize, char)| {
                left.1 != '\n'
                    && right.1 != '\n'
                    && html_text::atomic_boundary_wraps(
                        inputs.reader.styles(),
                        inputs.reader.layout_tree(),
                        left,
                        right,
                    )
            };
            let mut flags = self.data.span().token_at(index).boundary_flags();
            if !left.is_some_and(|left| allows(left, current)) {
                flags &= !NORMAL_BEFORE;
            }
            if !right.is_some_and(|right| allows(current, right)) {
                flags &= !NORMAL_AFTER;
            }
            self.data
                .update_header(index, |token| token.set_boundary_flags(flags));
        }
    }

    /// Find edge whitespace during the forward summary pass. Transparent
    /// inline boundaries retain their geometry while adjacent collapsible
    /// spaces lose their advance. Only affected summary blocks are rebuilt.
    pub(in crate::inline) fn rebuild_summary_blocks(&mut self) {
        let transparent_boundary = |token: &InlineToken| {
            matches!(
                token.kind(),
                InlineTokenKind::InlineBoundary { .. }
                    | InlineTokenKind::FloatAnchor { .. }
                    | InlineTokenKind::AbsoluteAnchor { .. }
            )
        };
        let mut leading = true;
        let mut leading_end = 0;
        let mut trailing_start = 0;
        let mut token_index = 0;
        let mut features = PreparedTokenFeatures {
            flags: self.features.flags
                & (PreparedTokenFeatures::NO_BIDI_GLYPH_SEPARATORS
                    | PreparedTokenFeatures::DEFERRED_HYPHENATION),
            ..PreparedTokenFeatures::default()
        };
        let runs = &self.runs;
        let mut metrics = MetricsView::direct(runs).cursor(self.data.len());
        let summaries = self.summary_blocks.make_mut();
        summaries.clear();
        summaries.extend(
            (0..self.data.len())
                .step_by(INLINE_SUMMARY_BLOCK_TOKENS)
                .map(|start| {
                    let tokens = self
                        .data
                        .span()
                        .slice(start..(start + INLINE_SUMMARY_BLOCK_TOKENS).min(self.data.len()));
                    InlineSummaryBlock::build(tokens, runs, |token| {
                        features.has_tabs |= token.is_tab();
                        features.flags |=
                            u8::from(matches!(token.kind(), InlineTokenKind::Break { .. }))
                                * PreparedTokenFeatures::HAS_BREAKS;
                        features.has_float_anchors |=
                            matches!(token.kind(), InlineTokenKind::FloatAnchor { .. });
                        features.preserves_spaces |=
                            token.run_metrics(runs).white_space.preserves_spaces();
                        if token_is_collapsible_line_edge_space(token.metadata(), &mut metrics) {
                            if leading {
                                leading_end = token_index + 1;
                            }
                        } else if !transparent_boundary(token) {
                            leading = false;
                            trailing_start = token_index + 1;
                        }
                        token_index += 1;
                    })
                }),
        );
        self.features = features;
        let ranges = [
            0..leading_end,
            trailing_start.max(leading_end)..self.data.len(),
        ];
        if ranges.iter().all(Range::is_empty) {
            return;
        }

        for range in ranges {
            let mut changed = false;
            for index in range.clone() {
                if self.data.span().metadata_at(index).break_kind() == BreakKind::Soft
                    && self.data.span().stored_width_at(index) != 0.0
                {
                    self.data.widths.make_mut()[index] = 0.0;
                    changed = true;
                }
            }
            if changed {
                for index in range.start / INLINE_SUMMARY_BLOCK_TOKENS
                    ..range.end.div_ceil(INLINE_SUMMARY_BLOCK_TOKENS)
                {
                    let start = index * INLINE_SUMMARY_BLOCK_TOKENS;
                    let end = (start + INLINE_SUMMARY_BLOCK_TOKENS).min(self.data.len());
                    summaries[index] =
                        InlineSummaryBlock::build(self.data.span().slice(start..end), runs, |_| {});
                }
            }
        }
    }

    /// Stores the local punctuation context on each ordinary space. The token
    /// owns only this compact semantic classification; both Knuth--Plass and
    /// final placement resolve it through the shared glue policy.
    pub(in crate::inline) fn classify_space_glue(&mut self, inputs: &LayoutReadContext<'_>) {
        fn glyph_character(inputs: &LayoutReadContext<'_>, token: &InlineToken) -> Option<char> {
            let InlineTokenKind::Glyph { glyph_idx } = token.kind() else {
                return None;
            };
            Some(inputs.text.character(glyph_idx))
        }

        fn context_character(
            inputs: &LayoutReadContext<'_>,
            tokens: TokenSpan<'_>,
            mut index: usize,
            direction: isize,
        ) -> Option<char> {
            loop {
                index = index.checked_add_signed(direction)?;
                if index >= tokens.len() {
                    return None;
                }
                let token = tokens.token_at(index);
                if let Some(character) = glyph_character(inputs, &token) {
                    return Some(character);
                }
                match token.kind() {
                    InlineTokenKind::InlineBoundary { .. }
                    | InlineTokenKind::FloatAnchor { .. }
                    | InlineTokenKind::AbsoluteAnchor { .. }
                    | InlineTokenKind::ZeroWidthBreak => {}
                    _ => return None,
                }
            }
        }

        for index in 0..self.data.len() {
            if !self.data.span().token_at(index).is_space() {
                continue;
            }
            // Glue classes preserve token kinds and glyph identities, so
            // later spaces still observe the same neighboring characters.
            let previous = context_character(inputs, self.data.span(), index, -1);
            let next = context_character(inputs, self.data.span(), index, 1);
            self.data.update_header(index, |token| {
                token.set_space_glue_class(classify_space(previous, next))
            });
        }
    }

    /// Attach Unicode opportunities to boundaries without inserting content.
    pub(in crate::inline) fn prepare_unicode_boundaries(&mut self, inputs: &LayoutReadContext<'_>) {
        for index in 0..self.data.len() {
            let InlineTokenKind::Glyph { glyph_idx } = self.data.span().token_at(index).kind()
            else {
                continue;
            };
            if inputs.text.whitespace_wrap_before(glyph_idx as usize)
                != WhitespaceWrapOverride::UnicodeAllow
            {
                continue;
            }
            self.data
                .update_header(index, |token| token.set_break_kind(BreakKind::None));
            if !self.data.span().token_at(index).is_cluster_boundary() {
                continue;
            }
            let mut position = index;
            // Opening decoration belongs to the following text.
            while let Some(previous) = position.checked_sub(1) {
                let opening = match self.data.span().metadata_at(previous).kind() {
                    InlineTokenKind::InlineBoundary { payload_idx } => matches!(
                        self.replaced[payload_idx as usize],
                        ReplacedToken::InlineBoundary {
                            inline_start: true,
                            inline_end: false,
                            ..
                        }
                    ),
                    InlineTokenKind::SourceItem { item_idx } => matches!(
                        inputs.text.inline_items()[item_idx as usize].kind,
                        InlineItemKind::InlineBoundary {
                            inline_start: true,
                            inline_end: false
                        }
                    ),
                    _ => false,
                };
                if !opening {
                    break;
                }
                position = previous;
            }
            self.data
                .update_header(position, InlineToken::mark_break_before);
        }
    }

    /// `pre-wrap` breaks after a complete sequence of preserved spaces, while
    /// `break-spaces` breaks after every preserved space. A normal soft token
    /// represents a break before discardable whitespace, so use explicit
    /// boundary flags for these after-space rules.
    pub(in crate::inline) fn prepare_preserved_space_boundaries(
        &mut self,
        inputs: &LayoutReadContext<'_>,
    ) {
        for index in 0..self.data.len() {
            let token = self.data.span().token_at(index);
            let InlineTokenKind::Glyph { glyph_idx } = token.kind() else {
                continue;
            };
            let character = inputs.text.character(glyph_idx);
            let white_space = token.run_metrics(&self.runs).white_space;
            if !matches!(character, ' ' | '\t')
                && !(white_space == WhiteSpace::BreakSpaces && character == '\u{3000}')
            {
                continue;
            }
            let break_after = match white_space {
                WhiteSpace::BreakSpaces => true,
                WhiteSpace::PreWrap => self
                    .data
                    .span()
                    .slice(index + 1..)
                    .iter()
                    .find(|next| {
                        if let InlineTokenKind::SourceItem { item_idx } = next.kind() {
                            return !matches!(
                                inputs.text.inline_items()[item_idx as usize].kind,
                                InlineItemKind::InlineBoundary { .. }
                                    | InlineItemKind::FloatAnchor { .. }
                                    | InlineItemKind::AbsoluteAnchor { .. }
                            );
                        }
                        !matches!(
                            next.kind(),
                            InlineTokenKind::InlineBoundary { .. }
                                | InlineTokenKind::FloatAnchor { .. }
                                | InlineTokenKind::AbsoluteAnchor { .. }
                        )
                    })
                    .is_none_or(|next| {
                        let InlineTokenKind::Glyph { glyph_idx } = next.kind() else {
                            return true;
                        };
                        let next_character = inputs.text.character(glyph_idx);
                        !matches!(next_character, ' ' | '\t')
                            || !next.run_metrics(&self.runs).white_space.preserves_spaces()
                    }),
                _ => false,
            };
            if break_after {
                self.data
                    .update_header(index, InlineToken::mark_break_after);
            }
        }
    }
}
