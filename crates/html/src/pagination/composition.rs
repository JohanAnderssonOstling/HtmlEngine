//! Page composition from published layout, column geometry and preferences.

use super::{PageAnchor, PageGeometry, PagePositions, PageRequest};
use crate::layout::RenderView;
use crate::pagination::PaginationTarget;
use crate::reading::{ReadingFragment, ReadingOrigin};
use html_engine_model::layout_state;
use html_engine_model::{LineScreen, VisibleLinePositions, page_placements};
use html_pipeline::engine::{Pagination, PaginationGroup};
use html_engine_model::layout_state::SourceIntervals;
use std::sync::Arc;
use std::collections::{HashMap, HashSet};
use std::ops::Range;

#[path = "semantic_keeps.rs"]
mod semantic_keeps;
#[path = "vertical_rhythm.rs"]
mod vertical_rhythm;


use crate::layout::{BoxId, ContentRole, LineId, RenderForcedBreak};
use kurbo::Point;

/// Merge overlapping protected content spans once per oversized group. Carrier
/// lines can cover whole galleries, so use actual text and image geometry.
fn protected_spans(doc: RenderView<'_>, group: &PaginationGroup<'_>) -> Vec<Range<f64>> {
    let mut spans = Vec::new();
    for member in group.lines() {
        if doc.text().line_text_fragments(member.id())
            .is_some_and(|mut fragments| fragments.any(|fragment| fragment.visible() && !fragment.glyphs().is_empty()))
        {
            if let Some(line) = doc.text().line(member.id()) {
                spans.push(line.point().y..line.point().y + line.height());
            }
        }
        for image in doc.fragments().images_for_line(member.id()).iter() {
            let rect = image.visible_rect();
            spans.push(member.point().y + rect.y0..member.point().y + rect.y1);
        }
    }
    spans.retain(|span| span.start.is_finite() && span.end.is_finite() && span.end > span.start);
    spans.sort_by(|a, b| a.start.total_cmp(&b.start));
    let mut merged: Vec<Range<f64>> = Vec::with_capacity(spans.len());
    for span in spans {
        if let Some(previous) = merged.last_mut()
            && span.start < previous.end - 0.01
        {
            previous.end = previous.end.max(span.end);
        } else {
            merged.push(span);
        }
    }
    merged
}

/// Prefer a content boundary; retain geometric slicing when an overlapping
/// content span itself cannot fit in a column, guaranteeing forward progress.
fn content_break(spans: &[Range<f64>], edge: f64, limit: f64, forward: bool) -> f64 {
    let index = spans.partition_point(|span| span.end <= limit + 0.01);
    if let Some(span) = spans.get(index)
        && span.start < limit - 0.01
    {
        let boundary = if forward { span.start } else { span.end };
        if (forward && boundary > edge + 0.01) || (!forward && boundary < edge - 0.01) {
            return boundary;
        }
    }
    limit
}

/// A single line or replaced element is monolithic, even when taller than a
/// column. Only containers with separate content spans have continuation cuts.
fn can_fragment(doc: RenderView<'_>, group: &PaginationGroup<'_>, height: f64) -> bool {
    if group.height() <= height { return false; }
    let mut first = None;
    for (cursor, member) in group.cursors().zip(group.lines()) {
        let Some(line) = doc.text().line(member.id()) else { continue };
        if line.glyphs().is_empty()
            || doc.pagination().source_line(cursor).is_some_and(|source| source.id() != member.id())
        { continue; }
        if first.is_some_and(|y: f64| (y - line.point().y).abs() > 0.01) {
            return true;
        }
        first = Some(line.point().y);
    }
    protected_spans(doc, group).len() > 1
}

/// Source-only data shared by all fragments of an oversized group.
struct GroupSlices {
    spans: Vec<Range<f64>>,
    members: SourceIntervals,
}

impl GroupSlices {
    fn new(doc: RenderView<'_>, group: &PaginationGroup<'_>) -> Self {
        let ownership = doc.boxes().pagination_ownership();
        let mut table_carriers = HashSet::new();
        let mut table_cursor = 0;
        let mut flow_cursor = 0;
        let mut box_carriers = HashSet::new();
        let mut entries = Vec::new();
        for (cursor, member) in group.cursors().zip(group.lines()) {
            let line = doc.text().line(member.id());
            let top = line.as_ref().map_or(member.point().y, |line| line.point().y);
            let bottom = line.as_ref().map_or(top, |line| top + line.height());
            let mut bounds = top..bottom;
            for image in doc.fragments().images_for_line(member.id()).iter() {
                let rect = image.visible_rect();
                bounds.start = bounds.start.min(member.point().y + rect.y0);
                bounds.end = bounds.end.max(member.point().y + rect.y1);
            }
            entries.push((cursor as u32, bounds.start, bounds.end));
            // Padding and otherwise empty portions of a row still need its
            // structural mapping. Retain one carrier per table fragment,
            // rather than every offscreen line in that row.
            if let Some(membership) = ownership.lines.get(layout_state::line_id_index(member.id())) {
                let flows = ownership.flow_line_index.sources_for_line(
                    layout_state::line_id_index(member.id()) as u32, &mut flow_cursor,
                );
                for &owner in membership.content.iter().chain(flows) {
                    if box_carriers.insert(owner)
                        && let Some(bounds) = doc.boxes().bounds(layout_state::box_id_new(owner as usize))
                    {
                        entries.push((cursor as u32, bounds.y0, bounds.y1));
                    }
                }
                let table_sources = ownership.table_line_index.sources_for_line(
                    layout_state::line_id_index(member.id()) as u32, &mut table_cursor,
                );
                for &source in table_sources {
                    if table_carriers.insert(source) {
                        let bounds = ownership.table_fragments[source as usize].bounds;
                        entries.push((cursor as u32, bounds.y0, bounds.y1));
                    }
                }
            }
        }
        Self { spans: protected_spans(doc, group), members: SourceIntervals::new(entries) }
    }
}

#[derive(Default)]
struct SliceCache {
    groups: HashMap<usize, Arc<GroupSlices>>,
}

impl SliceCache {
    fn group(&mut self, doc: RenderView<'_>, group: &PaginationGroup<'_>) -> Arc<GroupSlices> {
        Arc::clone(self.groups.entry(group.cursors().start)
            .or_insert_with(|| Arc::new(GroupSlices::new(doc, group))))
    }
}

struct RenderState {
    y_offset: f64,
    col_index: f64,
    should_stop: bool,
}

fn resolve_point_at(
    geometry: &PageGeometry,
    point: Point,
    elem_height: f64,
    mut render_state: RenderState,
    start_offset_y: f64,
) -> (Option<Point>, RenderState) {
    // map document point to screen columns
    let mut y = point.y + render_state.y_offset - start_offset_y;
    let mut col_index = (y / geometry.size.height).floor(); // initial column based on vertical position
    y -= col_index * geometry.size.height;

    if y + elem_height > geometry.size.height {
        // move to next column if element would overflow
        col_index += 1.0;
        render_state.y_offset += geometry.size.height - y;
        y = 0.;
    }

    // Past all columns - stop rendering
    if col_index >= geometry.col_count {
        // stop when past last column
        render_state.should_stop = true;
        return (None, render_state);
    }

    render_state.col_index = col_index;
    let x = page_placements::col_x(geometry, col_index, point.x);
    let resolved = Point::new(x, y);
    (Some(resolved), render_state)
}

fn initial_render_state() -> RenderState {
    RenderState {
        y_offset: 0.0,
        col_index: 0.0,
        should_stop: false,
    }
}

use crate::structure::{BlockFacts, SiblingPair, Structure, StructureView};
use crate::structure::{
    TableGroup as TablePaginationEvent, TableGroupKind as TablePaginationEventKind,
};

#[derive(Clone, Copy, Debug)]
struct ForcedBreakEvent {
    trigger_line: usize,
    top: f64,
    kind: RenderForcedBreak,
}

#[derive(Clone, Copy, Debug)]
struct KeepRangeEvent {
    trigger_line: usize,
    top: f64,
    bottom: f64,
}

/// Shared bounded inputs for CSS and reader keep preferences.
#[derive(Default)]
struct KeepContext {
    paragraph_lines: HashMap<BoxId, Vec<(usize, f64, f64)>>,
    lines_by_y: Vec<(f64, usize)>,
    blocks: Vec<BlockFacts>,
    siblings: Vec<SiblingPair>,
}

impl KeepContext {
    fn prepare(&mut self, anchors: &StructureView<'_>, doc: RenderView<'_>) {
        let lines = anchors.pagination();
        self.paragraph_lines.clear();
        // Preserve individual source lines for widows/orphans. Spatial overlap
        // and pagination-group extents can include unrelated floating content.
        for cursor in anchors.lines() {
            let Some(position) = lines.locate(PaginationTarget::Cursor(cursor)) else { continue };
            let Some(line) = doc.text().line(position.line.id()) else { continue };
            if line.glyphs().is_empty() {
                continue;
            }
            self.paragraph_lines.entry(line.owner_box()).or_default().push((
                cursor, line.point().y, line.point().y + line.height(),
            ));
        }
        for entries in self.paragraph_lines.values_mut() {
            entries.sort_unstable_by_key(|&(cursor, _, _)| cursor);
            entries.dedup_by_key(|entry| entry.0);
        }
        self.lines_by_y.clear();
        self.lines_by_y.extend(
            anchors
                .lines()
                .filter_map(|index| lines.locate(PaginationTarget::Cursor(index)))
                .filter_map(|position| {
                    let cursor = position.group.cursors().start;
                    position
                        .group
                        .lines()
                        .next()
                        .map(|line| (line.point().y, cursor))
                }),
        );
        self.lines_by_y.sort_by(|left, right| {
            left.0
                .total_cmp(&right.0)
                .then_with(|| left.1.cmp(&right.1))
        });
        self.lines_by_y.dedup_by_key(|(_, cursor)| *cursor);
        self.blocks.clear();
        self.blocks.extend(anchors.blocks());
        self.siblings = anchors.sibling_pairs();
    }

    fn lines_in_range(&self, top: f64, bottom: f64, indices: &mut Vec<usize>) {
        indices.clear();
        let start = self.lines_by_y.partition_point(|(y, _)| *y < top - 0.01);
        let end = self.lines_by_y.partition_point(|(y, _)| *y < bottom - 0.01);
        indices.extend(self.lines_by_y[start..end].iter().map(|(_, line)| *line));
        indices.sort_unstable();
    }
}

fn apply_keep_range_event(
    layout: &PageGeometry,
    event: KeepRangeEvent,
    start_offset_y: f64,
    render_state: &mut RenderState,
) {
    let fragment_height = layout.size.height;
    if fragment_height <= 0.0 || event.top < start_offset_y - 0.01 {
        return;
    }
    let height = (event.bottom - event.top).max(0.0);
    if height > fragment_height + 0.01 {
        return;
    }
    let projected_top = event.top + render_state.y_offset - start_offset_y;
    let local_top = projected_top.rem_euclid(fragment_height);
    if local_top > 0.01 && local_top + height > fragment_height + 0.01 {
        render_state.y_offset += fragment_height - local_top;
    }
}

