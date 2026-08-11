use crate::layout::{FragmentWriter, LayoutEngine};
use crate::layout_model::{BoxGeometry, DecorationFragment, InlineContent, RoundedDecoration};
use kurbo::Rect;

/// Emits geometry-dependent decoration fragments while limiting mutation to
/// fragment output and its transient decoration-order owner keys.
pub(super) struct DecorationEmitter<'scope, 'input, 'output> {
    pub(super) reader: &'scope super::read_context::LayoutReader<'input>,
    pub(super) inline_content: &'input InlineContent,
    pub(super) geometry: &'scope BoxGeometry,
    pub(super) fragments: &'scope mut FragmentWriter<'output>,
    pub(super) track_overflow_clips: bool,
}

impl<'scope, 'input, 'output> DecorationEmitter<'scope, 'input, 'output> {
    fn new(reader: &'scope super::read_context::LayoutReader<'input>, inline_content: &'input InlineContent, geometry: &'scope BoxGeometry, fragments: &'scope mut FragmentWriter<'output>, track_overflow_clips: bool) -> Self {
        Self { reader, inline_content, geometry, fragments, track_overflow_clips }
    }
}

impl DecorationEmitter<'_, '_, '_> {
    pub(super) fn push_decoration(&mut self, rect: Rect, color: u32, is_inline: bool, rounded: Option<RoundedDecoration>) {
        if let Some(rounded) = rounded {
            self.fragments.push_rounded_decoration(rect, color, is_inline, rounded);
        } else {
            self.fragments.push_decoration(DecorationFragment::border_rect(rect, color, is_inline, false));
        }
    }

    pub(super) fn push_background(&mut self, rect: Rect, color: u32, is_inline: bool, rounded: Option<RoundedDecoration>) {
        if let Some(rounded) = rounded {
            self.fragments.push_rounded_background(rect, color, is_inline, rounded);
        } else {
            self.fragments.push_decoration(DecorationFragment::background_rect(rect, color, is_inline));
        }
    }
}

pub(crate) fn emit_block_background(engine: &mut LayoutEngine<'_, '_>, box_idx: usize) {
    with_decoration_emitter(engine, |emitter| emitter.emit_block_background(box_idx));
}

pub(crate) fn emit_block_decorations(engine: &mut LayoutEngine<'_, '_>, box_idx: usize) {
    with_decoration_emitter(engine, |emitter| emitter.emit_block_decorations(box_idx));
}

pub(crate) fn emit_block_border_and_outline(engine: &mut LayoutEngine<'_, '_>, box_idx: usize) {
    with_decoration_emitter(engine, |emitter| emitter.emit_block_border_and_outline(box_idx));
}

pub(crate) fn emit_color_rect_for_owner(engine: &mut LayoutEngine<'_, '_>, owner_box_idx: usize, rect: Rect, color: u32) {
    with_decoration_emitter(engine, |emitter| emitter.emit_color_rect_for_owner(owner_box_idx, rect, color));
}

pub(crate) fn collect_inline_decorations(engine: &mut LayoutEngine<'_, '_>) {
    with_decoration_emitter(engine, |emitter| emitter.collect_inline_decorations());
}

fn with_decoration_emitter<'input, R>(engine: &mut LayoutEngine<'input, '_>, operation: impl FnOnce(&mut DecorationEmitter<'_, 'input, '_>) -> R) -> R {
    let mut emitter = DecorationEmitter::new(&engine.reader, engine.text.content(), engine.geometry.as_ref(), &mut engine.fragments, engine.track_overflow_clips);
    operation(&mut emitter)
}

#[cfg(test)]
mod tests {
    use crate::LaidOutDocument;
    use crate::parser::DocumentFactory;
    use crate::test_support::TestGlyphShaper;

    fn layout_html(html: &str) -> LaidOutDocument {
        let mut factory = DocumentFactory::new();
        let mut glyphs = TestGlyphShaper::new();
        factory.parse_with_new_pipeline(html, None).shape(&mut glyphs).expect("test glyphs shape").layout(crate::LayoutConstraints::new(300.0, 16.0).unwrap())
    }

    fn layout_html_with_font_metrics(html: &str, x_height_ratio: f32, ascent_ratio: f32) -> LaidOutDocument {
        let mut factory = DocumentFactory::new();
        let mut glyphs = TestGlyphShaper::with_font_relative_metrics(x_height_ratio, ascent_ratio);
        factory.parse_with_new_pipeline(html, None).shape(&mut glyphs).expect("test glyphs shape").layout(crate::LayoutConstraints::new(300.0, 16.0).unwrap())
    }

