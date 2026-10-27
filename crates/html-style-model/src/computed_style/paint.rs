//! Resolve reader paint independently of the computed typography tables.

use super::{
    ComputedStyles, ReaderStyleOverrides, StyleIndices, apply_reader_background,
    apply_reader_border, apply_reader_root_background,
};

/// Paint values for one retained publication style. These contain no font,
/// text-flow, box, or resource ownership and may change without invalidating
/// any of those results.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StylePaintColors {
    pub foreground: u32,
    pub background: u32,
    /// Top, right, bottom, and left in physical order.
    pub borders: [u32; 4],
    pub text_decoration: u32,
    pub outline: u32,
}

impl ComputedStyles {
    /// Resolve the paint part of a reader override without reinterning styles.
    /// Root and link context belong to the box/style binding, so identical
    /// publication styles can still resolve differently at those boundaries.
    pub fn reader_paint_colors(
        &self,
        indices: StyleIndices,
        overrides: &ReaderStyleOverrides,
        is_root: bool,
        is_link: bool,
    ) -> Option<StylePaintColors> {
        let style = self.view(indices)?;
        let foreground = overrides.foreground.unwrap_or_else(|| {
            overrides
                .theme
                .map_or(style.color(), |theme| theme.adapt(style.color()))
        });
        let mut background = style.background.clone();
        let mut border = style.border.clone();
        apply_reader_background(&mut background, overrides.theme);
        // Match the existing cascade contract: border currentColor is cached
        // before the link preference retargets inherited foreground colors.
        apply_reader_border(&mut border, foreground, overrides);
        if is_root {
            apply_reader_root_background(&mut background, overrides.background);
        }
        let foreground = if is_link {
            overrides.link_foreground.unwrap_or(foreground)
        } else {
            foreground
        };
        Some(StylePaintColors {
            foreground,
            background: if background.background_color_current_color {
                foreground
            } else {
                background.background_color
            },
            borders: [
                border.border_top_color,
                border.border_right_color,
                border.border_bottom_color,
                border.border_left_color,
            ],
            text_decoration: background.text_decoration.color.resolve(foreground),
            outline: background.outline.color.resolve(foreground),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::reintern_with_reader_overrides;
    use super::StylePaintColors;
    use crate::{
        Background, Border, BoxModel, ComputedStylesBuilder, DecorationColor, Font, InheritedText,
        LayoutStyle, ReaderStyleOverrides, ThemeAdaptation,
    };
    use html_dom::Document;

    #[test]
    fn paint_values_match_full_style_reinterning_for_alpha_and_current_color() {
        let document = Document::default();
        for authored in [0x123456ff, 0x12345680, 0x12345600] {
            for current_background in [false, true] {
                let mut builder = ComputedStylesBuilder::new(&document);
                let mut border = Border {
                    current_color_sides: 0b0101,
                    ..Default::default()
                };
                border.cache_current_color(authored);
                let mut background = Background {
                    background_color: 0xeeeeee80,
                    background_color_current_color: current_background,
                    ..Default::default()
                };
                background.text_decoration.color = DecorationColor::CurrentColor;
                background.outline.color = DecorationColor::Rgba(0x77889980);
                let indices = builder
                    .push_with_layout(
                        Font::default(),
                        InheritedText {
                            color: authored,
                            ..Default::default()
                        },
                        BoxModel::default(),
                        border,
                        background,
                        LayoutStyle::default(),
                    )
                    .unwrap();
                let source = builder.finish().unwrap();
                for theme in [
                    None,
                    Some(ThemeAdaptation {
                        ink: 0xddddddff,
                        surface: 0x111111ff,
                    }),
                ] {
                    for foreground in [None, Some(0xabcdef80)] {
                        for root in [false, true] {
                            let overrides = ReaderStyleOverrides {
                                theme,
                                foreground,
                                background: Some(0x112233ff),
                                ..Default::default()
                            };
                            let mut builder = ComputedStylesBuilder::new(&document);
                            let expected = reintern_with_reader_overrides(
                                &source,
                                &mut builder,
                                indices,
                                &overrides,
                                root,
                            )
                            .unwrap();
                            let reference = builder.finish().unwrap();
                            let style = reference.view(expected).unwrap();
                            let expected = StylePaintColors {
                                foreground: style.color(),
                                background: style.background_color(),
                                borders: [
                                    style.border_top_color(),
                                    style.border_right_color(),
                                    style.border_bottom_color(),
                                    style.border_left_color(),
                                ],
                                text_decoration: style.text_decoration_color(),
                                outline: style.outline_color(),
                            };
                            assert_eq!(
                                source.reader_paint_colors(indices, &overrides, root, false),
                                Some(expected)
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn link_foreground_and_dynamic_current_color_resolve_without_rebuilding_styles() {
        let document = Document::default();
        let mut builder = ComputedStylesBuilder::new(&document);
        let mut background = Background::default();
        background.background_color_current_color = true;
        background.text_decoration.color = DecorationColor::CurrentColor;
        background.outline.color = DecorationColor::CurrentColor;
        let indices = builder
            .push(
                Font::default(),
                InheritedText::default(),
                BoxModel::default(),
                Border::default(),
                background,
            )
            .unwrap();
        let styles = builder.finish().unwrap();
        let color = 0x3498dbff;
        let resolved = styles
            .reader_paint_colors(
                indices,
                &ReaderStyleOverrides {
                    link_foreground: Some(color),
                    ..Default::default()
                },
                false,
                true,
            )
            .unwrap();
        assert_eq!(resolved.foreground, color);
        assert_eq!(resolved.background, color);
        assert_eq!(resolved.text_decoration, color);
        assert_eq!(resolved.outline, color);
        assert_ne!(
            styles.view(indices).unwrap().color(),
            color,
            "publication styles remain unchanged"
        );
    }
}