fn apply_forced_break_event(
    layout: &PageGeometry,
    event: ForcedBreakEvent,
    start_offset_y: f64,
    previous_line: Option<LineScreen>,
    render_state: &mut RenderState,
) {
    let fragment_height = layout.size.height;
    if fragment_height <= 0.0 || event.top < start_offset_y - 0.01 {
        return;
    }
    // At the beginning of a newly selected reader page the break has
    // already been satisfied. Otherwise base the destination solely on
    // the last column that contains actual content, never on margin-only
    // document space.
    let target_col = previous_line.map_or(0.0, |previous| match event.kind {
        RenderForcedBreak::Column => previous.col_index as f64 + 1.0,
        RenderForcedBreak::Page => layout.col_count,
    });
    let projected_top = event.top + render_state.y_offset - start_offset_y;
    render_state.y_offset += target_col * fragment_height - projected_top;
}

fn apply_table_pagination_event(
    layout: &PageGeometry,
    event: TablePaginationEvent,
    start_offset_y: f64,
    render_state: &mut RenderState,
) {
    let fragment_height = layout.size.height;
    if fragment_height <= 0.0 || event.bottom <= start_offset_y + 0.01 {
        return;
    }
    let pagination_top = event.top.max(start_offset_y);
    let projected_top = pagination_top + render_state.y_offset - start_offset_y;
    let local_top = projected_top.rem_euclid(fragment_height);
    let height = (event.bottom - event.top).max(0.0);
    let crosses = local_top + height > fragment_height + 0.01;
    let move_to_fresh_fragment = match event.kind {
        // Move a whole table only when it can fit in a fresh fragment.
        TablePaginationEventKind::WholeTable => height <= fragment_height && crosses,
        // Splittable rows continue through their content boundaries. A forced
        // break already at a fragment boundary must not skip another column.
        TablePaginationEventKind::RowGroup { forced, allow_content_break } => {
            (forced && local_top > 0.01)
                || (!allow_content_break && height <= fragment_height && crosses)
        }
    };
    if move_to_fresh_fragment {
        render_state.y_offset += fragment_height - local_top;
    }
}

/// Semantic break information used while cutting one requested page.
#[derive(Default)]
struct PaginationEvents {
    tables: Vec<TablePaginationEvent>,
    forced_breaks: Vec<ForcedBreakEvent>,
    keeps: Vec<KeepRangeEvent>,
}

#[derive(Default)]
struct VerticalRhythmScratch {
    visible: Vec<(LineId, LineScreen)>,
    column_lines: Vec<(LineId, LineScreen)>,
    candidates: Vec<(LineId, f64)>,
    rigid_ranges: Vec<(f64, f64)>,
    original_breaks: Vec<(f64, f64)>,
}

/// Reusable pagination storage. Cached source data and cuts are invalidated
/// automatically when the layout publication, geometry, or mode changes.
#[derive(Default)]
pub struct PaginationScratch {
    cache_key: Option<(u64, [u64; 5], bool, bool)>,
    slices: SliceCache,
    continuation_cuts: HashMap<usize, Vec<f64>>,
    structure: Structure,
    events: PaginationEvents,
    probe_positions: Vec<LineScreen>,
    reverse_positions: Vec<(usize, LineScreen)>,
    reverse_keeps: Vec<KeepRangeEvent>,
    reverse_tables: Vec<TablePaginationEvent>,
    semantic_ranges: Vec<(usize, f64, f64)>,
    keep_context: KeepContext,
    range_lines: Vec<usize>,
    authored_boundaries: HashSet<usize>,
    merged_forced_breaks: Vec<ForcedBreakEvent>,
    vertical_rhythm: VerticalRhythmScratch,
}

/// Paginates a document into reader pages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaginationMode {
    Web,
    Book,
    BookWithVerticalRhythm,
}

#[derive(Clone, Copy)]
pub struct Paginator<'a> {
    doc: RenderView<'a>,
    layout: PageGeometry,
    /// Book composition enables the semantic keep and break rules; web
    /// composition paginates on CSS alone.
    book_optimized: bool,
    vertical_rhythm: bool,
}

impl<'a> Paginator<'a> {
    fn prepare_cache(&self, scratch: &mut PaginationScratch) {
        let key = (self.doc.boxes().pagination_ownership().revision,
            [self.layout.size.width.to_bits(), self.layout.size.height.to_bits(),
             self.layout.col_width.to_bits(), self.layout.col_count.to_bits(), self.layout.col_gap.to_bits()],
            self.book_optimized, self.vertical_rhythm);
        if scratch.cache_key != Some(key) {
            scratch.slices.groups.clear();
            scratch.continuation_cuts.clear();
            scratch.cache_key = Some(key);
        }
    }

