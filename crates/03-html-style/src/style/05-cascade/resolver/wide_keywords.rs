//! Computed longhand projection and CSS-wide values share catalogue ownership.
use super::context::CascadePhase;
use super::state::{ParentStyle, StyleRef, WorkingStyle};
use super::values::{default_justify_items, resolve_logical_text_alignments};
use crate::style::property_catalogue::{PropertyKey, property_catalogue};
use crate::style::syntax::wide_keywords::CssWideKeyword;
use html_style_model::{ComputedStylesBuilder, TextDirection};
use std::cell::LazyCell;
use std::slice;

macro_rules! finish_copy {
    (display, $dst:ident, $src:ident, $preserve:ident, $styles:ident) => {
        $dst.layout.static_position_inline = false
    };
    (plain, $dst:ident, $src:ident, $preserve:ident, $styles:ident) => {};
    (shorthand, $dst:ident, $src:ident, $preserve:ident, $styles:ident) => {};
    (white_space_collapse, $dst:ident, $src:ident, $preserve:ident, $styles:ident) => {
        $dst.text_mut($styles).white_space = $src
            .text
            .white_space
            .with_wrapping($dst.text($styles).white_space.allows_wrap())
    };
    (text_wrap_mode, $dst:ident, $src:ident, $preserve:ident, $styles:ident) => {
        $dst.text_mut($styles).white_space = $dst
            .text($styles)
            .white_space
            .with_wrapping($src.text.white_space.allows_wrap())
    };
    (direction, $dst:ident, $src:ident, $preserve:ident, $styles:ident) => {
        resolve_logical_text_alignments($dst.text_mut($styles))
    };
    (text_align, $dst:ident, $src:ident, $preserve:ident, $styles:ident) => {
        resolve_logical_text_alignments($dst.text_mut($styles))
    };
    (outline_width, $dst:ident, $src:ident, $preserve:ident, $styles:ident) => {
        $dst.background
            .outline
            .set_width($src.background.outline.width())
    };
    (outline_offset, $dst:ident, $src:ident, $preserve:ident, $styles:ident) => {
        if $src.background.outline.offset_is_inset() {
            $dst.background.outline.set_offset_inset()
        } else {
            $dst.background
                .outline
                .set_offset($src.background.outline.offset())
        }
    };
    (border_color($side:literal), $dst:ident, $src:ident, $preserve:ident, $styles:ident) => {
        $dst.border.current_color_sides = ($dst.border.current_color_sides & !(1 << $side))
            | if $preserve {
                $src.border.current_color_sides & (1 << $side)
            } else {
                0
            };
    };
    (logical($ltr:ident, $rtl:ident), $dst:ident, $src:ident, $preserve:ident, $styles:ident) => {{
        let key = if $dst.text($styles).direction == TextDirection::Ltr {
            PropertyKey::$ltr
        } else {
            PropertyKey::$rtl
        };
        copy_view($dst, $src, key, $preserve, $styles)
    }};
}
macro_rules! working_field {
    ($dst:ident, font, $styles:ident) => {
        $dst.font_mut($styles)
    };
    ($dst:ident, text, $styles:ident) => {
        $dst.text_mut($styles)
    };
    ($dst:ident, $field:ident, $styles:ident) => {
        (&mut $dst.$field)
    };
}

macro_rules! define_copy {
    ($($key:ident => $name:literal, $inherited:literal, $phase:ident, [$($field:ident $(.$member:ident)*),* $(,)?], $copy:ident $(($($arg:tt)*))?, [$($renderer:ident => $renderer_name:literal $(=> $native:ident)?),*];)+) => {
        pub(super) fn copy_computed(dst: &mut WorkingStyle, src: &WorkingStyle, key: PropertyKey, preserve_current_color: bool, styles: &ComputedStylesBuilder) {
            copy_view(dst, src.view(styles), key, preserve_current_color, styles);
        }

        fn copy_view(dst: &mut WorkingStyle, src: StyleRef<'_>, key: PropertyKey, preserve_current_color: bool, styles: &ComputedStylesBuilder) {
            match key {
                $(PropertyKey::$key => {
                    $(working_field!(dst, $field, styles)$(.$member)*.clone_from(&src.$field$(.$member)*);)*
                    finish_copy!($copy $(($($arg)*))?, dst, src, preserve_current_color, styles);
                },)+
            }
        }
    };
}
property_catalogue!(define_copy);

pub(super) fn apply_css_wide_keyword_in_phase(
    style: &mut WorkingStyle,
    targets: &[PropertyKey],
    keyword: CssWideKeyword,
    parent: &ParentStyle,
    styles: &ComputedStylesBuilder,
    root_font_size: f32,
    phase: CascadePhase,
) {
    let initial = LazyCell::new(|| {
        let mut initial = WorkingStyle::initial(root_font_size);
        initial.layout.justify_items = parent.with(styles, |parent| {
            default_justify_items(parent.layout.justify_items)
        });
        initial
    });
    for target in targets {
        let keys = if *target == PropertyKey::All {
            PropertyKey::ALL
        } else {
            slice::from_ref(target)
        };
        for &key in keys {
            if !key.is_longhand()
                || key.phase() != phase
                || (*target == PropertyKey::All && !key.affected_by_all())
            {
                continue;
            }
            match keyword {
                CssWideKeyword::Inherit => parent.with(styles, |parent| {
                    copy_view(style, parent, key, false, styles)
                }),
                CssWideKeyword::Unset if key.inherited() => {
                    parent.with(styles, |parent| copy_view(style, parent, key, true, styles))
                }
                CssWideKeyword::Initial | CssWideKeyword::Unset => {
                    copy_computed(style, &initial, key, true, styles)
                }
                _ => {}
            }
        }
    }
}
