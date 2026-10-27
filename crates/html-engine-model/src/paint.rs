use html_style_model::{ComputedStyles, ReaderStyleOverrides, StyleIndices, StylePaintColors};
use rustc_data_structures::fx::FxHashMap;
use std::ops::Range;

pub const PAINT_ROOT: u8 = 1;
pub const PAINT_LINK: u8 = 2;
use PAINT_LINK as LINK;
use PAINT_ROOT as ROOT;

#[derive(Clone, Copy)]
pub struct PaintView<'a> {
    overrides: &'a ReaderStyleOverrides,
    contexts: &'a [u8],
    boxes: &'a [StylePaintColors],
    other_styles: &'a FxHashMap<StyleIndices, StylePaintColors>,
    linked_styles: &'a FxHashMap<StyleIndices, StylePaintColors>,
}

impl<'a> PaintView<'a> {
    pub fn new(
        overrides: &'a ReaderStyleOverrides,
        contexts: &'a [u8],
        boxes: &'a [StylePaintColors],
        other_styles: &'a FxHashMap<StyleIndices, StylePaintColors>,
        linked_styles: &'a FxHashMap<StyleIndices, StylePaintColors>,
    ) -> Self {
        Self {
            overrides,
            contexts,
            boxes,
            other_styles,
            linked_styles,
        }
    }

    pub fn is_default(&self) -> bool {
        *self.overrides == ReaderStyleOverrides::default()
    }
    pub fn box_colors(&self, index: usize) -> Option<&'a StylePaintColors> {
        self.boxes.get(index)
    }
    pub fn style_colors(
        &self,
        indices: StyleIndices,
        context: usize,
    ) -> Option<&'a StylePaintColors> {
        if self
            .contexts
            .get(context)
            .is_some_and(|flags| flags & LINK != 0)
        {
            self.linked_styles
                .get(&indices)
                .or_else(|| self.other_styles.get(&indices))
        } else {
            self.other_styles.get(&indices)
        }
    }
    pub fn resolve(
        &self,
        styles: &ComputedStyles,
        indices: StyleIndices,
        context_box: usize,
    ) -> StylePaintColors {
        let flags = self.contexts.get(context_box).copied().unwrap_or(0);
        if flags == 0
            && let Some(&colors) = self.other_styles.get(&indices)
        {
            return colors;
        }
        if flags == LINK
            && let Some(&colors) = self.linked_styles.get(&indices)
        {
            return colors;
        }
        styles
            .reader_paint_colors(
                indices,
                &self.overrides,
                flags & ROOT != 0,
                flags & LINK != 0,
            )
            .expect("validated paint binding")
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct TextPaintRange {
    pub glyphs: Range<u32>,
    pub style: StyleIndices,
}

#[derive(Clone)]
pub struct PaintColorRun {
    pub glyphs: Range<u32>,
    pub color: u32,
}