    fn pagination(&self) -> Pagination<'a> {
        (self.doc).pagination()
            .for_page_height(self.layout.size.height)
    }

    pub fn new(doc: RenderView<'a>, layout: PageGeometry, mode: PaginationMode) -> Self {
        Self {
            doc,
            layout,
            book_optimized: !matches!(mode, PaginationMode::Web),
            vertical_rhythm: matches!(mode, PaginationMode::BookWithVerticalRhythm),
        }
    }

    /// Resolve the next page start, or report the document end. Before the
    /// first composition, use the current viewport origin as a fallback.
    pub fn next_page_anchor(
        &self,
        page: Option<&PagePositions>,
        start_offset_y: f64,
    ) -> Option<PageAnchor> {
        if page.is_some_and(|page| page.reached_end) {
            return None;
        }
        if let Some((_, offset_y)) = page.and_then(|page| page.continuation) {
            return Some(PageAnchor { cursor: None, offset_y });
        }
        if let Some(end) = page.and_then(|page| page.last_line) {
            let next = end.checked_add(1)?;
            let group = self
                .pagination()
                .locate(PaginationTarget::Cursor(next))?
                .group;
            return Some(PageAnchor {
                cursor: Some(group.cursors().start),
                offset_y: group.point().y,
            });
        }
        Some(PageAnchor {
            cursor: None,
            offset_y: page.map_or(start_offset_y, |page| {
                page.next_start_offset_y.max(start_offset_y)
            }),
        })
    }

    /// Resolve the previous page end, or report the document beginning. The
    /// current offset is used only when no installed cursor or group is available.
    pub fn previous_page_anchor(
        &self,
        page: Option<&PagePositions>,
        start_offset_y: f64,
    ) -> Option<PageAnchor> {
        if let Some(page) = page
            && let Some(position) = self.pagination().locate(PaginationTarget::Cursor(page.start_line))
            && page.paint_start_offset_y > position.group.point().y + 0.01
            && can_fragment(self.doc, &position.group, self.layout.size.height)
        {
            return Some(PageAnchor { cursor: None, offset_y: page.paint_start_offset_y });
        }
        let start = page.map(|page| page.start_line);
        if start.map_or(start_offset_y <= 0.0, |cursor| cursor == 0) {
            return None;
        }
        let cursor = start.and_then(|cursor| cursor.checked_sub(1));
        let offset_y = cursor
            .and_then(|cursor| {
                self.pagination()
                    .locate(PaginationTarget::Cursor(cursor))
                    .map(|position| position.group)
            })
            .map(|group| group.point().y + group.height())
            .unwrap_or(start_offset_y);
        Some(PageAnchor { cursor, offset_y })
    }

    /// Compose only the requested page. The anchor chooses forward or backward
    /// composition. Retain scratch and recycle the previous positions buffer to
    /// preserve allocations across page turns. A bounded probe limits structural
    /// work to the page and its immediate boundary lines.
    pub fn compose(
        &self,
        request: PageRequest,
        positions: Vec<LineScreen>,
        scratch: &mut PaginationScratch,
    ) -> PagePositions {
        self.prepare_cache(scratch);
        if !self.layout.size.height.is_finite() || self.layout.size.height <= 0.0
            || !self.layout.col_count.is_finite() || self.layout.col_count < 1.0
        {
            return PagePositions {
                request,
                geometry: self.layout,
                positions: page_placements::with_buffer(0, positions),
                reached_end: self.pagination().is_empty(),
                ..PagePositions::default()
            };
        }
        match request {
            PageRequest::StartingAtOffset(start_offset_y) => {
                let start_line = self.oversized_at(start_offset_y, false)
                    .map_or_else(|| self.pagination().start_at(start_offset_y, 0.1), |(cursor, _, _)| cursor);
                self.page_from_line_at_with_scratch(start_line, start_offset_y, positions, scratch)
            }
            PageRequest::StartingAt(start_line) => {
                let lines = self.pagination();
                let group = lines.groups(start_line..lines.len()).next();
                let start_line = group
                    .as_ref()
                    .map_or(lines.len(), |group| group.cursors().start);
                let offset = group.map_or(0.0, |group| group.point().y);
                let mut page =
                    self.page_from_line_at_with_scratch(start_line, offset, positions, scratch);
                page.request = PageRequest::StartingAt(start_line);
                page
            }
            PageRequest::EndingAtOffset(offset) => {
                if let Some((cursor, _, _)) = self.oversized_at(offset, true) {
                    let position = self.pagination().locate(PaginationTarget::Cursor(cursor))
                        .expect("located oversized group");
                    let start = self.continuation_start(&position.group, offset, scratch);
                    let mut page = self.page_from_line_until_with_scratch(cursor, start, Some(offset), positions, scratch);
                    page.request = PageRequest::EndingAtOffset(offset);
                    return page;
                }
                let request = self
                    .pagination()
                    .end_at(offset, 1e-6)
                    .map_or(PageRequest::StartingAtOffset(0.0), PageRequest::EndingAt);
                self.compose(request, positions, scratch)
            }
            PageRequest::EndingAt(end_line) => {
                let lines = self.pagination();
                let end_line = end_line.min(lines.len().saturating_sub(1));
                if let Some(position) = lines.locate(PaginationTarget::Cursor(end_line))
                    && can_fragment(self.doc, &position.group, self.layout.size.height)
                {
                    let top = position.group.point().y;
                    let bottom = top + position.group.height();
                    let offset = self.continuation_start(&position.group, bottom, scratch);
                    let mut page = self.page_from_line_until_with_scratch(
                        position.group.cursors().start, offset, Some(bottom), positions, scratch,
                    );
                    page.request = PageRequest::EndingAtOffset(bottom);
                    return page;
                }
                if end_line == 0 {
                    return self.compose(PageRequest::StartingAtOffset(0.0), positions, scratch);
                }
                let end_line = lines
                    .groups(0..end_line.saturating_add(1))
                    .next_back()
                    .and_then(|group| group.cursors().end.checked_sub(1))
                    .unwrap_or(0);
                let probe_buffer = std::mem::take(&mut scratch.probe_positions);
                let probe = self.build_backward_positions(end_line, 0, probe_buffer, None, scratch);
                scratch.probe_positions = probe.positions.into_buffer();
                self.events_for_lines(probe.start_line, end_line, scratch);
                let events = std::mem::take(&mut scratch.events);
                let page = self.build_backward_positions(
                    end_line,
                    probe.start_line,
                    positions,
                    Some(&events),
                    scratch,
                );
                scratch.events = events;
                if page.start_line == 0
                    && lines.locate(PaginationTarget::Cursor(0))
                        .is_none_or(|position| page.paint_start_offset_y <= position.group.point().y + 0.01)
                {
                    self.compose(
                        PageRequest::StartingAtOffset(0.0),
                        page.positions.into_buffer(),
                        scratch,
                    )
                } else {
                    page
                }
            }
        }
    }

    fn continuation_start(
        &self,
        group: &PaginationGroup<'_>,
        bottom: f64,
        scratch: &mut PaginationScratch,
    ) -> f64 {
        // Reuse the actual forward cuts, including nested table/keep rules.
        // An independent backwards estimate can start after an undisplayed
        // interval when the forward compositor moves a nested row whole.
        let cursor = group.cursors().start;
        let cuts = scratch.continuation_cuts.entry(cursor)
            .or_insert_with(|| vec![group.point().y]);
        let index = cuts.partition_point(|top| *top < bottom - 0.01);
        let mut top = cuts[index.saturating_sub(1)];
        if index < cuts.len() { return top; }
        let mut buffer = Vec::new();
        loop {
            // Replay only the previously unknown suffix, without producing paint.
            let page = self.forward_page(cursor, top, None, buffer, scratch, false);
            let next = page.next_start_offset_y;
            if next > top + 0.01 && !page.reached_end {
                let cuts = scratch.continuation_cuts.get_mut(&cursor).expect("group cuts");
                if cuts.last().is_none_or(|last| next > *last + 0.01) { cuts.push(next); }
            }
            if page.reached_end || next >= bottom - 0.01 || next <= top + 0.01 {
                return top;
            }
            top = next;
            buffer = page.positions.into_buffer();
        }
    }

    /// Restored offsets and continuations can address the interior of a group.
    /// Ordinary cursor navigation avoids this geometric lookup entirely.
    fn oversized_at(&self, offset: f64, include_end: bool) -> Option<(usize, f64, f64)> {
        let lines = self.pagination();
        lines.groups(0..lines.len()).find_map(|group| {
            let top = group.point().y;
            let bottom = top + group.height();
            (offset > top + 0.01
                && (if include_end { offset <= bottom + 0.01 } else { offset < bottom - 0.01 })
                && can_fragment(self.doc, &group, self.layout.size.height))
                .then_some((group.cursors().start, top, bottom))
        })
    }

    /// Append reading fragments across successive pages, preserving whole words
    /// and clipping them to the requested glyph range. Without a configured
    /// viewport, extract the range directly from the document.
    pub fn append_reading_fragments(
        &self,
        range: Range<u32>,
        origin: ReadingOrigin,
        fragments: &mut Vec<ReadingFragment>,
    ) {
        if range.is_empty() {
            return;
        }
        let text = self.doc.text();
        let reading = self.doc.addressing();
        // Before a viewport is configured there are no page boundaries;
        // reading remains available directly from the document's source range.
        if self.layout.size.height <= 0.0 || self.layout.col_count <= 0.0 {
            let end = range.end;
            fragments.extend(reading.reading_page(range, end).fragments);
            return;
        }
        let mut scratch = PaginationScratch::default();
        let (mut offset, mut cursor) = match origin {
            ReadingOrigin::Document => (
                text.lines()
                    .first()
                    .map(|line| line.point().y)
                    .unwrap_or(0.0),
                None,
            ),
            ReadingOrigin::Cursor(cursor) => (0.0, Some(cursor)),
            ReadingOrigin::Offset(offset) => (offset, None),
        };
        let mut consumed = range.start;
        loop {
            let request = cursor.map_or(
                PageRequest::StartingAtOffset(offset),
                PageRequest::StartingAt,
            );
            let page = self.compose(request, Vec::new(), &mut scratch);
            let end = page
                .positions
                .iter()
                .filter_map(|(index, _)| text.line(index).map(|line| line.end()))
                .max()
                .unwrap_or(consumed);
            let result = reading.reading_page(consumed..range.end, end);
            fragments.extend(result.fragments);
            consumed = result.next;
            if consumed >= range.end || page.reached_end {
                break;
            }
            let Some(next) = self.next_page_anchor(Some(&page), offset) else {
                break;
            };
            if next.cursor == cursor && next.offset_y <= offset + 0.01 {
                break;
            }
            cursor = next.cursor;
            offset = next.offset_y;
        }
    }

    fn events_for_lines(
        &self,
        start_line: usize,
        end_line: usize,
        scratch: &mut PaginationScratch,
    ) {
        let mut structure = scratch.structure.for_page(
            self.doc,
            start_line.saturating_sub(1)..end_line.saturating_add(2),
            self.layout.size.height,
        );
        scratch.events.tables.clear();
        if self.book_optimized {
            scratch.events.tables.extend_from_slice(structure.table_dependencies());
        }
        scratch.events.forced_breaks.clear();
        scratch.keep_context.prepare(&structure, self.doc);
        self.forced_break_events(
            &structure,
            &scratch.keep_context,
            &mut scratch.events.forced_breaks,
            &mut scratch.authored_boundaries,
            &mut scratch.merged_forced_breaks,
        );
        scratch.events.keeps.clear();
        // Floats participate in the same source cuts as their wrapped text.
        // Being floated is not an implicit break-inside:avoid constraint.
        // Authored keeps, figure keeps and whole-table preferences are retained
        // by the structural events below.
        self.automatic_keep_events(
            &mut scratch.events.keeps,
            &mut scratch.semantic_ranges,
            &scratch.keep_context,
            &mut scratch.range_lines,
        );
    }

    fn page_from_line_at_with_scratch(
        &self,
        start_line: usize,
        start_offset_y: f64,
        positions: Vec<LineScreen>,
        scratch: &mut PaginationScratch,
    ) -> PagePositions {
        self.page_from_line_until_with_scratch(start_line, start_offset_y, None, positions, scratch)
    }

    fn page_from_line_until_with_scratch(
        &self,
        start_line: usize,
        start_offset_y: f64,
        end_offset_y: Option<f64>,
        positions: Vec<LineScreen>,
        scratch: &mut PaginationScratch,
    ) -> PagePositions {
        self.forward_page(start_line, start_offset_y, end_offset_y, positions, scratch, true)
    }

    fn forward_page(
        &self,
        start_line: usize,
        start_offset_y: f64,
        end_offset_y: Option<f64>,
        positions: Vec<LineScreen>,
        scratch: &mut PaginationScratch,
        paint: bool,
    ) -> PagePositions {
        self.prepare_cache(scratch);
        let probe_buffer = std::mem::take(&mut scratch.probe_positions);
        let probe = self.build_forward_positions(
            start_line,
            start_offset_y,
            end_offset_y,
            probe_buffer,
            &[],
            &[],
            &[],
            None,
            &mut scratch.slices,
            false,
        );
        let end_line = probe.last_line.unwrap_or(probe.start_line);
        scratch.probe_positions = probe.positions.into_buffer();
        self.events_for_lines(probe.start_line, end_line, scratch);
        let page = self.build_forward_positions(
            start_line,
            start_offset_y,
            end_offset_y,
            positions,
            &scratch.events.tables,
            &scratch.events.forced_breaks,
            &scratch.events.keeps,
            Some(&mut scratch.vertical_rhythm),
            &mut scratch.slices,
            paint,
        );
        if end_offset_y.is_none()
            && let Some((cursor, next)) = page.continuation
            && cursor == start_line
            && let Some(position) = self.pagination().locate(PaginationTarget::Cursor(cursor))
        {
            let cuts = scratch.continuation_cuts.entry(cursor)
                .or_insert_with(|| vec![position.group.point().y]);
            // Only extend a known chain. An arbitrary restored offset does
            // not define the canonical cuts preceding it.
            if cuts.last().is_some_and(|last| (*last - start_offset_y).abs() <= 0.01)
                && next > start_offset_y + 0.01
            {
                cuts.push(next);
            }
        }
        page
    }

    fn keep_range_event(
        &self,
        trigger_line: usize,
        mut top: f64,
        mut bottom: f64,
    ) -> Option<KeepRangeEvent> {
        if bottom <= top + 0.01 {
            return None;
        }
        let lines = self.pagination();
        let group = lines.groups(trigger_line..trigger_line.saturating_add(1)).next()?;
        if group.height() <= self.layout.size.height {
            // An inner keep moves this entire atomic group. Use the same
            // boundary for fitting it and recording its translation; a cut
            // inside the group would give neighbouring content another offset.
            top = top.min(group.point().y);
            bottom = bottom.max(group.point().y + group.height());
        }
        (bottom > top + 0.01).then_some(KeepRangeEvent {
            trigger_line: group.cursors().start,
            top,
            bottom,
        })
    }

    fn leading_content_bottom(&self, owner: BoxId, count: usize, bottom: f64) -> f64 {
        let range = self.doc.boxes().content_line_range(owner);
        let pagination = self.pagination();
        (layout_state::line_id_index(range.start)..layout_state::line_id_index(range.end))
            .filter_map(|index| self.doc.text().line(LineId::new(index)))
            .filter(|line| !line.glyphs().is_empty()
                && self.doc.semantics().contains(owner, line.owner_box()))
            .take(count)
            .filter_map(|line| pagination.locate(PaginationTarget::Line(line.id())))
            .map(|position| position.group.point().y + position.group.height())
            .fold(bottom, f64::max)
    }

    fn automatic_keep_events(
        &self,
        events: &mut Vec<KeepRangeEvent>,
        semantic_ranges: &mut Vec<(usize, f64, f64)>,
        context: &KeepContext,
        range_lines: &mut Vec<usize>,
    ) {
        let lines = self.pagination();
        for block in &context.blocks {
            let constraints = &block.constraints;
            let point = block.bounds.origin();
            let bottom = block.bounds.y1;
            let semantics = block.roles;
            range_lines.clear();
            if block.is_block_container {
                context.lines_in_range(point.y, bottom, range_lines);
            }
            if (constraints.avoid_inside
                || (self.book_optimized
                    && (semantics.has_role(ContentRole::Figure)
                        || semantics.has_role(ContentRole::Sidebar))))
                && !constraints.has_forced_descendant
                && let Some(trigger_line) = block.before
                && let Some(event) = self.keep_range_event(trigger_line, point.y, bottom)
            {
                events.push(event);
            }

            if constraints.owns_line_flow
                && let Some(paragraph_lines) = context.paragraph_lines.get(&block.owner)
                && paragraph_lines.len() > 1
            {
                for (count, from_end) in [(constraints.orphans, false), (constraints.widows, true)]
                {
                    let count = count.min(paragraph_lines.len());
                    if count <= 1 {
                        continue;
                    }
                    let selected = if from_end {
                        &paragraph_lines[paragraph_lines.len() - count..]
                    } else {
                        &paragraph_lines[..count]
                    };
                    let group_top = selected.iter().map(|entry| entry.1)
                        .fold(f64::INFINITY, f64::min);
                    let group_bottom = selected.iter().map(|entry| entry.2)
                        .fold(f64::NEG_INFINITY, f64::max);
                    if let Some(&(trigger_line, _, _)) = selected.first()
                        && let Some(event) = self.keep_range_event(trigger_line, group_top, group_bottom)
                    {
                        events.push(event);
                    }
                }
            }
        }

        for pair in &context.siblings {
            if pair.forced_boundary {
                continue;
            }
            let previous_point = pair.previous_bounds.origin();
            let next_point = pair.next_bounds.origin();
            let previous_bottom = pair.previous_bounds.y1;
            let next_bottom = pair.next_bounds.y1;

            if pair.avoid_boundary || (self.book_optimized && pair.caption_pair) {
                let group_top = previous_point.y.min(next_point.y);
                let group_bottom = previous_bottom.max(next_bottom);
                if let Some(trigger_line) = pair.trigger_line
                    && let Some(event) =
                        self.keep_range_event(trigger_line, group_top, group_bottom)
                {
                    events.push(event);
                }
            } else if self.book_optimized && pair.heading_following {
                // Look ahead in the following sibling itself, even when its
                // first lines fall beyond the page probe. Geometric overlap
                // can otherwise borrow lines from a float beside the heading.
                let required_bottom = self.leading_content_bottom(pair.next_owner, 2, previous_bottom);
                if let Some(trigger_line) = pair.trigger_line
                    && let Some(event) =
                        self.keep_range_event(trigger_line, previous_point.y, required_bottom)
                {
                    events.push(event);
                }
            }
        }

        if self.book_optimized {
            semantic_keeps::semantic_keep_ranges(lines, semantic_ranges, context, range_lines);
            events.extend(
                semantic_ranges
                    .iter()
                    .filter_map(|&(trigger_line, top, bottom)| {
                        self.keep_range_event(trigger_line, top, bottom)
                    }),
            );
        }
        events.sort_by(|left, right| {
            left.trigger_line
                .cmp(&right.trigger_line)
                .then_with(|| left.top.total_cmp(&right.top))
                .then_with(|| right.bottom.total_cmp(&left.bottom))
        });
        events.dedup_by(|left, right| {
            left.trigger_line == right.trigger_line
                && (left.top - right.top).abs() <= 0.01
                && (left.bottom - right.bottom).abs() <= 0.01
        });
    }

    fn forced_break_events(
        &self,
        anchors: &StructureView<'_>,
        context: &KeepContext,
        events: &mut Vec<ForcedBreakEvent>,
        authored_boundary: &mut HashSet<usize>,
        merged: &mut Vec<ForcedBreakEvent>,
    ) {
        authored_boundary.clear();
        for boundary in anchors.authored_boundaries() {
            authored_boundary.insert(boundary.trigger_line);
            if let Some(kind) = boundary.forced {
                events.push(ForcedBreakEvent {
                    trigger_line: boundary.trigger_line,
                    top: boundary.top,
                    kind,
                });
            }
        }
        for block in &context.blocks {
            if self.book_optimized
                && block.roles.has_role(ContentRole::MajorSection)
                && let Some(trigger_line) = block.before
                && !authored_boundary.contains(&trigger_line)
            {
                events.push(ForcedBreakEvent {
                    trigger_line,
                    top: block.bounds.y0,
                    kind: RenderForcedBreak::Page,
                });
            }
        }
        events.sort_by(|left, right| {
            left.trigger_line
                .cmp(&right.trigger_line)
                .then_with(|| left.top.total_cmp(&right.top))
                .then_with(|| left.kind.cmp(&right.kind))
        });
        // Several nested boxes, or an adjoining break-after/break-before pair,
        // can describe the same boundary. Keep one event and let a page break
        // dominate a column break.
        merged.clear();
        for event in events.drain(..) {
            if let Some(previous) = merged.last_mut()
                && previous.trigger_line == event.trigger_line
            {
                if event.kind > previous.kind {
                    previous.kind = event.kind;
                }
                previous.top = previous.top.max(event.top);
            } else {
                merged.push(event);
            }
        }
        events.clear();
        events.extend(merged.drain(..));
    }
}

