//! Merged OpenType settings shared by immutable font styles during shaping.
use html_style_model::{Font, OpenTypeFeature};
use rustc_data_structures::fx::FxHashMap;
use std::marker::PhantomData;
use std::ops::Range;

#[derive(Default)]
pub(super) struct FeatureCache<'font> {
    // Immutable fonts stay borrowed for the cache lifetime; keys are identity only.
    ranges: FxHashMap<*const Font, Range<usize>>,
    values: Vec<OpenTypeFeature>,
    fonts: PhantomData<&'font Font>,
}

impl<'font> FeatureCache<'font> {
    fn range(&mut self, font: &'font Font) -> Range<usize> {
        let key = font as *const Font;
        if let Some(range) = self.ranges.get(&key) {
            return range.clone();
        }
        let start = self.values.len();
        font.append_open_type_features(&mut self.values);
        let range = start..self.values.len();
        self.ranges.insert(key, range.clone());
        range
    }

    pub(super) fn get(&mut self, font: &'font Font) -> &[OpenTypeFeature] {
        let range = self.range(font);
        &self.values[range]
    }

    pub(super) fn equal(&mut self, left: &'font Font, right: &'font Font) -> bool {
        if std::ptr::eq(left, right) {
            return true;
        }
        let left = self.range(left);
        let right = self.range(right);
        self.values[left] == self.values[right]
    }
}
