//! Per-view selection interaction and painting through host-supplied geometry.

use super::{
    Selection, SelectionAnchor, SelectionFlow, SelectionMode, SelectionState, TableHit, TableSelectionState,
};
use crate::export::{TableExportFormat, TableExportRequest, TextExportFormat};
use crate::geometry::{Geometry, SelectionHighlight, WordHit, WordHitPolicy};
use crate::layout::{BoxId, RenderView};
use crate::overlays::{OverlayPaintScratch, OverlayStyle, TextOverlay, paint_overlays};
use crate::render::{LinePlacement, PaintFragment, PaintProjection, Painter};
use crate::targets::LinkTarget;

use html_dom::DomNodeId;
use kurbo::{Point, Rect};
use peniko::Color;
use std::ops::Range;

/// The displayed document and its geometric projection. Hosts can restrict
/// pointer candidates to a column; painting uses all displayed placements.
pub struct SelectionLayout<'a, I> {
    pub view: RenderView<'a>,
    pub placements: I,
    pub projection: &'a dyn PaintProjection,
}

#[derive(Clone, Copy, Debug)]
pub enum SelectionUnit {
    Word,
    Element,
    TableCell,
}

#[derive(Clone, Copy, Debug)]
pub enum SelectionKind {
    Text,
    Table,
}

/// Platform-neutral input in the coordinates of the supplied placements.
/// Hosts retain navigation, clipboard, focus and scrolling policy.
pub enum SelectionInput {
    PointerLeave,
    PointerDown {
        position: Point,
        mode: SelectionMode,
        /// An existing table selection gets first refusal. A host handling an
        /// image or another action can suppress starting a text gesture.
        select_text: bool,
        /// A host-owned action, such as an annotation, takes precedence over
        /// immediate word selection. Moving the pointer still starts a drag.
        click_action: bool,
        clicks: usize,
    },
    PointerMove {
        position: Point,
        mode: SelectionMode,
        drag_distance: f64,
    },
    PointerUp {
        position: Point,
        mode: SelectionMode,
        drag_distance: f64,
    },
    SelectAt {
        position: Point,
        unit: SelectionUnit,
    },
    BeginHandle {
        start: bool,
    },
    Finish,
    Clear(SelectionKind),
    SetRange {
        range: Range<u32>,
        mode: SelectionMode,
    },
}

#[derive(Default)]
pub struct SelectionOutcome {
    pub over_link: bool,
    pub hover_changed: bool,
    pub handled: bool,
    pub changed: bool,
    /// Text selection completed or changed through a touch handle.
    /// The host can publish its source address without serializing text.
    pub finished: bool,
    /// Whether the host should keep routing pointer movement to this view.
    pub dragging: bool,
    /// A collapsed pointer gesture. The host decides whether it activates a link.
    pub click: Option<u32>,
}

/// Projected text-selection geometry for menus and drag handles.
#[derive(Default)]
pub struct SelectionGeometry {
    pub bounds: Option<Rect>,
    pub endpoints: (Option<Point>, Option<Point>),
}

#[derive(Default)]
pub struct SelectionHit {
    pub selected: bool,
    pub selected_table: bool,
}

pub enum SelectionExport {
    /// Plain text with authored paragraph breaks, for clipboard copying.
    PlainText,
    Markdown,
    TableAt {
        position: Point,
        format: TableExportFormat,
    },
    TableCellsMarkdown,
}

#[derive(Clone, Copy)]
enum Highlight {
    Text,
    SemanticCell,
    TableCell,
}

/// Selection and hover appearance for one view.
#[derive(Clone, Copy)]
pub struct SelectionPaintStyle {
    pub selection: Color,
    pub hover_foreground: Option<u32>,
}

/// Reusable buffers, independent of the lifetime of a document or a view.
#[derive(Default)]
pub struct SelectionPaintScratch {
    hover: OverlayPaintScratch,
    rects: Vec<(Highlight, Rect)>,
    cells: Vec<(Highlight, BoxId)>,
    fragments: Vec<PaintFragment>,
}