    #[test]
    fn block_text_decoration_propagates_to_nested_inline_text() {
        let document = layout_html("<html><body><p style='text-decoration:underline 2px solid #123456'><span>Decorated</span></p></body></html>");
        let decorations = document.render_view().fragments().decorations().iter().filter(|fragment| fragment.color() == 0x123456FF).collect::<Vec<_>>();

        assert!(!decorations.is_empty(), "the block-established underline must paint beneath descendant text");
        assert!(decorations.iter().all(|fragment| fragment.is_inline()));
        assert!(decorations.iter().any(|fragment| (fragment.rect().height() - 2.0).abs() < 0.001));
    }

    #[test]
    fn table_text_decoration_propagates_through_rows_and_cells() {
        let document = layout_html("<html><body><table style='text-decoration:underline 2px solid #123456'><tr><td>Decorated</td></tr></table></body></html>");
        let decorations = document.render_view().fragments().decorations().iter().filter(|fragment| fragment.color() == 0x123456FF).collect::<Vec<_>>();

        assert!(!decorations.is_empty(), "a table-established decoration must reach text inside its cells");
        assert!(decorations.iter().all(|fragment| fragment.is_inline()));
    }

    #[test]
    fn line_through_keeps_dashed_style_as_one_semantic_span() {
        let document = layout_html("<html><body><span style='text-decoration:line-through 2px dashed #654321'>Long decorated text</span></body></html>");
        let decorations = document.render_view().fragments().decorations().iter().filter(|fragment| fragment.color() == 0x654321FF).collect::<Vec<_>>();

        assert_eq!(decorations.len(), 1, "layout should not tessellate a dashed line into painter rectangles");
        assert!(decorations.iter().all(|fragment| fragment.rect().height() > 0.0));
        assert!(decorations.iter().all(|fragment| fragment.pattern() == crate::RenderDecorationPattern::DashedHorizontal));
        assert!(decorations.iter().all(|fragment| fragment.is_foreground()), "line-through paints above glyphs");
    }

    #[test]
    fn overline_placement_uses_the_selected_fonts_ascent() {
        let html = "<html><body><span style='font-size:20px;text-decoration:overline 2px solid #123456'>Text</span></body></html>";
        let low_ascent = layout_html_with_font_metrics(html, 0.5, 0.5);
        let high_ascent = layout_html_with_font_metrics(html, 0.5, 0.7);
        let overline_y = |document: &LaidOutDocument| document.render_view().fragments().decorations().iter().find(|fragment| fragment.color() == 0x123456FF).expect("overline fragment").rect().y0;
        let baseline_y = |document: &LaidOutDocument| {
            let line = document.render_view().text().line(0).expect("decorated line");
            line.point().y + line.baseline()
        };

        let low_offset = baseline_y(&low_ascent) - overline_y(&low_ascent);
        let high_offset = baseline_y(&high_ascent) - overline_y(&high_ascent);
        assert!((high_offset - low_offset - 4.0).abs() < 0.001, "low-ascent offset={low_offset}, high-ascent offset={high_offset}");
    }

    #[test]
    fn line_through_placement_uses_half_the_selected_fonts_x_height() {
        let html = "<html><body><span style='font-size:20px;text-decoration:line-through 2px solid #654321'>Text</span></body></html>";
        let low_x_height = layout_html_with_font_metrics(html, 0.4, 0.7);
        let high_x_height = layout_html_with_font_metrics(html, 0.7, 0.7);
        let strike_y = |document: &LaidOutDocument| document.render_view().fragments().decorations().iter().find(|fragment| fragment.color() == 0x654321FF).expect("line-through fragment").rect().y0;

        assert!((strike_y(&low_x_height) - strike_y(&high_x_height) - 3.0).abs() < 0.001);
    }

    #[test]
    fn empty_end_fragment_after_block_descendant_paints_its_right_border() {
        let document = layout_html("<html><body><div style='width:200px'><span style='font-size:200px;line-height:200px;border-right:200px solid #008000;margin-right:-200px'><div></div></span></div></body></html>");
        let green = document.render_view().fragments().decorations().iter().find(|fragment| fragment.color() == 0x008000ff).expect("the trailing inline boundary must paint its right border");

        assert!((green.rect().width() - 200.0).abs() < 0.001, "right border width must survive its cancelling negative margin: {:?}", green.rect());
        assert!((green.rect().height() - 200.0).abs() < 0.001, "empty inline fragment must retain its authored line height: {:?}", green.rect());
    }

