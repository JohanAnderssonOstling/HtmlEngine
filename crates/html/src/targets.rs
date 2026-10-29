//! Document targets and annotation anchors. Navigation and publication identity
//! belong to the embedding application.

use crate::layout::{RenderView, SourceLocation};
use crate::resources::ResourceProvider;
use std::ops::Range;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextQuote {
    pub exact: String,
    pub prefix: Option<String>,
    pub suffix: Option<String>,
}

/// Source addresses survive reflow; the quote provides recovery after edits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextAnchor {
    pub start: SourceLocation,
    pub end: SourceLocation,
    pub quote: TextQuote,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinkTarget<'a> {
    pub href: &'a str,
    pub is_note_reference: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ElementTarget {
    pub glyphs: Option<Range<u32>>,
    pub y: Option<f64>,
    pub is_note: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedLink {
    pub resource: String,
    pub fragment: Option<String>,
}

pub fn resolve_link(provider: &dyn ResourceProvider, base: &str, href: &str) -> ResolvedLink {
    let (path, fragment) = href.split_once('#').unwrap_or((href, ""));
    ResolvedLink {
        resource: if path.is_empty() {
            base.to_owned()
        } else {
            provider.resolve(base, path)
        },
        fragment: (!fragment.is_empty()).then(|| fragment.to_owned()),
    }
}

impl crate::engine::Engine {
    /// Resolve a link through the document's resource provider. Whether to
    /// navigate, open a popup, or launch another application is a host decision.
    pub fn resolve_link(&self, href: &str) -> Option<ResolvedLink> {
        let base = &self.inputs()?.base_uri;
        Some(resolve_link(self.provider.as_ref(), base, href))
    }
}

#[derive(Clone, Copy)]
pub struct Targets<'a> {
    view: RenderView<'a>,
}

impl<'a> Targets<'a> {
    pub fn new(view: RenderView<'a>) -> Self {
        Self { view }
    }