impl Paginator<'_> {
    fn prepare_visible_block_decorations(
        &self,
        start_offset_y: f64,
        positions: &VisibleLinePositions,
        indexes: &mut Vec<usize>,
    ) {
        let height = self.layout.size.height;
        let mut windows = vec![(
            start_offset_y,
            start_offset_y + self.layout.col_count * height,
        )];
        // A compressed or moved group can expose source geometry beyond the
        // nominal viewport interval. Include its actual column's source window.
        for (index, screen) in positions.iter() {
            if let Some(line) = self.doc.text().line(index) {
                let top = line.point().y - screen.point.y;
                windows.push((top, top + height));
            }
        }
        windows.sort_unstable_by(|left, right| left.0.total_cmp(&right.0));
        let mut merged: Vec<(f64, f64)> = Vec::new();
        for (top, bottom) in windows {
            if let Some(previous) = merged.last_mut()
                && top <= previous.1
            {
                previous.1 = previous.1.max(bottom);
            } else {
                merged.push((top, bottom));
            }
        }
        indexes.clear();
        let mut candidates = Vec::new();
        for (top, bottom) in merged {
            (self.doc.fragments()).block_decoration_indices_intersecting(top, bottom, &mut candidates);
            indexes.extend_from_slice(&candidates);
        }
        // Scene replay restores CSS order after combining the source windows.
        indexes.sort_unstable();
        indexes.dedup();
    }


    fn build_backward_positions(
        &self,
        end_line: usize,
        minimum_line: usize,
        positions: Vec<LineScreen>,
        events: Option<&PaginationEvents>,
        scratch: &mut PaginationScratch,
    ) -> PagePositions {
        self.prepare_cache(scratch);
        let lines = self.pagination();
        let Some(end_data) = lines.groups(0..end_line.saturating_add(1)).next_back() else {
            return PagePositions {
                request: PageRequest::StartingAtOffset(0.0),
                geometry: self.layout,
                paint_start_offset_y: 0.0,
                start_line: 0,
                last_line: None,
                positions: page_placements::with_buffer(0, positions),
                offset_breaks: Vec::new(),
                block_decoration_indices: Some(Vec::new()),
                flow_placements: Default::default(),
                table_fragments: Default::default(),
                reached_end: true,
                next_start_offset_y: 0.0,
                continuation: None,
            };
        };
        // Cursor order follows group starts, not their bottoms: a table row
        // can precede a short body line while extending below it. Measure
        // backwards from the complete extent of the requested prefix, or
        // clamping that row's negative reverse distance pulls it over its
        // neighbours. Use the same prefix for the probe and final pass even
        // when the latter has a narrower minimum_line.
        // This changes only the geometric origin. end_line remains the
        // membership boundary, so the next page still resumes at the same
        // cursor without including any of its groups here.
        let end_bottom = lines
            .groups(0..end_line.saturating_add(1))
            .map(|group| group.point().y + group.height())
            .fold(end_data.point().y + end_data.height(), f64::max);
        let mut render_state = initial_render_state();
        let mut reversed = std::mem::take(&mut scratch.reverse_positions);
        reversed.clear();

        let reverse_range = |top: f64, bottom: f64| (end_bottom - bottom, end_bottom - top);
        let range_trigger = |top: f64, bottom: f64| {
            (minimum_line..=end_line).rev().find_map(|cursor| {
                let position = lines.locate(PaginationTarget::Cursor(cursor))?;
                let line = position.line;
                if line.point().y < bottom - 0.01 && line.point().y + line.height() > top + 0.01 {
                    position.group.cursors().end.checked_sub(1)
                } else {
                    None
                }
            })
        };
        let mut reverse_keeps = std::mem::take(&mut scratch.reverse_keeps);
        reverse_keeps.clear();
        if let Some(events) = events {
            reverse_keeps.extend(events.keeps.iter().filter_map(|event| {
                let trigger_line = range_trigger(event.top, event.bottom)?;
                let (top, bottom) = reverse_range(event.top, event.bottom);
                Some(KeepRangeEvent {
                    trigger_line,
                    top,
                    bottom,
                })
            }));
        }
        let mut reverse_tables = std::mem::take(&mut scratch.reverse_tables);
        reverse_tables.clear();
        if let Some(events) = events {
            reverse_tables.extend(events.tables.iter().filter_map(|event| {
                let trigger_line = range_trigger(event.top, event.bottom)?;
                let (top, bottom) = reverse_range(event.top, event.bottom);
                Some(TablePaginationEvent {
                    table_box: event.table_box,
                    trigger_line,
                    top,
                    bottom,
                    kind: match event.kind {
                        TablePaginationEventKind::WholeTable => {
                            TablePaginationEventKind::WholeTable
                        }
                        TablePaginationEventKind::RowGroup { forced, allow_content_break, .. } => {
                            TablePaginationEventKind::RowGroup {
                                forced,
                                allow_content_break,
                            }
                        }
                    },
                })
            }));
        }

        let mut groups = lines
            .groups(minimum_line..end_line.saturating_add(1))
            .rev()
            .peekable();
        let mut first_slice = None;
        let mut reverse_breaks: Vec<(f64, f64)> = Vec::new();
        'groups: while let Some(line) = groups.next() {
            let line_idx = line.cursors().end - 1;
            let effective_height = line.height();
            let reverse_y = (end_bottom - (line.point().y + effective_height)).max(0.0);
            let oversized = scratch.slices.groups.contains_key(&line.cursors().start)
                || can_fragment(self.doc, &line, self.layout.size.height);
            render_state.y_offset = reverse_breaks.iter().filter(|(at, _)| *at <= reverse_y + 0.01)
                .max_by(|a, b| a.0.total_cmp(&b.0)).map_or(0.0, |(_, offset)| *offset);

            if line_idx < end_line
                && let Some(forced) = events.and_then(|events| {
                    events
                        .forced_breaks
                        .iter()
                        .find(|event| event.trigger_line == line_idx + 1)
                })
            {
                match forced.kind {
                    RenderForcedBreak::Page => break,
                    RenderForcedBreak::Column => {
                        let projected = reverse_y + render_state.y_offset;
                        let local = projected.rem_euclid(self.layout.size.height);
                        if local > 0.01 {
                            render_state.y_offset += self.layout.size.height - local;
                        }
                    }
                }
            }
            for event in reverse_keeps
                .iter()
                .filter(|event| event.trigger_line == line_idx
                    && (!oversized || event.top <= reverse_y + 0.01))
            {
                apply_keep_range_event(&self.layout, *event, 0.0, &mut render_state);
            }
            for event in reverse_tables
                .iter()
                .filter(|event| event.trigger_line == line_idx
                    && (!oversized || event.top <= reverse_y + 0.01))
            {
                apply_table_pagination_event(&self.layout, *event, 0.0, &mut render_state);
            }

            reverse_breaks.push((reverse_y, render_state.y_offset));
            if oversized {
                let source = scratch.slices.group(self.doc, &line);
                let mut visible_members = Vec::new();
                let mut slice_bottom = line.point().y + effective_height;
                while slice_bottom > line.point().y + 0.01 {
                    let reverse_top = end_bottom - slice_bottom;
                    let mut limit = (reverse_top + self.layout.size.height).min(end_bottom - line.point().y);
                    for (top, bottom) in reverse_keeps.iter().filter(|event| event.trigger_line == line_idx)
                        .map(|event| (event.top, event.bottom))
                        .chain(reverse_tables.iter().filter(|event| event.trigger_line == line_idx)
                            .filter(|event| matches!(event.kind, TablePaginationEventKind::WholeTable
                                | TablePaginationEventKind::RowGroup { allow_content_break: false, .. }))
                            .map(|event| (event.top, event.bottom)))
                    {
                        if top > reverse_top + 0.01 && top < limit - 0.01
                            && bottom > limit + 0.01 && bottom - top <= self.layout.size.height + 0.01
                        { limit = top; }
                    }
                    if let Some(events) = events {
                        for event in &events.forced_breaks {
                            let at = end_bottom - event.top;
                            if at > reverse_top + 0.01 && at < limit - 0.01 { limit = at; }
                        }
                    }
                    let slice_top = content_break(&source.spans, slice_bottom, end_bottom - limit, false);
                    let slice_height = slice_bottom - slice_top;
                    let (reverse_point, next_state) = resolve_point_at(
                        &self.layout,
                        Point::new(line.point().x, end_bottom - slice_bottom),
                        slice_height,
                        render_state,
                        0.0,
                    );
                    render_state = next_state;
                    reverse_breaks.push((reverse_top, render_state.y_offset));
                    let Some(reverse_point) = reverse_point else { break 'groups };
                    let column = (self.layout.col_count as i32 - 1 - render_state.col_index as i32).max(0);
                    let top = self.layout.size.height - reverse_point.y - slice_height;
                    visible_members.clear();
                    source.members.intersecting(slice_top, slice_bottom, &mut visible_members);
                    visible_members.sort_unstable();
                    visible_members.dedup();
                    for &cursor in visible_members.iter().rev() {
                        let cursor = cursor as usize;
                        let Some(member) = lines.locate(PaginationTarget::Cursor(cursor)).map(|position| position.line) else { continue };
                        reversed.push((cursor, LineScreen {
                            point: Point::new(
                                page_placements::col_x(&self.layout, column as f64, member.point().x),
                                top + member.point().y - slice_top,
                            ),
                            col_index: column,
                            visible_y: Some((top, top + slice_height)),
                        }));
                    }
                    first_slice = Some((line.cursors().start, slice_top));
                    slice_bottom = slice_top;
                    if events.is_some_and(|events| events.forced_breaks.iter().any(|event|
                        matches!(event.kind, RenderForcedBreak::Page)
                            && (event.top - slice_bottom).abs() <= 0.01))
                    {
                        break 'groups;
                    }
                    if slice_bottom > line.point().y + 0.01 {
                        render_state.y_offset += self.layout.size.height - reverse_point.y - slice_height;
                        reverse_breaks.push((end_bottom - slice_bottom, render_state.y_offset));
                    }
                }
                continue;
            }
            let fit_height = effective_height.min(self.layout.size.height);
            let (reverse_point, next_state) = resolve_point_at(
                &self.layout,
                kurbo::Point::new(line.point().x, reverse_y),
                fit_height,
                render_state,
                0.0,
            );
            render_state = next_state;
            reverse_breaks.push((reverse_y, render_state.y_offset));
            let Some(reverse_point) = reverse_point else {
                break;
            };
            let reverse_col = render_state.col_index as i32;
            let col_index = (self.layout.col_count as i32 - 1 - reverse_col).max(0);
            let point = kurbo::Point::new(
                page_placements::col_x(&self.layout, col_index as f64, line.point().x),
                (self.layout.size.height - reverse_point.y - fit_height).max(0.0),
            );
            for (cursor, member) in line.cursors().rev().zip(line.lines().rev()) {
                reversed.push((
                    cursor,
                    LineScreen {
                        point: point + (member.point() - line.point()),
                        col_index,
                        visible_y: None,
                    },
                ));
            }

        }

        reversed.reverse();
        let start_line = reversed.first().map_or(end_line, |(line_idx, _)| {
            lines.locate(PaginationTarget::Cursor(*line_idx))
                .map_or(*line_idx, |position| position.group.cursors().start)
        });
        let mut paint_start_offset_y = lines
            .locate(PaginationTarget::Cursor(start_line))
            .map_or(0.0, |position| position.group.point().y);
        if let Some((cursor, top)) = first_slice
            && cursor == start_line
        {
            paint_start_offset_y = top;
        }
        let mut visible = page_placements::with_buffer(start_line, positions);
        let mut offset_breaks: Vec<(f64, f64)> = Vec::new();
        let mut previous_offset: Option<f64> = None;
        for (line_idx, screen) in reversed.drain(..) {
            let position = lines
                .locate(PaginationTarget::Cursor(line_idx))
                .expect("positioned reverse line must exist");
            let group_y = position.group.point().y;
            let line = position.line;
            let offset = screen.point.y
                + screen.col_index as f64 * self.layout.size.height
                + paint_start_offset_y
                - line.point().y;
            if previous_offset.is_none_or(|previous| (previous - offset).abs() > 1e-6) {
                offset_breaks.push((group_y, offset));
                previous_offset = Some(offset);
            }
            page_placements::push_fragment(&mut visible, line.id(), screen);
        }
        offset_breaks.sort_by(|left, right| left.0.total_cmp(&right.0));
        let mut block_decoration_indices = Vec::new();
        if events.is_some() {
            self.prepare_visible_block_decorations(
                paint_start_offset_y,
                &visible,
                &mut block_decoration_indices,
            );
        }
        scratch.reverse_positions = reversed;
        scratch.reverse_keeps = reverse_keeps;
        scratch.reverse_tables = reverse_tables;
        let next_group = lines.groups(end_line.saturating_add(1)..lines.len()).next();
        let mut page = PagePositions {
            request: if first_slice.is_some_and(|(cursor, top)| cursor == start_line
                && lines.locate(PaginationTarget::Cursor(cursor))
                    .is_some_and(|position| top > position.group.point().y + 0.01))
            {
                PageRequest::StartingAtOffset(paint_start_offset_y)
            } else {
                PageRequest::EndingAt(end_line)
            },
            geometry: self.layout,
            paint_start_offset_y,
            start_line,
            last_line: (!visible.is_empty()).then_some(end_line),
            positions: visible,
            offset_breaks,
            block_decoration_indices: Some(block_decoration_indices),
            flow_placements: Default::default(),
            table_fragments: Default::default(),
            reached_end: next_group.is_none(),
            next_start_offset_y: next_group.map_or(end_bottom, |group| group.point().y),
            continuation: None,
        };
        if events.is_none() { return page; }
        super::table_fragments::finish(self.doc, &mut page);
        super::projection::compose_box_fragments(self.doc, &mut page);
        page
    }
    fn build_forward_positions(
        &self,
        start_line: usize,
        start_offset_y: f64,
        end_offset_y: Option<f64>,
        positions: Vec<LineScreen>,
        table_events: &[TablePaginationEvent],
        forced_break_events: &[ForcedBreakEvent],
        keep_events: &[KeepRangeEvent],
        rhythm_scratch: Option<&mut VerticalRhythmScratch>,
        slices: &mut SliceCache,
        paint: bool,
    ) -> PagePositions {
        let lines = self.pagination();
        let requested_offset_y = start_offset_y;
        let mut table_event_idx =
            table_events.partition_point(|event| event.trigger_line < start_line);
        let mut forced_break_event_idx =
            forced_break_events.partition_point(|event| event.trigger_line < start_line);
        let mut keep_event_idx =
            keep_events.partition_point(|event| event.trigger_line < start_line);
        let mut positions = page_placements::with_buffer(start_line, positions);
        let mut offset_breaks: Vec<(f64, f64)> = Vec::new();
        let mut render_state = initial_render_state();
        let mut last_line: Option<usize> = None;
        let mut next_start_offset_y = start_offset_y;
        let mut continuation = None;
        let mut sliced_content = false;
        let mut groups = lines.groups(start_line..lines.len()).peekable();
        while let Some(line) = groups.next() {
            let line_idx = line.cursors().start;
            let oversized = slices.groups.contains_key(&line.cursors().start)
                || can_fragment(self.doc, &line, self.layout.size.height);
            // A previously visited container can have scheduled cuts below
            // this group's source origin. Those future offsets do not apply
            // to parallel text beside the container.
            render_state.y_offset = offset_breaks.iter()
                .filter(|(at, _)| *at <= line.point().y.max(start_offset_y) + 0.01)
                .max_by(|a, b| a.0.total_cmp(&b.0))
                .map_or(0.0, |(_, offset)| *offset);

            if line.point().y < start_offset_y - 0.01 && line.height() > self.layout.size.height {
                // The group's start rules were applied on its first fragment.
                forced_break_event_idx = forced_break_events.partition_point(|event| event.trigger_line <= line_idx);
                keep_event_idx = keep_events.partition_point(|event| event.trigger_line <= line_idx);
                table_event_idx = table_events.partition_point(|event| event.trigger_line <= line_idx);
            }
            while forced_break_events
                .get(forced_break_event_idx)
                .is_some_and(|event| event.trigger_line == line_idx)
            {
                // Nested constraints belong to their source boundary, not to
                // the first cursor of the enclosing oversized group.
                if oversized && forced_break_events[forced_break_event_idx].top > line.point().y + 0.01 {
                    forced_break_event_idx += 1;
                    continue;
                }
                let previous_offset = render_state.y_offset;
                let previous_line = last_line
                    .and_then(|cursor| lines.locate(PaginationTarget::Cursor(cursor)))
                    .and_then(|position| positions.fragments(position.line.id()).last());
                apply_forced_break_event(
                    &self.layout,
                    forced_break_events[forced_break_event_idx],
                    start_offset_y,
                    previous_line,
                    &mut render_state,
                );
                if (render_state.y_offset - previous_offset).abs() > 1e-6 {
                    offset_breaks.push((
                        forced_break_events[forced_break_event_idx].top,
                        render_state.y_offset,
                    ));
                }
                forced_break_event_idx += 1;
            }
            while keep_events
                .get(keep_event_idx)
                .is_some_and(|event| event.trigger_line == line_idx)
            {
                // Nested constraints belong to their source boundary, not to
                // the first cursor of the enclosing oversized group.
                if oversized && keep_events[keep_event_idx].top > line.point().y + 0.01 {
                    keep_event_idx += 1;
                    continue;
                }
                let previous_offset = render_state.y_offset;
                apply_keep_range_event(
                    &self.layout,
                    keep_events[keep_event_idx],
                    start_offset_y,
                    &mut render_state,
                );
                if (render_state.y_offset - previous_offset).abs() > 1e-6 {
                    offset_breaks.push((keep_events[keep_event_idx].top, render_state.y_offset));
                }
                keep_event_idx += 1;
            }
            while table_events
                .get(table_event_idx)
                .is_some_and(|event| event.trigger_line == line_idx)
            {
                // Nested constraints belong to their source boundary, not to
                // the first cursor of the enclosing oversized group.
                if oversized && table_events[table_event_idx].top > line.point().y + 0.01 {
                    table_event_idx += 1;
                    continue;
                }
                let previous_offset = render_state.y_offset;
                apply_table_pagination_event(
                    &self.layout,
                    table_events[table_event_idx],
                    start_offset_y,
                    &mut render_state,
                );
                if (render_state.y_offset - previous_offset).abs() > 1e-6 {
                    offset_breaks.push((table_events[table_event_idx].top, render_state.y_offset));
                }
                table_event_idx += 1;
            }
            let effective_height = line.height();
            if oversized {
                sliced_content = true;
                let group_bottom = line.point().y + effective_height;
                let bottom = end_offset_y.map_or(group_bottom, |end| end.min(group_bottom));
                let source = slices.group(self.doc, &line);
                let mut visible_members = Vec::new();
                let mut slice_top = line.point().y.max(start_offset_y);
                while slice_top < bottom - 0.01 {
                    let mut limit = (slice_top + self.layout.size.height).min(bottom);
                    for event in forced_break_events.iter().filter(|event| event.trigger_line == line_idx) {
                        if event.top > slice_top + 0.01 && event.top < limit - 0.01 { limit = event.top; }
                    }
                    for (top, end) in keep_events.iter().filter(|event| event.trigger_line == line_idx)
                        .map(|event| (event.top, event.bottom))
                        .chain(table_events.iter().filter(|event| event.trigger_line == line_idx)
                            .filter(|event| matches!(event.kind, TablePaginationEventKind::WholeTable
                                | TablePaginationEventKind::RowGroup { allow_content_break: false, .. }))
                            .map(|event| (event.top, event.bottom)))
                    {
                        if top > slice_top + 0.01 && top < limit - 0.01
                            && end > limit + 0.01 && end - top <= self.layout.size.height + 0.01
                        { limit = top; }
                    }
                    let slice_bottom = content_break(&source.spans, slice_top, limit, true);
                    let slice_height = slice_bottom - slice_top;
                    let previous_offset = render_state.y_offset;
                    let (point, next_state) = resolve_point_at(
                        &self.layout,
                        Point::new(line.point().x, slice_top),
                        slice_height,
                        render_state,
                        start_offset_y,
                    );
                    render_state = next_state;
                    let Some(point) = point else {
                        continuation = Some((line_idx, slice_top));
                        next_start_offset_y = slice_top;
                        break;
                    };
                    if (render_state.y_offset - previous_offset).abs() > 1e-6 {
                        offset_breaks.push((slice_top, render_state.y_offset));
                    }
                    // A source line can paint in several columns. Each mapping
                    // is clipped to its column by the shared page projection.
                    visible_members.clear();
                    source.members.intersecting(slice_top, slice_bottom, &mut visible_members);
                    visible_members.sort_unstable();
                    visible_members.dedup();
                    for &cursor in &visible_members {
                        let Some(member) = lines.locate(PaginationTarget::Cursor(cursor as usize)).map(|position| position.line) else { continue };
                        let screen = LineScreen {
                            point: point + (member.point() - Point::new(line.point().x, slice_top)),
                            col_index: render_state.col_index as i32,
                            visible_y: Some((point.y, point.y + slice_height)),
                        };
                        page_placements::push_fragment(&mut positions, member.id(), screen);
                    }
                    last_line = line.cursors().end.checked_sub(1);
                    slice_top += slice_height;
                    let page_break = forced_break_events.iter().any(|event|
                        event.trigger_line == line_idx
                            && matches!(event.kind, RenderForcedBreak::Page)
                            && (event.top - slice_top).abs() <= 0.01);
                    if page_break {
                        // A page break skips every remaining column, including
                        // when the forced boundary coincides with a natural cut.
                        render_state.y_offset = self.layout.col_count * self.layout.size.height
                            + start_offset_y - slice_top;
                        offset_breaks.push((slice_top, render_state.y_offset));
                    } else if slice_top < bottom - 0.01 {
                        render_state.y_offset += self.layout.size.height - point.y - slice_height;
                        offset_breaks.push((slice_top, render_state.y_offset));
                    }
                }
                if bottom < group_bottom - 0.01 && continuation.is_none() {
                    continuation = Some((line_idx, bottom));
                    next_start_offset_y = bottom;
                    render_state.should_stop = true;
                }
                if continuation.is_some() {
                    break;
                }
                if end_offset_y.is_some_and(|end| bottom >= end - 0.01) {
                    if let Some(next) = groups.peek() {
                        render_state.should_stop = true;
                        next_start_offset_y = next.point().y;
                    }
                    break;
                }
                continue;
            }
            let fit_height = effective_height.min(self.layout.size.height);
            let previous_offset = render_state.y_offset;
            let (point, next_state) = resolve_point_at(
                &self.layout,
                line.point(),
                fit_height,
                render_state,
                start_offset_y,
            );
            render_state = next_state;
            if (render_state.y_offset - previous_offset).abs() > 1e-6 {
                offset_breaks.push((line.point().y, render_state.y_offset));
            }
            if let Some(point) = point {
                for member in line.lines() {
                    let screen = LineScreen {
                        point: point + (member.point() - line.point()),
                        col_index: render_state.col_index as i32,
                        visible_y: None,
                    };
                    page_placements::push(&mut positions, member.id(), screen);
                }
                last_line = line.cursors().end.checked_sub(1);


            }
            if render_state.should_stop {
                next_start_offset_y = line.point().y;
                break;
            }
        }
        let reached_end = !render_state.should_stop;
        offset_breaks.sort_by(|left, right| left.0.total_cmp(&right.0));
        let mut page = PagePositions {
            request: PageRequest::StartingAtOffset(requested_offset_y),
            geometry: self.layout,
            paint_start_offset_y: start_offset_y,
            start_line,
            last_line,
            positions,
            offset_breaks,
            block_decoration_indices: Some(Vec::new()),
            flow_placements: Default::default(),
            table_fragments: Default::default(),
            reached_end,
            next_start_offset_y,
            continuation,
        };
        if !sliced_content
            && self.vertical_rhythm
            && self.book_optimized
            && let Some(scratch) = rhythm_scratch
        {
            scratch.rigid_ranges.clear();
            scratch.rigid_ranges.extend(
                table_events
                    .iter()
                    .map(|event| (event.top, event.bottom))
                    .chain(keep_events.iter().map(|event| (event.top, event.bottom))),
            );
            self.apply_vertical_rhythm(&mut page, scratch);
        }
        if !paint { return page; }
        self.prepare_visible_block_decorations(
            start_offset_y,
            &page.positions,
            page.block_decoration_indices
                .as_mut()
                .expect("composed page decorations"),
        );
        super::table_fragments::finish(self.doc, &mut page);
        super::projection::compose_box_fragments(self.doc, &mut page);
        page
    }


}