    #[test]
    fn vertical_margin_does_not_move_a_non_replaced_inline_border() {
        let plain = layout_html("<html><body><div><span style='border-top:20px solid #123456'>XXXXXXXXXX</span></div></body></html>");
        let margined = layout_html("<html><body><div><span style='border-top:20px solid #123456;margin-top:50px'>XXXXXXXXXX</span></div></body></html>");
        let border_rects = |document: &LaidOutDocument| document.render_view().fragments().decorations().iter().filter(|fragment| fragment.color() == 0x123456FF).map(|fragment| fragment.rect()).collect::<Vec<_>>();

        assert_eq!(border_rects(&plain), border_rects(&margined), "vertical margins do not apply to non-replaced inline boxes");
        assert_eq!(border_rects(&margined).len(), 1, "a single authored top border must paint even when the other border sides are none");
        assert!((border_rects(&margined)[0].height() - 20.0).abs() < 0.001);
    }

    #[test]
    fn overflow_hidden_resolves_padding_box_clips_for_descendant_paint() {
        let document = layout_html("<html><body><div style='height:40px;margin-top:40px;overflow:hidden'><div style='height:20px;margin-top:40px;background:#ff0000'></div></div></body></html>");
        let red = document.render_view().fragments().decorations().iter().find(|fragment| fragment.color() == 0xff0000ff).expect("red overflowing descendant background");
        let clip = red.overflow_clip().expect("overflow ancestor must publish a clip");
        assert!(red.rect().y0 >= clip.rect().y1 - 0.001, "the descendant starts outside the parent's padding box: rect={:?}, clip={clip:?}", red.rect());
    }

    #[test]
    fn visible_overflow_keeps_the_sparse_clip_fast_path() {
        let document = layout_html("<html><body><div><span style='background:#ff0000'>visible text</span></div></body></html>");
        let root = document.render_view();
        assert!(root.fragments().decorations().iter().all(|fragment| fragment.overflow_clip().is_none()));
        let text = root.text();
        assert!((0..text.line_count()).all(|line| text.line_overflow_clip(line).is_none()));
    }

    #[test]
    fn mixed_overflow_axes_do_not_clip_the_visible_axis() {
        let document = layout_html("<html><body><div style='width:20px;height:10px;overflow-x:clip;overflow-y:visible'><div style='width:40px;height:20px;background:#ff0000'></div></div></body></html>");
        let red = document.render_view().fragments().decorations().iter().find(|fragment| fragment.color() == 0xff0000ff).expect("red descendant background");
        let clip = red.overflow_clip().expect("horizontal overflow clip");
        assert!(clip.clips_x());
        assert!(!clip.clips_y(), "overflow-y:visible must remain paintable outside the parent height");
    }

    #[test]
    fn solid_outline_paints_outside_layout_without_changing_geometry() {
        let plain = layout_html("<html><body><div style='width:50px;height:20px'>A</div></body></html>");
        let outlined = layout_html("<html><body><div style='width:50px;height:20px;outline:2px solid #abcdef'>A</div></body></html>");
        let outline_fragments = outlined.render_view().fragments().decorations().iter().filter(|fragment| fragment.color() == 0xABCDEFFF).collect::<Vec<_>>();

        assert_eq!(plain.render_view().text().lines().get(0).unwrap().point(), outlined.render_view().text().lines().get(0).unwrap().point());
        assert_eq!(plain.render_view().text().lines().get(0).unwrap().height(), outlined.render_view().text().lines().get(0).unwrap().height());
        assert_eq!(outline_fragments.len(), 4, "a solid rectangular outline has four paint edges");
        assert!(outline_fragments.iter().all(|fragment| !fragment.is_inline()));
        assert!(outline_fragments.iter().all(|fragment| fragment.is_foreground()), "outlines paint above box contents");
    }