/// One view's text, table and touch selection. Documents can have several
/// independent controllers; no UI state is stored in the document itself.
#[derive(Clone, Default)]
pub struct SelectionController {
    text: SelectionState,
    table: TableSelectionState,
    touch: Option<SelectionAnchor>,
    hovered_node: Option<DomNodeId>,
    hovered_text: Option<Range<u32>>,
    dismissed_click: bool,
}

impl SelectionController {
    pub(crate) fn is_dragging(&self) -> bool {
        self.touch.is_some() || self.text.is_selecting() || self.table.is_dragging()
    }

    pub(crate) fn drag_flow(&self) -> Option<SelectionFlow> {
        self.is_dragging().then_some(self.text.flow).flatten()
    }

    pub fn link_hovered(&self) -> bool {
        self.hovered_node.is_some()
    }

    /// Resolve the hovered text link when the host needs to activate it.
    pub fn hovered_link<'a>(&self, view: RenderView<'a>) -> Option<LinkTarget<'a>> {
        view.addressing()
            .link_for_glyph(self.hovered_text.as_ref()?.start)
    }

    /// The selected text, for explicit copying and source addressing.
    pub fn selection<'a>(&self, view: RenderView<'a>) -> Option<Selection<'a>> {
        self.text.selection(view)
    }

    /// Consume one input. Placements stay lazy and are inspected only when the
    /// active interaction needs hit testing; idle pointer moves do no selection work.
    pub fn interact(
        &mut self,
        layout: SelectionLayout<'_, impl IntoIterator<Item = LinePlacement> + Clone>,
        input: SelectionInput,
    ) -> SelectionOutcome {
        let mut outcome = SelectionOutcome::default();
        let SelectionLayout {
            view,
            placements,
            projection,
        } = layout;
        let geometry = Geometry::new(view);
        let hover = match &input {
            SelectionInput::PointerMove { position, .. }
            | SelectionInput::PointerDown { position, .. }
            | SelectionInput::PointerUp { position, .. } => Some(Some(*position)),
            SelectionInput::PointerLeave => Some(None),
            _ => None,
        };
        if let Some(position) = hover {
            let node = position.and_then(|point| {
                // Link hover must respect paint clips; selection may still
                // resolve the nearest text outside the visible column.
                let mut projected = Vec::new();
                let visible: Vec<_> = placements.clone().into_iter().filter(|placement| {
                    let Some(line) = view.text().line(placement.line) else {
                        return false;
                    };
                    projected.clear();
                    projection.project_line(placement.line, line.point(), &mut projected);
                    projected.iter().any(|line| {
                        line.origin == placement.origin
                            && line.clip.is_none_or(|clip| clip.contains(point))
                    })
                }).collect();
                geometry.link_node_at_lines(point, visible.iter().copied())
            });
            outcome.hover_changed = node != self.hovered_node;
            if outcome.hover_changed {
                self.hovered_node = node;
                self.hovered_text = node.and_then(|node| {
                    (view.addressing()).link_text_range(node)
                });
            }
        }
        let hover_changed = outcome.hover_changed;
        let flow = if matches!(&input, SelectionInput::PointerMove { .. } | SelectionInput::PointerUp { .. })
            && (self.touch.is_some() || self.text.is_selecting())
        {
            self.text.flow
        } else {
            None
        };
        let word_at = |position, policy| {
            geometry.word_hit_at_lines(position, placements, policy, Some(projection), flow)
        };
        match input {
            SelectionInput::PointerLeave => {}
            SelectionInput::PointerDown {
                position,
                mode,
                select_text,
                click_action,
                clicks,
            } => {
                self.touch = None;
                self.dismissed_click = clicks == 1 && self.text.range().is_some();
                if self.dismissed_click {
                    self.text = SelectionState::default();
                    outcome.handled = true;
                    outcome.changed = true;
                }
                if !outcome.handled && self.table.is_active() {
                    if let Some(hit) = table_at(view, projection, position)
                        && self.table.begin_drag(hit)
                    {
                        outcome.handled = true;
                        outcome.changed = true;
                    }
                    if !outcome.handled {
                        outcome.changed = self.table.clear();
                    }
                }
                if !outcome.handled && select_text {
                    if let Some(mut hit) = word_at(position, WordHitPolicy::InkOnly) {
                        let flow = SelectionFlow::at(view, hit.word.start);
                        if clicks >= 2 {
                            if let Some(selection) = Selection::containing_element(view, hit.word.start) {
                                hit.word = selection.range();
                            }
                            self.text.begin_selected_word(hit, mode);
                        } else if click_action || self.hovered_node.is_some() {
                            // Actions retain click activation; dragging still selects words.
                            self.text.begin_at_word(hit, mode);
                        } else {
                            self.text.begin_selected_word(hit, mode);
                        }
                        self.text.flow = flow;
                        outcome.handled = true;
                        outcome.changed = true;
                    } else {
                        outcome.changed |= self.text.range().is_some() || self.text.is_selecting();
                        self.text = SelectionState::default();
                        outcome.handled |= outcome.changed;
                    }
                }
            }
            SelectionInput::PointerMove {
                position,
                mode,
                drag_distance,
            } => {
                if self.touch.is_some() {
                    outcome = self.extend_touch(word_at(position, WordHitPolicy::Nearest));
                } else if self.table.is_dragging() {
                    outcome.handled = true;
                    outcome.changed = self
                        .table
                        .update_drag(table_at(view, projection, position))
                        .unwrap_or(false);
                } else if self.text.is_selecting() {
                    outcome.changed = self.text.update_word_interaction(
                        word_at(position, WordHitPolicy::Nearest),
                        mode,
                        drag_distance,
                    );
                    outcome.handled = outcome.changed;
                }
            }
            SelectionInput::PointerUp {
                position,
                mode,
                drag_distance,
            } => {
                if std::mem::take(&mut self.dismissed_click) {
                    outcome.handled = true;
                } else if self.touch.is_some() {
                    outcome = self.extend_touch(word_at(position, WordHitPolicy::Nearest));
                    self.touch = None;
                    outcome.handled = self.text.range().is_some();
                    outcome.finished = outcome.handled;
                } else if self.table.is_dragging() {
                    outcome.changed = self
                        .table
                        .update_drag(table_at(view, projection, position))
                        .unwrap_or(false);
                    self.table.finish_drag();
                    outcome.handled = true;
                } else if self.text.is_selecting() {
                    outcome.changed = self.text.update_word_interaction(
                        word_at(position, WordHitPolicy::Nearest),
                        mode,
                        drag_distance,
                    );
                    outcome.click = self.text.finish_interaction();
                    outcome.finished = true;
                    outcome.handled = self.text.range().is_some();
                }
            }
            SelectionInput::SelectAt {
                position,
                unit: SelectionUnit::TableCell,
            } => {
                if let Some(hit) = table_at(view, projection, position) {
                    self.table.begin(hit);
                    self.touch = None;
                    outcome.handled = true;
                    outcome.changed = true;
                }
            }
            SelectionInput::SelectAt { position, unit } => {
                if let Some(hit) = word_at(position, WordHitPolicy::InkOnly) {
                    let flow = SelectionFlow::at(view, hit.word.start);
                    let range = match unit {
                        SelectionUnit::Word => Some(hit.word),
                        SelectionUnit::Element => {
                            Selection::containing_element(view, hit.word.start)
                                .map(|selection| selection.range())
                        }
                        SelectionUnit::TableCell => unreachable!(),
                    };
                    if let Some(range) = range {
                        self.touch = matches!(unit, SelectionUnit::Word)
                            .then(|| SelectionAnchor::new(range.clone()));
                        self.text
                            .select_range(range.start, range.end, SelectionMode::Plain);
                        self.text.flow = flow;
                        self.table.clear();
                        outcome.handled = true;
                        outcome.changed = true;
                        outcome.finished = true;
                    }
                }
            }
            SelectionInput::BeginHandle { start } => {
                if let Some(range) = self.text.range() {
                    let fixed = if start { range.end } else { range.start };
                    self.touch = Some(SelectionAnchor::new(fixed..fixed));
                    outcome.handled = true;
                }
            }
            SelectionInput::Finish => {
                self.dismissed_click = false;
                self.touch = None;
                if self.table.finish_drag() {
                    outcome.handled = true;
                } else if self.text.is_selecting() {
                    outcome.click = self.text.finish_interaction();
                    outcome.handled = self.text.range().is_some();
                    outcome.finished = true;
                }
            }
            SelectionInput::Clear(SelectionKind::Text) => {
                outcome.changed = self.text.range().is_some() || self.text.is_selecting();
                self.text = SelectionState::default();
                self.touch = None;
                outcome.handled = outcome.changed;
            }
            SelectionInput::Clear(SelectionKind::Table) => {
                outcome.changed = self.table.clear();
                outcome.handled = outcome.changed;
            }
            SelectionInput::SetRange { range, mode } => {
                outcome.changed = true;
                self.text.select_range(range.start, range.end, mode);
                self.touch = None;
                outcome.handled = true;
            }
        }
        outcome.hover_changed = hover_changed;
        outcome.over_link = self.link_hovered();
        outcome.changed |= hover_changed;
        outcome.dragging = self.is_dragging();
        outcome
    }

    /// Bounds and handle positions share the same displayed line traversal.
    pub(crate) fn geometry(
        &self,
        layout: SelectionLayout<'_, impl IntoIterator<Item = LinePlacement>>,
    ) -> SelectionGeometry {
        let Some(selection) = self.text.selection(layout.view) else {
            return SelectionGeometry::default();
        };
        let ranges = selection.selected_ranges();
        let geometry = Geometry::new(layout.view);
        let mut result = SelectionGeometry::default();
        for placement in layout.placements {
            for range in &ranges {
                for bounds in geometry.placed_selection_rects(placement, range.clone()) {
                    result.bounds = Some(
                        result
                            .bounds
                            .map_or(bounds, |previous| previous.union(bounds)),
                    );
                }
                let (start, end) = geometry.selection_endpoints_lines(range.clone(), [placement]);
                result.endpoints.0 = start.or(result.endpoints.0);
                result.endpoints.1 = end.or(result.endpoints.1);
            }
        }
        result.bounds = result
            .bounds
            .filter(|bounds| bounds.width() > 0.0 || bounds.height() > 0.0);
        result
    }

    pub(crate) fn hit_test(
        &self,
        layout: SelectionLayout<'_, impl IntoIterator<Item = LinePlacement>>,
        position: Point,
    ) -> SelectionHit {
        let SelectionLayout {
            view,
            placements,
            projection,
        } = layout;
        let mut fragments = Vec::new();
        let selected = self.text.selection(view).is_some_and(|selection| {
            Geometry::new(view).selection_contains_lines(
                &selection,
                position,
                placements,
                |index| box_contains(view, projection, index, position, &mut fragments),
            )
        });
        SelectionHit {
            selected,
            selected_table: self.table.is_active()
                && table_at(view, projection, position)
                    .is_some_and(|hit| self.table.is_in_selected_table(hit)),
        }
    }

    /// Copy is explicit and lazy. Markdown prefers a selected table over text.
    pub fn export(
        &self,
        layout: SelectionLayout<'_, impl IntoIterator<Item = LinePlacement>>,
        request: SelectionExport,
    ) -> Option<String> {
        let range = match request {
            SelectionExport::PlainText => {
                let selection = self.text.selection(layout.view)?;
                return selection.plain_text_with_format(TextExportFormat::PlainWithParagraphBreaks);
            }
            SelectionExport::Markdown => {
                if let Some(range) = self.table.range() {
                    range
                } else {
                    return self.text.selection(layout.view)?.markdown();
                }
            }
            SelectionExport::TableAt { position, format } => {
                let box_idx = table_at(layout.view, layout.projection, position)?.table_box();
                return (layout.view.boxes()).table(box_idx)
                    .and_then(|table| table.export(TableExportRequest::Whole(format)));
            }
            SelectionExport::TableCellsMarkdown => self.table.range()?,
        };
        (layout.view.boxes()).table(range.table_box())
            .and_then(|table| range.markdown(&table))
    }

    /// Paint text and cell selections using the same projection as the content.
    /// Reuse scratch storage between frames, including the link-hover underline.
    pub fn paint(
        &self,
        layout: SelectionLayout<'_, impl IntoIterator<Item = LinePlacement> + Clone>,
        style: SelectionPaintStyle,
        painter: &mut impl Painter,
        scratch: &mut SelectionPaintScratch,
    ) {
        if let Some(range) = &self.hovered_text {
            let rgba = style
                .hover_foreground
                .or_else(|| (layout.view.text()).paint_color_at(range.start))
                .unwrap_or(0x000000ff);
            paint_overlays(
                layout.view,
                std::iter::once(TextOverlay {
                    range: range.clone(),
                    style: OverlayStyle::Underline,
                    color: Color::rgba8(
                        (rgba >> 24) as u8,
                        (rgba >> 16) as u8,
                        (rgba >> 8) as u8,
                        rgba as u8,
                    ),
                }),
                layout.placements.clone(),
                painter,
                &mut scratch.hover,
            );
        }
        let color = style.selection;
        scratch.rects.clear();
        scratch.cells.clear();
        let selection = self
            .text
            .selection(layout.view)
            .map(|selection| SelectionHighlight {
                selection,
                text: Highlight::Text,
                cells: Highlight::SemanticCell,
            });
        Geometry::new(layout.view).collect_highlights(
            [],
            selection,
            self.table
                .range()
                .map(|range| (Highlight::TableCell, range)),
            layout.placements,
            &mut scratch.rects,
            &mut scratch.cells,
        );
        for &(highlight, bounds) in &scratch.rects {
            paint_highlight(painter, highlight, bounds, color);
        }
        for &(highlight, index) in &scratch.cells {
            scratch.fragments.clear();
            if let Some(bounds) = layout.view.boxes().bounds(index) {
                layout.projection.project_decoration(
                    Some(index),
                    bounds,
                    None,
                    &mut scratch.fragments,
                );
            }
            for fragment in &scratch.fragments {
                paint_highlight(
                    painter,
                    highlight,
                    Rect::from_origin_size(fragment.destination, fragment.source.size()),
                    color,
                );
            }
        }
    }
    fn extend_touch(&mut self, hit: Option<WordHit>) -> SelectionOutcome {
        let mut outcome = SelectionOutcome::default();
        if let Some(hit) = hit
            && let Some(anchor) = &self.touch
        {
            let range = anchor.extend_to(hit.word);
            outcome.changed = self.text.range().as_ref() != Some(&range);
            if outcome.changed {
                let flow = self.text.flow;
                self.text
                    .select_range(range.start, range.end, SelectionMode::Plain);
                self.text.flow = flow;
                self.table.clear();
                outcome.finished = true;
                outcome.handled = true;
            }
        }
        outcome
    }
}