#[cfg(test)]
mod tests {
    use html_pipeline::engine::LayoutRequest;



    use crate::text::{FontMetrics, ShapedRun, TextRunShapeRequest, ShapedGlyph};

    use super::*;
    use crate::text::{FontSlant, GlyphHandle, GlyphResourceGeneration, GlyphShaper};
    use html_pipeline::DocumentFactory;
    use html_pipeline::engine::GlyphResourceStore;
    use html_pipeline::engine::{LaidOutDocument, LayoutConstraints};
    use kurbo::Size;
    use std::collections::HashMap;

    /// Fixed-metric shaper: every glyph is half the font size wide and
    /// three-quarters tall, so line heights in these tests are predictable.
    #[derive(Default)]
    struct TestShaper {
        glyphs: HashMap<(char, u32), GlyphHandle>,
        generation: GlyphResourceGeneration,
    }

    impl GlyphShaper for TestShaper {
        fn resource_generation(&self) -> GlyphResourceGeneration {
            self.generation
        }

        fn shape_run(&mut self, request: TextRunShapeRequest<'_>) -> ShapedRun {
            ShapedRun::characters(request, |character, style| {
                let key = (character, style.font_size.to_bits());
                let advance = style.font_size * 0.5;
                let metrics = FontMetrics {
                    ascent: style.font_size * 0.75,
                    descent: style.font_size * 0.25,
                };
                if let Some(&glyph) = self.glyphs.get(&key) {
                    return ShapedGlyph::new(glyph, advance, metrics);
                }
                let glyph = GlyphHandle(self.glyphs.len() as u32);
                self.glyphs.insert(key, glyph);
                ShapedGlyph::new(glyph, advance, metrics)
            })
        }
    }