    #[test]
    fn outline_offset_moves_block_and_inline_outline_without_affecting_layout() {
        let block_zero = layout_html("<html><body><div style='width:50px;height:20px;outline:2px solid #abcdef'>A</div></body></html>");
        let block_offset = layout_html("<html><body><div style='width:50px;height:20px;outline:2px solid #abcdef;outline-offset:5px'>A</div></body></html>");
        let bounds = |document: &LaidOutDocument| {
            document
                .render_view()
                .fragments()
                .decorations()
                .iter()
                .filter(|fragment| fragment.color() == 0xABCDEFFF)
                .map(|fragment| fragment.rect())
                .reduce(|a, b| a.union(b))
                .expect("outline fragments")
        };
        assert_eq!(block_zero.render_view().text().lines().get(0).unwrap().point(), block_offset.render_view().text().lines().get(0).unwrap().point());
        let zero = bounds(&block_zero);
        let positive = bounds(&block_offset);
        assert_eq!(positive.x0, zero.x0 - 5.0);
        assert_eq!(positive.x1, zero.x1 + 5.0);

        let inline_zero = layout_html("<html><body><span style='outline:2px solid #123456'>text</span></body></html>");
        let inline_inset = layout_html("<html><body><span style='outline:2px solid #123456;outline-offset:-1px'>text</span></body></html>");
        let inline_bounds = |document: &LaidOutDocument| {
            document
                .render_view()
                .fragments()
                .decorations()
                .iter()
                .filter(|fragment| fragment.color() == 0x123456FF)
                .map(|fragment| fragment.rect())
                .reduce(|a, b| a.union(b))
                .expect("inline outline fragments")
        };
        let zero = inline_bounds(&inline_zero);
        let inset = inline_bounds(&inline_inset);
        assert_eq!(inset.x0, zero.x0 + 1.0);
        assert_eq!(inset.x1, zero.x1 - 1.0);
        assert_eq!(inline_zero.render_view().text().lines().get(0).unwrap().height(), inline_inset.render_view().text().lines().get(0).unwrap().height());

        let keyword = layout_html("<html><body><div style='width:50px;height:20px;outline:2px solid #abcdef;outline-offset:inset'>A</div></body></html>");
        let numeric = layout_html("<html><body><div style='width:50px;height:20px;outline:2px solid #abcdef;outline-offset:-2px'>A</div></body></html>");
        assert_eq!(bounds(&keyword), bounds(&numeric), "inset is the negative used outline width");
    }

    #[test]
    fn hidden_boxes_keep_layout_but_emit_no_owned_decorations() {
        let visible = layout_html("<html><body><div style='width:50px;height:20px;background:#123456;border:2px solid #abcdef;outline:1px solid #fedcba'>A</div></body></html>");
        let hidden = layout_html("<html><body><div style='visibility:hidden;width:50px;height:20px;background:#123456;border:2px solid #abcdef;outline:1px solid #fedcba'>A</div></body></html>");
        assert_eq!(visible.render_view().text().lines().get(0).unwrap().point(), hidden.render_view().text().lines().get(0).unwrap().point());
        assert_eq!(visible.render_view().text().lines().get(0).unwrap().height(), hidden.render_view().text().lines().get(0).unwrap().height());
        assert!(!hidden.render_view().fragments().decorations().iter().any(|fragment| matches!(fragment.color(), 0x123456FF | 0xABCDEFFF | 0xFEDCBAFF)));
    }

    #[test]
    fn dotted_outline_is_four_semantic_foreground_edges() {
        let document = layout_html("<html><body><div style='width:80px;height:20px;outline:2px dotted #13579b'>A</div></body></html>");
        let fragments = document.render_view().fragments().decorations().iter().filter(|fragment| fragment.color() == 0x13579BFF).collect::<Vec<_>>();

        assert_eq!(fragments.len(), 4);
        assert!(fragments.iter().all(|fragment| fragment.is_foreground()));
        assert_eq!(fragments.iter().filter(|fragment| fragment.pattern() == crate::RenderDecorationPattern::DottedHorizontal).count(), 2);
        assert_eq!(fragments.iter().filter(|fragment| fragment.pattern() == crate::RenderDecorationPattern::DottedVertical).count(), 2);
    }