    pub fn link(self, glyph: u32) -> Option<LinkTarget<'a>> {
        let addressing = self.view.addressing();
        let id = addressing.link_for_glyph(glyph)?;
        Some(LinkTarget {
            href: self.view.string(id),
            is_note_reference: addressing.is_note_reference(glyph),
        })
    }

    pub fn box_link(self, box_idx: usize) -> Option<LinkTarget<'a>> {
        let (href, is_note_reference) = self.view.addressing().link_for_box(box_idx)?;
        Some(LinkTarget {
            href,
            is_note_reference,
        })
    }

    pub fn element(self, id: &str) -> Option<ElementTarget> {
        let addressing = self.view.addressing();
        let key = self.view.lookup_string(id)?;
        let glyphs = addressing.glyph_range_for_anchor(id);
        let y = addressing
            .anchor_position(key)
            .map(|position| position.y())
            .or_else(|| {
                let glyph = addressing.anchor_glyph(key)?;
                let line = self.view.text().line_index_for_glyph(glyph)?;
                Some(self.view.text().line(line)?.point_y())
            });
        let is_note = addressing.is_note_target(id);
        (glyphs.is_some() || y.is_some() || is_note).then_some(ElementTarget { glyphs, y, is_note })
    }

    pub fn has_element(self, id: &str) -> bool {
        self.view.lookup_string(id).is_some_and(|key| {
            let addressing = self.view.addressing();
            addressing.anchor_glyph(key).is_some() || addressing.anchor_position(key).is_some()
        })
    }

    /// The nearest anchor at or before a logical character position.
    pub fn preceding_anchor(self, glyph: u32) -> Option<&'a str> {
        self.preceding_anchor_where(glyph, |_| true)
    }

    /// Restrict candidates without making the engine aware of a publication's
    /// table of contents or other application-specific anchor sets.
    pub fn preceding_anchor_where(
        self,
        glyph: u32,
        mut accepts: impl FnMut(&str) -> bool,
    ) -> Option<&'a str> {
        let mut best = None;
        for (key, position) in self.view.addressing().anchor_glyphs() {
            if position <= glyph
                && best.is_none_or(|(_, previous)| position > previous)
                && accepts(self.view.string(key))
            {
                best = Some((key, position));
            }
        }
        best.map(|(key, _)| self.view.string(key))
    }

    /// Find a preceding anchor in document coordinates. Anchors at the same
    /// height are ordered by source order, including empty elements.
    pub fn anchor_at_or_before_y(self, y: f64) -> Option<&'a str> {
        self.anchor_at_or_before_y_where(y, |_| true)
    }

    pub fn anchor_at_or_before_y_where(
        self,
        y: f64,
        mut accepts: impl FnMut(&str) -> bool,
    ) -> Option<&'a str> {
        if !y.is_finite() {
            return None;
        }
        let mut best = None;
        for (key, position) in self.view.addressing().anchor_positions().iter() {
            let order = (position.y(), position.order());
            if position.y() <= y
                && best.is_none_or(|(_, previous)| order > previous)
                && accepts(self.view.string(key))
            {
                best = Some((key, order));
            }
        }
        best.map(|(key, _)| self.view.string(key))
    }

    pub fn quote(self, range: Range<u32>, context: u32) -> Option<TextQuote> {
        let exact = self.view.export().plain_text(range.clone())?;
        let count = self.view.text().glyph_count().min(u32::MAX as usize) as u32;
        Some(TextQuote {
            exact,
            prefix: self
                .view
                .export()
                .plain_text(range.start.saturating_sub(context)..range.start),
            suffix: self
                .view
                .export()
                .plain_text(range.end..range.end.saturating_add(context).min(count)),
        })
    }

    pub fn source_location_on_line(
        self,
        line: usize,
        glyph: Option<u32>,
    ) -> Option<SourceLocation> {
        self.view.addressing().source_location_on_line(line, glyph)
    }

    pub fn source_element_y(self, source: &SourceLocation) -> Option<f64> {
        self.view.addressing().source_element_y(source)
    }

    /// Validate a host-decoded address against its quote before trusting it.
    /// Falls back to exact quoted text and context after source edits.
    pub fn resolve_range(
        self,
        preferred: Option<Range<u32>>,
        quote: &TextQuote,
    ) -> Option<Range<u32>> {
        if let Some(range) = preferred {
            if self.view.export().plain_text(range.clone()).as_deref() == Some(quote.exact.as_str())
            {
                return Some(range);
            }
        }
        self.resolve_quote(
            &quote.exact,
            quote.prefix.as_deref(),
            quote.suffix.as_deref(),
        )
    }

    pub fn resolve_quote(
        self,
        exact: &str,
        prefix: Option<&str>,
        suffix: Option<&str>,
    ) -> Option<Range<u32>> {
        self.view.text().locate_quote(exact, prefix, suffix)
    }

    pub fn capture(self, range: Range<u32>, context: u32) -> Option<TextAnchor> {
        let quote = self.quote(range.clone(), context)?;
        let location = |glyph, after| {
            self.view
                .addressing()
                .source_position_for_glyph(glyph, after)
                .map(SourceLocation::from)
        };
        Some(TextAnchor {
            start: location(range.start, false)?,
            end: location(range.end - 1, true)?,
            quote,
        })
    }

    pub fn resolve(self, anchor: &TextAnchor) -> Option<Range<u32>> {
        let position = |location: &SourceLocation, allow_end| {
            let (&text, elements) = location.path.split_last()?;
            self.view.addressing().resolve_source_position(
                elements,
                Some(text),
                location.utf16_offset,
                allow_end,
            )
        };
        let preferred = position(&anchor.start, false)
            .zip(position(&anchor.end, true))
            .map(|(start, end)| start..end);
        self.resolve_range(preferred, &anchor.quote)
    }
}
