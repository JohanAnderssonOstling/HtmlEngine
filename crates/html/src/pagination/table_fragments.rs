//! Table and row fragments derived from the final page placements. Whole and
//! split rows share this path; painting never reconstructs their placement.

use super::PagePositions;
use crate::layout::{BoxId, RenderView};
use html_engine_model::layout_state;
use html_engine_model::page_placements::{self, BoxProjectionBand, TableFragmentPlacement};

pub(super) fn finish(doc: RenderView<'_>, page: &mut PagePositions) {
    page.table_fragments.clear();
    let boxes = doc.boxes();
    let ownership = boxes.pagination_ownership();
    for (id, screen) in page.positions.iter() {
        let Some(line) = doc.text().line(id) else { continue };
        let Some(membership) = ownership.lines.get(layout_state::line_id_index(id)) else { continue };
        if membership.table_fragments.is_empty() {
            continue;
        }
        let mut translation = screen.point - line.point();
        translation.x -= page_placements::col_x(&page.geometry, f64::from(screen.col_index), 0.0);
        let band = BoxProjectionBand {
            column: screen.col_index,
            translation,
            source_y: line.point().y,
            top: screen.visible_top() - translation.y,
            bottom: screen.visible_bottom(page.geometry.size.height) - translation.y,
            distance: 0,
        };
        if band.column < 0 || f64::from(band.column) >= page.geometry.col_count || band.bottom <= band.top {
            continue;
        }
        for &source in &membership.table_fragments {
            let placed = page.table_fragments.entry(source).or_insert_with(TableFragmentPlacement::default);
            if let Some(previous) = placed.bands.last_mut()
                && previous.column == band.column && previous.translation == band.translation
                && previous.top == band.top && previous.bottom == band.bottom
            {
                previous.source_y = previous.source_y.min(band.source_y);
            } else {
                placed.bands.push(band);
            }
        }
    }
    for (&id, placed) in &mut page.table_fragments {
        placed.bands = super::projection::resolve_box_bands(
            std::mem::take(&mut placed.bands),
            ownership.table_fragments[id as usize].bounds,
            page.geometry.col_count,
        );
    }
}

/// A table-owned box has no visible fragment when its row was moved to another
/// page. Returning an empty slice explicitly prevents ancestor-flow fallback
/// from leaving an empty grid at the row's former location.
pub(super) fn bands<'a>(
    doc: RenderView<'_>,
    page: &'a PagePositions,
    owner: BoxId,
) -> Option<&'a [BoxProjectionBand]> {
    let boxes = doc.boxes();
    let ownership = boxes.pagination_ownership();
    let source = ownership.boxes
        .get(layout_state::box_id_index(owner))?.table_fragment?;
    if let Some(placed) = page.table_fragments.get(&source) {
        return Some(placed.bands.as_slice());
    }
    if ownership.table_fragments[source as usize].line_count == 0 {
        // A genuinely empty row still has height and decoration. Inherit its
        // enclosing table fragment, unlike a nonempty row moved off this page.
        for ancestor in boxes.ancestors(owner) {
            let ancestor_source = ownership.boxes[layout_state::box_id_index(ancestor)].table_fragment;
            if let Some(ancestor_source) = ancestor_source
                && ancestor_source != source
            {
                return Some(page.table_fragments.get(&ancestor_source)
                    .map_or(&[], |placed| placed.bands.as_slice()));
            }
        }
        return None;
    }
    Some(&[])
}
