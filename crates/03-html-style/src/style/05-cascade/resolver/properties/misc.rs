use super::super::context::{ApplyResult, PropertyContext};
use html_style_model::{BoxSizing, Hyphens, OverflowWrap, WordBreak};
use lightningcss::properties::Property;

pub(super) fn apply(context: &mut PropertyContext<'_>, property: &Property<'_>) -> ApplyResult {
    let style = &mut *context.style;
    match property {
        Property::Hyphens(value, _) => {
            use lightningcss::properties::text::Hyphens as CssHyphens;
            style.text.hyphens = match value {
                CssHyphens::None => Hyphens::None,
                CssHyphens::Manual => Hyphens::Manual,
                CssHyphens::Auto => Hyphens::Auto,
            };
        }
        Property::WordBreak(wb) => {
            use lightningcss::properties::text::WordBreak as LcWordBreak;
            style.text.word_break = match wb {
                LcWordBreak::Normal => WordBreak::Normal,
                LcWordBreak::BreakAll => WordBreak::BreakAll,
                LcWordBreak::KeepAll => WordBreak::KeepAll,
                LcWordBreak::BreakWord => WordBreak::BreakWord,
            };
        }
        Property::OverflowWrap(ow) | Property::WordWrap(ow) => {
            use lightningcss::properties::text::OverflowWrap as LcOverflowWrap;
            style.text.overflow_wrap = match ow {
                LcOverflowWrap::Normal => OverflowWrap::Normal,
                LcOverflowWrap::BreakWord => OverflowWrap::BreakWord,
                LcOverflowWrap::Anywhere => OverflowWrap::Anywhere,
            };
        }
        Property::BoxSizing(bs, _) => {
            use lightningcss::properties::size::BoxSizing as LcBoxSizing;
            style.box_model.box_sizing = match bs {
                LcBoxSizing::ContentBox => BoxSizing::ContentBox,
                LcBoxSizing::BorderBox => BoxSizing::BorderBox,
            };
        }

        _ => return ApplyResult::Unhandled,
    }
    ApplyResult::Applied
}