    #[test]
    fn combined_double_lines_preserve_paint_order_and_current_color() {
        let document = layout_html("<html><body><span style='color:#2468ac;text-decoration-line:underline overline line-through;text-decoration-style:double;text-decoration-thickness:2px'>Decorated</span></body></html>");
        let fragments = document.render_view().fragments().decorations().iter().filter(|fragment| fragment.color() == 0x2468ACFF).collect::<Vec<_>>();
        let background_count = fragments.iter().filter(|fragment| !fragment.is_foreground()).count();
        let foreground_count = fragments.iter().filter(|fragment| fragment.is_foreground()).count();

        assert_eq!(background_count, 2, "underline and overline remain semantic double-line spans");
        assert_eq!(foreground_count, 1, "line-through remains one foreground semantic span");
        assert!(fragments.iter().all(|fragment| fragment.pattern() == crate::RenderDecorationPattern::DoubleHorizontal));
        assert!(fragments.iter().all(|fragment| (fragment.rect().height() - 6.0).abs() < 0.001));
    }

    #[test]
    fn descendant_none_does_not_cancel_an_ancestor_text_decoration() {
        let document = layout_html("<html><body><p style='text-decoration:underline 2px solid #112233'>Before <span style='text-decoration:none;color:#abcdef'>inside</span> after</p></body></html>");
        let fragments = document.render_view().fragments().decorations();

        assert!(fragments.iter().any(|fragment| fragment.color() == 0x112233FF), "an ancestor-established decoration propagates through descendant text");
        assert!(!fragments.iter().any(|fragment| fragment.color() == 0xABCDEFFF), "text color alone must not establish a descendant decoration");
    }

    #[test]
    fn inline_outline_uses_current_color_and_stays_outside_layout() {
        let plain = layout_html("<html><body><span>Outlined text</span></body></html>");
        let outlined = layout_html("<html><body><span style='color:#3579bd;outline:3px dashed currentColor'>Outlined text</span></body></html>");
        let fragments = outlined.render_view().fragments().decorations().iter().filter(|fragment| fragment.color() == 0x3579BDFF).collect::<Vec<_>>();

        assert_eq!(plain.render_view().text().lines().get(0).unwrap().point(), outlined.render_view().text().lines().get(0).unwrap().point());
        assert_eq!(plain.render_view().text().lines().get(0).unwrap().height(), outlined.render_view().text().lines().get(0).unwrap().height());
        assert_eq!(fragments.len(), 4, "layout publishes four semantic outline edges");
        assert!(fragments.iter().all(|fragment| matches!(fragment.pattern(), crate::RenderDecorationPattern::DashedHorizontal | crate::RenderDecorationPattern::DashedVertical)));
        assert!(fragments.iter().all(|fragment| fragment.is_inline() && fragment.is_foreground()));
    }

    #[test]
    fn transparent_or_zero_thickness_paint_values_emit_no_fragments() {
        let document = layout_html("<html><body><span style='color:transparent;text-decoration:underline;outline:2px solid'>transparent</span><span style='text-decoration:underline 0 solid red'>zero</span></body></html>");

        assert!(!document.render_view().fragments().decorations().iter().any(|fragment| fragment.color() == 0xFF0000FF));
        assert!(!document.render_view().fragments().decorations().iter().any(|fragment| fragment.color() & 0xFF == 0));
    }

    #[test]
    fn block_background_uses_the_stored_border_box_not_the_margin_box() {
        let document = layout_html("<html><body><div style='width:50px;height:20px;margin:11px 13px 17px 19px;background:#123456'></div></body></html>");
        let fragment = document.render_view().fragments().decorations().iter().find(|fragment| fragment.color() == 0x123456FF).expect("block background should be emitted");

        assert_eq!(fragment.rect().width(), 50.0);
        assert_eq!(fragment.rect().height(), 20.0);
    }

    #[test]
    fn empty_block_border_edges_remain_line_independent() {
        let document = layout_html("<html><body><p>lead</p><div style='border-color:blue;border-style:solid;border-width:10px;height:96px'></div></body></html>");
        let blue = document.render_view().fragments().decorations().iter().filter(|fragment| fragment.color() == 0x0000ffff).collect::<Vec<_>>();

        assert_eq!(blue.len(), 4, "a uniform block border must publish four physical edges");
        assert!(blue.iter().all(|fragment| fragment.line_idx().is_none()), "block edges must not be consumed as decorations of the preceding text line: {blue:?}");
    }