    fn layout(
        html: &str,
        width: f64,
        resources: &mut GlyphResourceStore,
    ) -> LaidOutDocument<'static> {
        let mut factory = DocumentFactory::new();
        let mut shaper = TestShaper::default();
        factory
            .parse_with_new_pipeline(html, None)
            .shape(&mut shaper, resources)
            .expect("the test shaper registers every glyph")
            .layout(
                LayoutRequest::new(
                    LayoutConstraints::new(width, 16.0).expect("constraints must be valid"),
                ),
                &mut shaper,
                resources,
            )
            .expect("test layout succeeds")
    }

    fn columns(width: f64, height: f64, count: f64) -> PageGeometry {
        PageGeometry {
            col_width: width,
            col_count: count,
            col_gap: 0.0,
            size: Size::new(width * count, height),
        }
    }

    /// Paginates one page and returns (line index, column, screen y) per line.
    fn paginate(
        document: &LaidOutDocument,
        layout: &PageGeometry,
        mode: PaginationMode,
        resources: &GlyphResourceStore,
    ) -> Vec<(LineId, i32, f64)> {
        let paginator = Paginator::new((document).render_view(resources), *layout, mode);
        let page = paginator.compose(
            PageRequest::StartingAtOffset(0.0),
            Vec::new(),
            &mut PaginationScratch::default(),
        );
        page.positions
            .iter()
            .map(|(line, screen)| (line, screen.col_index, screen.point.y))
            .collect()
    }

    fn column_of(placed: &[(LineId, i32, f64)], line: LineId) -> Option<i32> {
        placed
            .iter()
            .find(|(idx, _, _)| *idx == line)
            .map(|(_, col, _)| *col)
    }

    const PARAGRAPHS: &str = "<p>aaa</p><p>bbb</p><p>ccc</p><p>ddd</p><p>eee</p><p>fff</p>";

    #[test]
    fn wrapped_list_placements_preserve_line_identity_in_both_directions() {
        let mut resources = GlyphResourceStore::default();
        let document = layout(
            "<body style='margin:0'><ul style='margin:0'><li>First item wraps onto several lines with its bullet beside the first line.</li><li>Another wrapped item keeps its continuation below its own first line.</li></ul></body>",
            200.0,
            &mut resources,
        );
        let view = document.render_view(&resources);
        let lines = view.text().lines();
        assert!(
            lines
                .iter()
                .enumerate()
                .any(|(rank, line)| rank != layout_state::line_id_index(line.id()))
        );
        let geometry = columns(200.0, 2000.0, 1.0);
        for mode in [
            PaginationMode::Web,
            PaginationMode::Book,
            PaginationMode::BookWithVerticalRhythm,
        ] {
            let paginator = Paginator::new((&document).render_view(&resources), geometry, mode);
            for page in [
                paginator.compose(
                    PageRequest::StartingAtOffset(0.0),
                    Vec::new(),
                    &mut PaginationScratch::default(),
                ),
                paginator.compose(
                    PageRequest::EndingAt(lines.len() - 1),
                    Vec::new(),
                    &mut PaginationScratch::default(),
                ),
            ] {
                assert_eq!(page.positions.iter().count(), lines.len());
                let first = lines.first().unwrap();
                let displacement = page.positions.get(first.id()).unwrap().point - first.point();
                for line in lines.iter() {
                    let actual = page
                        .positions
                        .get(line.id())
                        .expect("every line has a placement");
                    assert_eq!(actual.col_index, 0);
                    let expected = line.point() + displacement;
                    assert!(
                        (actual.point - expected).hypot() < 0.001,
                        "{mode:?}: line {:?} must retain its own geometry: actual={:?}, expected={expected:?}",
                        line.id(),
                        actual.point
                    );
                }
            }
        }
    }

    #[test]
    fn paginating_wrapped_lists_across_pages_keeps_bullets_with_their_text() {
        let mut resources = GlyphResourceStore::default();
        let content =
            "<li>Several words make this list item wrap across multiple lines.</li>".repeat(6);
        let document = layout(
            &format!("<body style='margin:0'><ul style='margin:0'>{content}</ul></body>"),
            200.0,
            &mut resources,
        );
        let view = document.render_view(&resources);
        let geometry = columns(200.0, 115.0, 2.0);
        for mode in [
            PaginationMode::Web,
            PaginationMode::Book,
            PaginationMode::BookWithVerticalRhythm,
        ] {
            let paginator = Paginator::new((&document).render_view(&resources), geometry, mode);
            let mut seen = HashSet::new();
            let mut offset = 0.0;
            for _ in 0..view.text().line_count() {
                let page = paginator.compose(
                    PageRequest::StartingAtOffset(offset),
                    Vec::new(),
                    &mut PaginationScratch::default(),
                );
                assert!(!page.positions.is_empty());
                for (id, screen) in page.positions.iter() {
                    assert!(
                        seen.insert(id),
                        "line {id:?} must appear on exactly one page"
                    );
                    let line = view.text().line(id).unwrap();
                    assert_eq!(
                        screen.point.x,
                        page_placements::col_x(
                            &geometry,
                            f64::from(screen.col_index),
                            line.point().x
                        )
                    );
                    let content: String = line.glyphs()
                        .map(|glyph| {
                            (view.text()).glyph_metric((view.text()).glyph_at(glyph as usize).unwrap())
                            .unwrap()
                            .ch()
                        })
                        .collect();
                    if content == "•" {
                        let first_text = view
                            .text()
                            .lines()
                            .iter()
                            .find(|candidate| {
                                candidate.id() != id
                                    && (candidate.point().y - line.point().y).abs() < 0.001
                            })
                            .unwrap();
                        let text_screen = page
                            .positions
                            .get(first_text.id())
                            .expect("a bullet and its text must share a page");
                        assert_eq!(screen.col_index, text_screen.col_index);
                        assert!((screen.point.y - text_screen.point.y).abs() < 0.001);
                    }
                }
                if page.reached_end {
                    break;
                }
                assert!(page.next_start_offset_y > offset);
                offset = page.next_start_offset_y;
            }
            assert_eq!(seen.len(), view.text().line_count());
        }
    }

    #[test]
    fn oversized_list_content_keeps_its_marker_on_the_same_page() {
        let mut resources = GlyphResourceStore::default();
        let document = layout(
            "<body style='margin:0'><ul style='margin:0'><li><span style='line-height:80px;font-size:60px'>Big</span></li></ul></body>",
            200.0,
            &mut resources,
        );
        let geometry = columns(200.0, 40.0, 1.0);
        let page = Paginator::new(
            (&document).render_view(&resources),
            geometry,
            PaginationMode::Book,
        )
        .compose(
            PageRequest::StartingAtOffset(0.0),
            Vec::new(),
            &mut PaginationScratch::default(),
        );
        assert!(page.reached_end);
        assert_eq!(
            page.positions.iter().count(),
            document.render_view(&resources).text().line_count()
        );
        for (_, screen) in page.positions.iter() {
            assert_eq!(screen.col_index, 0);
            assert_eq!(screen.point.y, 0.0);
        }
    }

    #[test]
    fn interleaved_positioned_text_does_not_split_an_oversized_list_group() {
        for other_y in [10, 30] {
            let mut resources = GlyphResourceStore::default();
            let document = layout(
                &format!(
                    "<body style='margin:0;line-height:20px'><ul style='margin:0'><li><span style='line-height:80px'>Tall first</span></li></ul><div style='position:absolute;top:{other_y}px;left:120px'>Other</div></body>"
                ),
                200.0,
                &mut resources,
            );
            let view = document.render_view(&resources);
            let geometry = columns(200.0, 40.0, 1.0);
            let item = (0..(view.boxes()).len())
                .map(layout_state::box_id_new)
                .find(|&index| (view.boxes()).tag(index) == Some("li"))
                .unwrap();
            let marker_box = (view.boxes()).list_marker(item)
                .unwrap()
                .marker_box();
            let marker = view
                .text()
                .lines()
                .iter()
                .find(|line| line.owner_box() == marker_box)
                .unwrap();
            let source = view.pagination()
                .source_line(
                    view.pagination()
                        .locate(PaginationTarget::Line(marker.id()))
                        .map(|position| position.cursor)
                        .unwrap(),
                )
                .unwrap();
            for mode in [
                PaginationMode::Web,
                PaginationMode::Book,
                PaginationMode::BookWithVerticalRhythm,
            ] {
                let paginator = Paginator::new((&document).render_view(&resources), geometry, mode);
                let mut seen = HashSet::new();
                let mut offset = 0.0;
                for _ in 0..view.text().line_count() {
                    let page = paginator.compose(
                        PageRequest::StartingAtOffset(offset),
                        Vec::new(),
                        &mut PaginationScratch::default(),
                    );
                    assert!(!page.positions.is_empty());
                    for (id, _) in page.positions.iter() {
                        assert!(seen.insert(id), "a line must not repeat across pages");
                    }
                    assert_eq!(
                        page.positions.contains(marker.id()),
                        page.positions.contains(source.id()),
                        "a marker and its text must share a page"
                    );
                    if page.reached_end {
                        break;
                    }
                    assert!(page.next_start_offset_y > offset);
                    offset = page.next_start_offset_y;
                }
                assert_eq!(seen.len(), view.text().line_count());
            }
        }
    }

    #[test]
    fn backward_page_includes_the_complete_requested_group() {
        let mut resources = GlyphResourceStore::default();
        let document = layout(
            "<body style='margin:0'><ul style='margin:0'><li>First</li></ul></body>",
            200.0,
            &mut resources,
        );
        let geometry = columns(200.0, 100.0, 1.0);
        let paginator = Paginator::new(
            (&document).render_view(&resources),
            geometry,
            PaginationMode::Book,
        );
        for end in 0..document.render_view(&resources).text().line_count() {
            let page = paginator.compose(
                PageRequest::EndingAt(end),
                Vec::new(),
                &mut PaginationScratch::default(),
            );
            assert_eq!(
                page.positions.iter().count(),
                document.render_view(&resources).text().line_count()
            );
            let mut scratch = PaginationScratch::default();
            let page = paginator.build_backward_positions(
                page.last_line.unwrap_or(0),
                0,
                Vec::new(),
                None,
                &mut scratch,
            );
            assert_eq!(
                page.positions.iter().count(),
                document.render_view(&resources).text().line_count()
            );
        }
    }

    #[test]
    fn page_retains_each_visible_line_once() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout(
            "<style>.front { position: relative; z-index: 2 }</style><p>first</p><p class='front'>front</p><p>last</p>",
            200.0,
            &mut document_resources,
        );
        let geometry = columns(200.0, 200.0, 1.0);
        let page = Paginator::new(
            (&document).render_view(&document_resources),
            geometry,
            PaginationMode::Book,
        )
        .compose(
            PageRequest::StartingAtOffset(0.0),
            Vec::new(),
            &mut PaginationScratch::default(),
        );
        let expected = (document.render_view(&document_resources).text()).paint_order()
            .iter()
            .copied()
            .filter(|&line| page.positions.contains(line))
            .collect::<Vec<_>>();

        assert_eq!(
            expected.len(),
            page.positions.iter().count(),
            "the retained traversal contains each visible line exactly once"
        );
    }

    #[test]
    fn page_retains_only_intersecting_block_decorations_in_css_order() {
        let content = (0..40)
            .map(|index| {
                format!("<p style='margin:0;height:20px;background:#{index:02x}0000'>row</p>")
            })
            .collect::<String>();
        let mut document_resources = GlyphResourceStore::default();
        let document = layout(&content, 200.0, &mut document_resources);
        let geometry = columns(200.0, 100.0, 1.0);
        let page = Paginator::new(
            (&document).render_view(&document_resources),
            geometry,
            PaginationMode::Book,
        )
        .compose(
            PageRequest::StartingAtOffset(0.0),
            Vec::new(),
            &mut PaginationScratch::default(),
        );
        let fragments = document.render_view(&document_resources).fragments();
        let expected = fragments.block_decorations()
            .iter()
            .filter(|decoration| decoration.rect().y0 < 100.0 && decoration.rect().y1 > 0.0)
            .map(|decoration| {
                (
                    decoration.rect(),
                    decoration.color(),
                )
            })
            .collect::<Vec<_>>();

        let actual = fragments.paint_order_for(&[], page.block_decoration_indices.as_deref().unwrap())
        .filter_map(|item| match item {
            html_pipeline::engine::RenderPaintItem::Decoration(decoration) => Some((
                decoration.rect(),
                decoration.color(),
            )),
            html_pipeline::engine::RenderPaintItem::Line(_) => None,
        })
        .collect::<Vec<_>>();

        assert_eq!(actual, expected);
        assert!(
            actual.len()
                < (fragments.block_decorations()).len(),
            "off-page decorations stay out of the retained page traversal"
        );
    }

    #[test]
    fn lines_that_do_not_fit_move_to_the_next_column() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout(PARAGRAPHS, 200.0, &mut document_resources);
        let placed = paginate(
            &document,
            &columns(200.0, 40.0, 2.0),
            PaginationMode::Book,
            &document_resources,
        );

        assert!(!placed.is_empty(), "something must be laid out");
        let columns_used: Vec<i32> = placed.iter().map(|(_, col, _)| *col).collect();
        assert!(
            columns_used.contains(&1),
            "a 40px column cannot hold six paragraphs, so later lines belong in column 1"
        );
        assert!(
            columns_used.windows(2).all(|w| w[0] <= w[1]),
            "columns must fill in order"
        );
    }

    #[test]
    fn every_placed_line_sits_within_its_column() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout(PARAGRAPHS, 200.0, &mut document_resources);
        let geometry = columns(200.0, 40.0, 2.0);
        for (line, _, y) in paginate(
            &document,
            &geometry,
            PaginationMode::Book,
            &document_resources,
        ) {
            assert!(y >= 0.0, "line {line:?} was placed above its column");
            assert!(
                y < geometry.size.height,
                "line {line:?} at y={y} overflows the {}px column",
                geometry.size.height
            );
        }
    }

    #[test]
    fn a_forced_break_before_moves_content_to_a_fresh_column() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout(
            "<p>aaa</p><p style='break-before: column'>bbb</p>",
            200.0,
            &mut document_resources,
        );
        let placed = paginate(
            &document,
            &columns(200.0, 200.0, 2.0),
            PaginationMode::Book,
            &document_resources,
        );

        assert_eq!(column_of(&placed, LineId::new(0)), Some(0));
        assert_eq!(
            column_of(&placed, LineId::new(1)),
            Some(1),
            "an authored column break must move the paragraph even though the column has room"
        );
    }

    #[test]
    fn a_semantic_chapter_starts_a_fresh_page_under_book_composition() {
        let source = "<p>aaa</p><section epub:type='chapter'><p>bbb</p></section>";
        let mut document_resources = GlyphResourceStore::default();
        let document = layout(source, 200.0, &mut document_resources);
        let layout = columns(200.0, 200.0, 2.0);

        let book = paginate(
            &document,
            &layout,
            PaginationMode::Book,
            &document_resources,
        );
        assert_eq!(column_of(&book, LineId::new(0)), Some(0));
        assert_ne!(
            column_of(&book, LineId::new(1)),
            Some(0),
            "a chapter section should not share a page with preceding content"
        );
    }

    #[test]
    fn web_composition_ignores_semantic_chapter_breaks() {
        let source = "<p>aaa</p><section epub:type='chapter'><p>bbb</p></section>";
        let mut document_resources = GlyphResourceStore::default();
        let document = layout(source, 200.0, &mut document_resources);
        let layout = columns(200.0, 200.0, 2.0);

        let web = paginate(&document, &layout, PaginationMode::Web, &document_resources);
        assert_eq!(column_of(&web, LineId::new(0)), Some(0));
        assert_eq!(
            column_of(&web, LineId::new(1)),
            Some(0),
            "web composition paginates on CSS alone, so the section stays in flow"
        );
    }

    #[test]
    fn book_composition_keeps_a_compact_list_item_whole() {
        // Four lines is the compact limit for a list item; the column below fits
        // three, so the whole item must move rather than split.
        let source = "<p>aaa</p><ul><li>one two three four five six seven eight</li></ul>";
        let mut document_resources = GlyphResourceStore::default();
        let document = layout(source, 60.0, &mut document_resources);
        let layout = columns(60.0, 60.0, 2.0);

        let book = paginate(
            &document,
            &layout,
            PaginationMode::Book,
            &document_resources,
        );
        let item_lines: Vec<i32> = book.iter().skip(1).map(|(_, col, _)| *col).collect();
        assert!(
            item_lines.len() > 1,
            "the fixture must wrap onto several lines or the test proves nothing: {book:?}"
        );
        assert!(
            item_lines.windows(2).all(|w| w[0] == w[1]),
            "a compact list item must not be split across columns: {book:?}"
        );
    }

    #[test]
    fn events_are_empty_under_web_composition() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout(
            "<table><tr><td>aaa</td></tr></table><section epub:type='chapter'><p>bbb</p></section>",
            200.0,
            &mut document_resources,
        );
        let layout = columns(200.0, 200.0, 2.0);
        let paginator = Paginator::new(
            (&document).render_view(&document_resources),
            layout,
            PaginationMode::Web,
        );
        let mut scratch = PaginationScratch::default();
        paginator.events_for_lines(
            0,
            document
                .render_view(&document_resources)
                .text()
                .line_count()
                .saturating_sub(1),
            &mut scratch,
        );

        assert!(
            scratch.events.tables.is_empty(),
            "table pagination is a book-composition rule"
        );
        assert!(
            scratch.events.keeps.is_empty(),
            "automatic keeps are a book-composition rule"
        );
    }

    #[test]
    fn pagination_is_deterministic_for_the_same_inputs() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout(PARAGRAPHS, 200.0, &mut document_resources);
        let layout = columns(200.0, 40.0, 2.0);
        assert_eq!(
            paginate(
                &document,
                &layout,
                PaginationMode::Book,
                &document_resources
            ),
            paginate(
                &document,
                &layout,
                PaginationMode::Book,
                &document_resources
            )
        );
    }

    #[test]
    fn a_later_start_offset_begins_at_a_later_line() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout(PARAGRAPHS, 200.0, &mut document_resources);
        let layout = columns(200.0, 40.0, 2.0);
        let paginator = Paginator::new(
            (&document).render_view(&document_resources),
            layout,
            PaginationMode::Book,
        );
        let first = paginator.compose(
            PageRequest::StartingAtOffset(0.0),
            Vec::new(),
            &mut PaginationScratch::default(),
        );
        let next = paginator.compose(
            PageRequest::StartingAtOffset(first.next_start_offset_y),
            Vec::new(),
            &mut PaginationScratch::default(),
        );
        assert!(
            next.start_line > first.start_line,
            "paging forward must advance: {} then {}",
            first.start_line,
            next.start_line
        );
    }

    #[test]
    fn an_empty_document_paginates_to_nothing() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout("", 200.0, &mut document_resources);
        let placed = paginate(
            &document,
            &columns(200.0, 200.0, 2.0),
            PaginationMode::Book,
            &document_resources,
        );
        assert!(placed.is_empty());
    }

    #[test]
    fn visible_pagination_work_does_not_grow_with_trailing_chapter_content() {
        let prefix = (0..20)
            .map(|index| format!("<p>visible {index}</p>"))
            .collect::<String>();
        let trailing = (0..2_000)
            .map(|index| format!("<p>trailing {index}</p>"))
            .collect::<String>();
        let mut short_resources = GlyphResourceStore::default();
        let short = layout(&prefix, 200.0, &mut short_resources);
        let mut long_resources = GlyphResourceStore::default();
        let long = layout(&format!("{prefix}{trailing}"), 200.0, &mut long_resources);
        let geometry = columns(200.0, 120.0, 1.0);

        let candidate_count = |document: &LaidOutDocument, resources: &GlyphResourceStore| {
            let paginator = Paginator::new(
                (document).render_view(resources),
                geometry,
                PaginationMode::Book,
            );
            let probe = paginator.build_forward_positions(0, 0.0, None, Vec::new(), &[], &[], &[], None, &mut super::SliceCache::default(), false);
            let mut structure = Structure::default();
            let anchors = structure.for_lines(
                paginator.doc,
                probe.start_line.saturating_sub(1)
                    ..probe
                        .last_line
                        .unwrap_or(probe.start_line)
                        .saturating_add(2),
            );
            anchors.blocks().count()
        };

        assert_eq!(
            candidate_count(&short, &short_resources),
            candidate_count(&long, &long_resources),
            "content after the viewport must not add pagination candidates"
        );
    }

    #[test]
    fn pagination_scratch_retains_hot_path_allocations() {
        let rows = (0..12)
            .map(|index| format!("<tr><td>body {index}</td></tr>"))
            .collect::<String>();
        let mut document_resources = GlyphResourceStore::default();
        let document = layout(
            &format!(
                "<h1>Heading</h1><table><thead><tr><th>header</th></tr></thead><tbody>{rows}</tbody></table>"
            ),
            120.0,
            &mut document_resources,
        );
        let geometry = columns(120.0, 80.0, 2.0);
        let paginator = Paginator::new(
            (&document).render_view(&document_resources),
            geometry,
            PaginationMode::BookWithVerticalRhythm,
        );
        let mut scratch = PaginationScratch::default();

        let _ = paginator.compose(PageRequest::StartingAtOffset(0.0), Vec::new(), &mut scratch);
        let capacities = [
            scratch.events.tables.capacity(),
            scratch.events.forced_breaks.capacity(),
            scratch.events.keeps.capacity(),
            scratch.probe_positions.capacity(),
            scratch.keep_context.lines_by_y.capacity(),
            scratch.range_lines.capacity(),
            scratch.vertical_rhythm.visible.capacity(),
            scratch.vertical_rhythm.column_lines.capacity(),
            scratch.vertical_rhythm.candidates.capacity(),
            scratch.vertical_rhythm.rigid_ranges.capacity(),
        ];

        let _ = paginator.compose(PageRequest::StartingAtOffset(0.0), Vec::new(), &mut scratch);
        let reused_capacities = [
            scratch.events.tables.capacity(),
            scratch.events.forced_breaks.capacity(),
            scratch.events.keeps.capacity(),
            scratch.probe_positions.capacity(),
            scratch.keep_context.lines_by_y.capacity(),
            scratch.range_lines.capacity(),
            scratch.vertical_rhythm.visible.capacity(),
            scratch.vertical_rhythm.column_lines.capacity(),
            scratch.vertical_rhythm.candidates.capacity(),
            scratch.vertical_rhythm.rigid_ranges.capacity(),
        ];

        assert_eq!(
            reused_capacities, capacities,
            "a repeated page build should reuse its warmed scratch buffers"
        );
        assert!(
            capacities.iter().any(|&capacity| capacity > 0),
            "the fixture must exercise allocated scratch storage"
        );
    }
}
