//! Borrowed inputs for text token preparation, without mutable executor state.

use crate::layout::LayoutEngine;
use crate::layout::config::LayoutConfig;
use crate::layout::inline_reader::InlineReader;
use crate::layout::read_context::LayoutReader;
use html_engine_model::TextPaintRange;

#[derive(Clone, Copy)]
pub(in crate::layout) struct TextPreparationInputs<'a> {
    pub config: LayoutConfig,
    pub reader: LayoutReader<'a>,
    pub text: InlineReader<'a>,
    pub text_style_ranges: &'a [TextPaintRange],
}

impl<'a> LayoutEngine<'a, '_> {
    pub(in crate::layout) fn text_preparation_inputs(&self) -> TextPreparationInputs<'a> {
        TextPreparationInputs {
            config: self.config,
            reader: self.reader,
            text: self.text,
            text_style_ranges: self.text_style_ranges,
        }
    }
}
