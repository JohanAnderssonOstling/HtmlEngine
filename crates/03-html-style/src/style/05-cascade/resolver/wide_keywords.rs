//! Computed longhand projection and CSS-wide values share catalogue ownership.
use super::context::CascadePhase;
use super::state::WorkingStyle;
use super::values::{default_justify_items, resolve_logical_text_alignments};
use crate::style::property_catalogue::{PropertyKey, property_catalogue};
use html_style_model::TextDirection;
use std::cell::LazyCell;
use std::slice;

macro_rules! finish_copy {
    (plain, $dst:ident, $src:ident, $preserve:ident) => {};
    (shorthand, $dst:ident, $src:ident, $preserve:ident) => {};
    (direction, $dst:ident, $src:ident, $preserve:ident) => {
        resolve_logical_text_alignments(&mut $dst.text)
    };
    (text_align, $dst:ident, $src:ident, $preserve:ident) => {
        if !$dst.text.text_align_last_explicit {
            $dst.text.text_align_last = $dst.text.text_align;
            $dst.text.text_align_last_logical = $dst.text.text_align_logical;
        }
    };
    (outline_width, $dst:ident, $src:ident, $preserve:ident) => {
        $dst.background
            .outline
            .set_width($src.background.outline.width())
    };
    (outline_offset, $dst:ident, $src:ident, $preserve:ident) => {
        if $src.background.outline.offset_is_inset() {
            $dst.background.outline.set_offset_inset()
        } else {
            $dst.background
                .outline
                .set_offset($src.background.outline.offset())
        }
    };
    (border_color($side:literal), $dst:ident, $src:ident, $preserve:ident) => {
        $dst.border.current_color_sides = ($dst.border.current_color_sides & !(1 << $side))
            | if $preserve {
                $src.border.current_color_sides & (1 << $side)
            } else {
                0
            };
    };
    (logical($ltr:ident, $rtl:ident), $dst:ident, $src:ident, $preserve:ident) => {
        copy_computed(
            $dst,
            $src,
            if $dst.text.direction == TextDirection::Ltr {
                PropertyKey::$ltr
            } else {
                PropertyKey::$rtl
            },
            $preserve,
        )
    };
}
macro_rules! define_copy {
    ($($key:ident => $name:literal, $inherited:literal, $phase:ident, [$($field:ident $(.$member:ident)*),* $(,)?], $copy:ident $(($($arg:tt)*))?;)+) => {
        pub(super) fn copy_computed(dst: &mut WorkingStyle, src: &WorkingStyle, key: PropertyKey, preserve_current_color: bool) {
            match key {
                $(PropertyKey::$key => {
                    $(dst.$field$(.$member)*.clone_from(&src.$field$(.$member)*);)*
                    finish_copy!($copy $(($($arg)*))?, dst, src, preserve_current_color);
                },)+
            }
        }
    };
}
property_catalogue!(define_copy);

pub(super) fn apply_css_wide_keyword_in_phase(
    style: &mut WorkingStyle,
    targets: &[PropertyKey],
    keyword: &str,
    parent: &WorkingStyle,
    root_font_size: f32,
    phase: CascadePhase,
) {
    let initial = LazyCell::new(|| {
        let mut initial = WorkingStyle::initial(root_font_size);
        initial.layout.justify_items = default_justify_items(parent.layout.justify_items);
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
            let source = match keyword {
                "inherit" => parent,
                "initial" => &*initial,
                "unset" if key.inherited() => parent,
                "unset" => &*initial,
                _ => continue,
            };
            copy_computed(style, source, key, keyword != "inherit");
        }
    }
}