    #[test]
    fn root_or_body_background_is_propagated_once_to_the_canvas() {
        let root = layout_html("<html style='background:#123456'><body style='background:#abcdef'></body></html>");
        assert_eq!(root.render_view().canvas_background_color(), Some(0x123456FF));
        assert!(!root.render_view().fragments().decorations().iter().any(|fragment| fragment.color() == 0x123456FF), "the propagated root background must not also paint as an element box");
        assert!(root.render_view().fragments().decorations().iter().any(|fragment| fragment.color() == 0xABCDEFFF), "an unpropagated body background still paints its element box");

        let body = layout_html("<html><body style='background:#2468ac'></body></html>");
        assert_eq!(body.render_view().canvas_background_color(), Some(0x2468ACFF));
        assert!(!body.render_view().fragments().decorations().iter().any(|fragment| fragment.color() == 0x2468ACFF), "the propagated body background must not also paint as an element box");

        let image_root = layout_html("<html style=\"background:url('missing.png')\"><body style='background:#13579b'></body></html>");
        assert_eq!(image_root.render_view().canvas_background_color(), None, "a root image layer suppresses body propagation even before image painting is implemented");

        let contents_body = layout_html("<html><body style='display:contents;background:#2468ac'><div></div></body></html>");
        assert_eq!(contents_body.render_view().canvas_background_color(), None, "a body without a principal box cannot propagate its background to the canvas");
        assert!(image_root.render_view().fragments().decorations().iter().any(|fragment| fragment.color() == 0x13579BFF));

        let hidden_root = layout_html("<html style='visibility:hidden;background:#123456'><body></body></html>");
        assert_eq!(hidden_root.render_view().canvas_background_color(), None, "the propagated root background still obeys visibility");
    }

    #[test]
    fn negative_margin_does_not_expand_a_block_decoration() {
        let document = layout_html("<html><body><div style='width:40px;height:6px;margin-right:-90px;margin-bottom:-70px;background:#234567'></div></body></html>");
        let fragment = document.render_view().fragments().decorations().iter().find(|fragment| fragment.color() == 0x234567FF).expect("block background should be emitted");

        assert_eq!(fragment.rect().width(), 40.0);
        assert_eq!(fragment.rect().height(), 6.0);
    }

    #[test]
    fn ancestor_block_decorations_paint_before_descendant_decorations() {
        let document = layout_html("<html><body><div style='width:50px;height:20px;background:#345678'><div style='width:20px;height:10px;background:#456789'></div></div></body></html>");
        let colors = document.render_view().fragments().decorations().iter().map(|fragment| fragment.color()).collect::<Vec<_>>();
        let ancestor = colors.iter().position(|color| *color == 0x345678FF).expect("ancestor decoration");
        let descendant = colors.iter().position(|color| *color == 0x456789FF).expect("descendant decoration");

        assert!(ancestor < descendant, "ancestor background must paint below descendant content");
    }

    #[test]
    fn adjoining_empty_block_borders_match_equivalent_background_boxes() {
        let bordered = layout_html("<html><body><p>same lead-in</p><div style='border-top:5px solid blue'></div><div style='border-bottom:5px solid orange'></div></body></html>");
        let backgrounds = layout_html("<html><body><p>same lead-in</p><div style='height:5px;background:blue'></div><div style='height:5px;background:orange'></div></body></html>");
        let colored_rects =
            |document: &LaidOutDocument| document.render_view().fragments().decorations().iter().filter(|fragment| matches!(fragment.color(), 0x0000FFFF | 0xFFA500FF)).map(|fragment| (fragment.color(), fragment.rect())).collect::<Vec<_>>();

        assert_eq!(colored_rects(&bordered), colored_rects(&backgrounds));
    }

    #[test]
    fn inherited_padding_and_negative_margin_align_overpainting_borders() {
        let document = layout_html(
            "<html><body><p>same lead-in</p><div style='padding-top:96px'><div style='border-top:96px solid red;padding-top:inherit'></div><div style='border-bottom:96px solid black;margin-top:-192px'></div></div></body></html>",
        );
        let reference = layout_html("<html><body><p>same lead-in</p><div style='border-top:96px solid black;margin-top:112px'></div></body></html>");
        let red = document.render_view().fragments().decorations().iter().find(|fragment| fragment.color() == 0xFF0000FF).expect("red border").rect();
        let black = document.render_view().fragments().decorations().iter().find(|fragment| fragment.color() == 0x000000FF).expect("black border").rect();
        let reference_black = reference.render_view().fragments().decorations().iter().find(|fragment| fragment.color() == 0x000000FF).expect("reference black border").rect();

        assert_eq!(red, black, "the later black border must exactly overpaint the red border");
        assert_eq!(black, reference_black, "the source and reference border geometry must agree");
    }
}