fn box_contains(
    view: RenderView<'_>,
    projection: &dyn PaintProjection,
    index: BoxId,
    point: Point,
    fragments: &mut Vec<PaintFragment>,
) -> bool {
    fragments.clear();
    if let Some(bounds) = view.boxes().bounds(index) {
        projection.project_decoration(Some(index), bounds, None, fragments);
    }
    fragments.iter().any(|fragment| {
        Rect::from_origin_size(fragment.destination, fragment.source.size()).contains(point)
    })
}

fn table_at(
    view: RenderView<'_>,
    projection: &dyn PaintProjection,
    point: Point,
) -> Option<TableHit> {
    let mut fragments = Vec::new();
    Geometry::new(view)
        .table_at_where(|index| box_contains(view, projection, index, point, &mut fragments))
}

fn paint_highlight(painter: &mut impl Painter, highlight: Highlight, bounds: Rect, color: Color) {
    match highlight {
        Highlight::Text => painter.fill_rect(bounds, color),
        Highlight::SemanticCell => painter.fill_rect(bounds, Color::rgba8(72, 133, 237, 54)),
        Highlight::TableCell => {
            painter.fill_rect(bounds, Color::rgba8(72, 133, 237, 54));
            let width = 1.5;
            for border in [
                Rect::new(bounds.x0, bounds.y0, bounds.x1, bounds.y0 + width),
                Rect::new(bounds.x0, bounds.y1 - width, bounds.x1, bounds.y1),
                Rect::new(
                    bounds.x0,
                    bounds.y0 + width,
                    bounds.x0 + width,
                    bounds.y1 - width,
                ),
                Rect::new(
                    bounds.x1 - width,
                    bounds.y0 + width,
                    bounds.x1,
                    bounds.y1 - width,
                ),
            ] {
                painter.fill_rect(border, Color::rgba8(45, 102, 201, 210));
            }
        }
    }
}
