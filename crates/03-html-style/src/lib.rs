#[cfg(test)]
pub(crate) mod document {
    pub use html_dom::*;
    pub use html_style_model::*;
}

#[cfg(test)]
pub(crate) mod parser {
    pub(crate) struct DocumentFactory;

    impl DocumentFactory {
        pub(crate) fn new() -> Self {
            Self
        }

        pub(crate) fn parse_with_new_pipeline(
            &mut self,
            html: &str,
            css: Option<&str>,
        ) -> crate::StyledDocument {
            match css {
                Some(css) => self.parse_with_new_pipeline_css_chunks(html, &[css]),
                None => self.parse_with_new_pipeline_css_chunks(html, &[]),
            }
        }

        pub(crate) fn parse_with_new_pipeline_css_chunks(
            &mut self,
            html: &str,
            css: &[&str],
        ) -> crate::StyledDocument {
            let standards_html = if html
                .trim_start()
                .to_ascii_lowercase()
                .starts_with("<!doctype html")
            {
                html.to_owned()
            } else {
                format!("<!doctype html>{html}")
            };
            let document = html_parse::parse_dom_document(&standards_html)
                .expect("parser produced a valid standards-mode DOM");
            crate::style_document(document, css)
        }
    }
}

#[path = "style/mod.rs"]
mod style;

use html_dom::Document;

pub use style::DEFAULT_CSS;
pub use style::document::{AuthorStylesheetInput, StyledDocument};
pub use style::timings::StyleTimings;
pub use style::source::fonts::{
    FontFace, FontFaceDescriptor, FontFaceSource, FontFaceStyle, resolve_font_face_urls,
};
pub use style::source::resource::{ResourceImport, ResourceStylesheet, ResourceStylesheetEntry};
pub use style::source::imports::{
    ImportLayer, StylesheetEntry, StylesheetImport, stylesheet_entries,
};
pub use style::support::{
    DeclarationSupport, PropertyCapability, PropertySyntax, PropertyValueSyntax,
    UnsupportedStyleFeature,
};
pub use style::syntax::capabilities::property_name_is_supported;
pub use style::syntax::validation::{
    declaration_support, declaration_syntax, property_value_syntax, selector_syntax_is_valid,
    stylesheet_syntax_is_valid,
};

pub use style::environment::{MediaEnvironment, MediaMatchKey, MediaQuerySet, MediaType};
pub use style::program::{
    StyleProgram, StyleProgramCache, StyleProgramCacheStats, resolve_document_with_program,
    style_document_with_cached_program_and_reader_overrides,
    style_document_with_cached_program_and_timings,
    style_document_with_program_and_reader_overrides, style_document_with_program_and_timings,
};

#[cfg(test)]
mod allocation_test_support {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    thread_local! {
        static ALLOCATION_COUNT: Cell<Option<usize>> = const { Cell::new(None) };
    }

    pub(crate) struct TrackingAllocator;

    fn record_allocation() {
        let _ = ALLOCATION_COUNT.try_with(|count| {
            if let Some(current) = count.get() {
                count.set(Some(current + 1));
            }
        });
    }

    unsafe impl GlobalAlloc for TrackingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            record_allocation();
            unsafe { System.alloc(layout) }
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            record_allocation();
            unsafe { System.alloc_zeroed(layout) }
        }

        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            record_allocation();
            unsafe { System.realloc(ptr, layout, new_size) }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            unsafe { System.dealloc(ptr, layout) }
        }
    }

    pub(crate) fn count_allocations<T>(operation: impl FnOnce() -> T) -> (T, usize) {
        ALLOCATION_COUNT.with(|count| {
            assert!(
                count.get().is_none(),
                "allocation counters cannot be nested"
            );
            count.set(Some(0));
        });
        let output = operation();
        let allocations =
            ALLOCATION_COUNT.with(|count| count.take().expect("allocation tracking is active"));
        (output, allocations)
    }
}

#[cfg(test)]
#[global_allocator]
static TEST_ALLOCATOR: allocation_test_support::TrackingAllocator =
    allocation_test_support::TrackingAllocator;

/// Resolve the user-agent stylesheet and all author stylesheet chunks.
///
/// Invalid author chunks are skipped, matching the renderer's previous
/// best-effort behavior. Lightning CSS values remain an implementation detail
/// of this crate rather than leaking into the pipeline API.
pub fn style_document(document: Document, css_chunks: &[&str]) -> StyledDocument {
    style_document_with_timings(document, css_chunks).0
}

pub fn style_document_with_timings(
    document: Document,
    css_chunks: &[&str],
) -> (StyledDocument, StyleTimings) {
    let (styled, timings, _) = style_document_with_environment_and_timings(
        document,
        css_chunks,
        MediaEnvironment::default(),
    );
    (styled, timings)
}

pub fn style_document_with_environment(
    document: Document,
    css_chunks: &[&str],
    environment: MediaEnvironment,
) -> (StyledDocument, MediaQuerySet) {
    let (styled, _, media_queries) =
        style_document_with_environment_and_timings(document, css_chunks, environment);
    (styled, media_queries)
}

pub fn style_document_with_environment_and_timings(
    document: Document,
    css_chunks: &[&str],
    environment: MediaEnvironment,
) -> (StyledDocument, StyleTimings, MediaQuerySet) {
    let root = document.dom_root();
    let inputs = css_chunks
        .iter()
        .map(|css| AuthorStylesheetInput {
            css,
            implicit_scope_root: root,
        })
        .collect::<Vec<_>>();
    style_document_with_author_stylesheets_and_environment_and_timings(
        document,
        &inputs,
        environment,
    )
}

pub fn style_document_with_author_stylesheets_and_environment_and_timings(
    document: Document,
    inputs: &[AuthorStylesheetInput<'_>],
    environment: MediaEnvironment,
) -> (StyledDocument, StyleTimings, MediaQuerySet) {
    style::program::compile_and_apply(document, inputs, environment)
}

#[cfg(test)]
mod boundary_tests {
    use super::{
        AuthorStylesheetInput, DeclarationSupport, MediaEnvironment, PropertyCapability,
        PropertySyntax, StyleProgramCache, UnsupportedStyleFeature, declaration_support,
        selector_syntax_is_valid, style_document, style_document_with_cached_program_and_timings,
        style_document_with_environment,
    };
    use html_style_model::{
        LengthPct, PreferredSize, SizeComparison, TabSizeKind, WhiteSpace,
        resolve_used_preferred_size,
    };
    use std::time::Duration;

    fn standards_document(html: &str) -> html_dom::Document {
        html_parse::parse_dom_document(&format!("<!doctype html>{html}"))
            .expect("valid standards-mode HTML")
    }

    #[test]
    fn compiled_style_programs_are_reused_and_rebind_implicit_scope_roots() {
        let css = "@scope { p { color: red } }";
        let mut cache = StyleProgramCache::default();
        for expected_hits in [0, 1] {
            let document = standards_document("<html><body><p>cached</p></body></html>");
            let root = document.dom_root();
            let inputs = [AuthorStylesheetInput {
                css,
                implicit_scope_root: root,
            }];
            let (styled, timings, _) = style_document_with_cached_program_and_timings(
                &mut cache,
                document,
                &inputs,
                MediaEnvironment::default(),
            );
            let paragraph = styled
                .document()
                .node_ids()
                .find(|node| styled.document().get_dom_tag(*node) == Some("p"))
                .expect("paragraph");
            let style = styled.style_for_node(paragraph).expect("paragraph style");
            assert_eq!(
                styled
                    .text_style(style)
                    .expect("paragraph text style")
                    .color,
                0xFF0000FF
            );
            assert_eq!(cache.stats().hits, expected_hits);
            if expected_hits == 1 {
                assert_eq!(
                    timings.parse_default_css
                        + timings.parse_author_css
                        + timings.prepare_rules
                        + timings.selector_index,
                    Duration::ZERO
                );
            }
        }
        assert_eq!(cache.stats().misses, 1);
    }

    #[test]
    fn renderer_capabilities_limit_syntax_validation() {
        assert_eq!(
            declaration_support("width", "10px"),
            DeclarationSupport {
                syntax: PropertySyntax::Valid,
                capability: PropertyCapability::Supported
            }
        );
        assert_eq!(
            declaration_support("width", "red"),
            DeclarationSupport {
                syntax: PropertySyntax::Invalid,
                capability: PropertyCapability::Supported
            }
        );
        assert_eq!(
            declaration_support("opacity", "0.5"),
            DeclarationSupport {
                syntax: PropertySyntax::Unsupported,
                capability: PropertyCapability::Unsupported(UnsupportedStyleFeature::Property)
            }
        );
        assert_eq!(
            declaration_support("future-property", "some-value"),
            DeclarationSupport {
                syntax: PropertySyntax::Unsupported,
                capability: PropertyCapability::Unsupported(UnsupportedStyleFeature::Property)
            }
        );
    }

    #[test]
    fn gradients_are_recognized_but_explicitly_not_rendered() {
        assert_eq!(
            declaration_support("background", "linear-gradient(red, blue)"),
            DeclarationSupport {
                syntax: PropertySyntax::Unsupported,
                capability: PropertyCapability::Unsupported(UnsupportedStyleFeature::Gradient)
            }
        );
        assert_eq!(
            declaration_support("background", "red"),
            DeclarationSupport {
                syntax: PropertySyntax::Valid,
                capability: PropertyCapability::Supported
            }
        );
        assert_eq!(
            declaration_support("background", "url('paper.png') blue"),
            DeclarationSupport {
                syntax: PropertySyntax::Unsupported,
                capability: PropertyCapability::Unsupported(
                    UnsupportedStyleFeature::BackgroundImage
                )
            }
        );
    }

    #[test]
    fn generated_content_support_is_value_aware() {
        assert_eq!(
            declaration_support("content", "'hello'").syntax,
            PropertySyntax::Valid
        );
        assert_eq!(
            declaration_support("content", "url('marker.svg')").capability,
            PropertyCapability::Unsupported(UnsupportedStyleFeature::GeneratedContent)
        );
        assert_eq!(
            declaration_support("counter-reset", "reversed(chapter)").capability,
            PropertyCapability::Unsupported(UnsupportedStyleFeature::GeneratedContent)
        );
    }

    #[test]
    fn rendered_raw_and_typed_properties_are_reported_as_supported() {
        for (name, value) in [
            ("all", "initial"),
            ("overflow", "hidden auto"),
            ("z-index", "2"),
            ("order", "-1"),
            ("grid-gap", "10px 20%"),
            ("table-layout", "fixed"),
            ("border-collapse", "collapse"),
            ("caption-side", "bottom"),
            ("empty-cells", "hide"),
            ("text-box-trim", "trim-start"),
            ("text-box-edge", "cap alphabetic"),
            ("text-box", "trim-start cap alphabetic"),
        ] {
            assert_eq!(
                declaration_support(name, value),
                DeclarationSupport {
                    syntax: PropertySyntax::Valid,
                    capability: PropertyCapability::Supported
                },
                "{name}: {value}"
            );
        }

        for (name, value) in [
            ("table-layout", "auto fixed"),
            ("border-collapse", "none"),
            ("text-box-edge", "text cap"),
            ("text-box", "cap none alphabetic"),
        ] {
            assert_eq!(
                declaration_support(name, value).syntax,
                PropertySyntax::Unsupported,
                "{name}: {value}"
            );
        }
    }

    #[test]
    fn border_images_are_recognized_but_explicitly_not_rendered() {
        assert_eq!(
            declaration_support("border-image", "url('frame.png') 30 round"),
            DeclarationSupport {
                syntax: PropertySyntax::Unsupported,
                capability: PropertyCapability::Unsupported(UnsupportedStyleFeature::BorderImage)
            }
        );
        assert_eq!(
            declaration_support("border-image-slice", "30 fill"),
            DeclarationSupport {
                syntax: PropertySyntax::Unsupported,
                capability: PropertyCapability::Unsupported(UnsupportedStyleFeature::BorderImage)
            }
        );
    }

    #[test]
    fn box_shadows_are_recognized_but_explicitly_not_rendered() {
        assert_eq!(
            declaration_support("BOX-SHADOW", "2px 4px 8px black"),
            DeclarationSupport {
                syntax: PropertySyntax::Unsupported,
                capability: PropertyCapability::Unsupported(UnsupportedStyleFeature::BoxShadow)
            }
        );
    }

    #[test]
    fn filter_effects_are_recognized_but_explicitly_not_rendered() {
        assert_eq!(
            declaration_support("filter", "blur(2px)"),
            DeclarationSupport {
                syntax: PropertySyntax::Unsupported,
                capability: PropertyCapability::Unsupported(UnsupportedStyleFeature::FilterEffects)
            }
        );
        assert_eq!(
            declaration_support("backdrop-filter", "contrast(150%)"),
            DeclarationSupport {
                syntax: PropertySyntax::Unsupported,
                capability: PropertyCapability::Unsupported(UnsupportedStyleFeature::FilterEffects)
            }
        );
    }

    #[test]
    fn transitions_are_recognized_but_explicitly_not_rendered() {
        assert_eq!(
            declaration_support("transition", "color 200ms ease-in"),
            DeclarationSupport {
                syntax: PropertySyntax::Unsupported,
                capability: PropertyCapability::Unsupported(UnsupportedStyleFeature::Transitions)
            }
        );
        assert_eq!(
            declaration_support("transition-duration", "200ms"),
            DeclarationSupport {
                syntax: PropertySyntax::Unsupported,
                capability: PropertyCapability::Unsupported(UnsupportedStyleFeature::Transitions)
            }
        );
    }

    #[test]
    fn animations_are_recognized_but_explicitly_not_rendered() {
        assert_eq!(
            declaration_support("animation", "chapter-fade 200ms ease-in"),
            DeclarationSupport {
                syntax: PropertySyntax::Unsupported,
                capability: PropertyCapability::Unsupported(UnsupportedStyleFeature::Animations)
            }
        );
        assert_eq!(
            declaration_support("animation-duration", "200ms"),
            DeclarationSupport {
                syntax: PropertySyntax::Unsupported,
                capability: PropertyCapability::Unsupported(UnsupportedStyleFeature::Animations)
            }
        );
    }

    #[test]
    fn selector_validation_uses_native_acceptance() {
        assert!(selector_syntax_is_valid("p:not(.excluded)"));
        assert!(selector_syntax_is_valid(":heading(2n)"));
        assert!(!selector_syntax_is_valid("p:not("));
        assert!(!selector_syntax_is_valid("p >"));
    }

    #[test]
    fn invalid_selector_lists_cannot_leak_declarations_into_the_cascade() {
        let styled =
            styled_paragraph("p { color: green; } p, > { color: red; } p:not(>) { color: red; }");
        assert_eq!(paragraph_color(&styled), 0x008000FF);
    }

    #[test]
    fn a_bad_author_rule_does_not_discard_the_whole_stylesheet_chunk() {
        let styled = styled_paragraph("p { color: green; } ::part(foo):lang(en) { color: red; }");
        assert_eq!(paragraph_color(&styled), 0x008000FF);
    }

    #[test]
    fn conditional_rules_are_evaluated_once_during_preparation() {
        let styled = styled_paragraph(
            "p { color: black } @media all { p { color: red } } @media print { p { color: blue } } @supports (width: 10px) { p { color: green } } @supports (opacity: 0.5) { p { color: blue } } @supports (background: linear-gradient(red, blue)) { p { color: blue } }",
        );
        assert_eq!(paragraph_color(&styled), 0x008000FF);
    }

    #[test]
    fn supports_uses_the_renderer_font_kerning_grammar() {
        assert_eq!(
            declaration_support("font-kerning", "auto"),
            DeclarationSupport {
                syntax: PropertySyntax::Valid,
                capability: PropertyCapability::Supported
            }
        );
        assert_eq!(
            declaration_support("font-kerning", "sideways"),
            DeclarationSupport {
                syntax: PropertySyntax::Unsupported,
                capability: PropertyCapability::Unsupported(UnsupportedStyleFeature::Value)
            }
        );

        let styled = styled_paragraph(
            "p { color: red } @supports (font-kerning: auto) { p { color: green } } @supports (font-kerning: sideways) { p { color: blue } }",
        );
        assert_eq!(paragraph_color(&styled), 0x008000FF);
    }

    #[test]
    fn width_media_queries_use_the_renderer_environment() {
        let css = "p { color: black } @media (min-width: 500px) { p { color: green } }";
        let narrow_document = standards_document("<html><body><p>text</p></body></html>");
        let wide_document = standards_document("<html><body><p>text</p></body></html>");

        let (narrow, _) = style_document_with_environment(
            narrow_document,
            &[css],
            MediaEnvironment::screen(400.0, Some(800.0)).unwrap(),
        );
        let (wide, _) = style_document_with_environment(
            wide_document,
            &[css],
            MediaEnvironment::screen(600.0, Some(800.0)).unwrap(),
        );

        assert_eq!(paragraph_color(&narrow), 0x000000FF);
        assert_eq!(paragraph_color(&wide), 0x008000FF);
    }

    #[test]
    fn viewport_font_units_use_the_effective_column_dimensions() {
        let document = standards_document("<html><body><p>text</p></body></html>");
        let environment = MediaEnvironment::screen(800.0, Some(600.0)).unwrap();
        let css = "html { font-size: 10vw } p { font-size: 10vh; width: calc(25vw + 5px); height: 10vmin; margin-left: 10vmax; line-height: 5vh }";
        let (styled, dependencies) = style_document_with_environment(document, &[css], environment);
        let root = styled.document().dom_root().expect("document element");
        let paragraph = paragraph_node(&styled);
        let paragraph_indices = styled.style_for_node(paragraph).expect("paragraph style");
        let box_model = styled
            .box_model_style(paragraph_indices)
            .expect("validated box style");
        let text = styled
            .text_style(paragraph_indices)
            .expect("validated text style");

        assert_eq!(
            styled
                .font_style(styled.style_for_node(root).expect("root style"))
                .expect("root font")
                .font_size,
            80.0
        );
        assert_eq!(
            styled
                .font_style(paragraph_indices)
                .expect("paragraph font")
                .font_size,
            60.0
        );
        assert_eq!(
            box_model.width,
            PreferredSize::Calc {
                absolute_px: 205.0,
                percentage: 0.0,
                x_height_px: 0.0,
                ch_advance_px: 0.0,
                cap_height_px: 0.0,
                percentage_dependent: false
            }
        );
        assert_eq!(box_model.height, PreferredSize::Px(60.0));
        assert_eq!(box_model.margin_left, LengthPct::Px(80.0));
        assert_eq!(text.line_height, 30.0);
        assert!(dependencies.uses_viewport_units());
    }

    #[test]
    fn height_dependent_viewport_units_do_not_guess_when_height_is_unavailable() {
        let document = standards_document("<html><body><p>text</p></body></html>");
        let environment = MediaEnvironment::screen(800.0, None).unwrap();
        let css = "p { font-size: 20px; font-size: 10vh; width: 123px; width: 10vmin; margin-left: 10vw }";
        let (styled, dependencies) = style_document_with_environment(document, &[css], environment);
        let indices = styled
            .style_for_node(paragraph_node(&styled))
            .expect("paragraph style");
        let box_model = styled
            .box_model_style(indices)
            .expect("validated box style");

        assert_eq!(
            styled
                .font_style(indices)
                .expect("paragraph font")
                .font_size,
            20.0
        );
        assert_eq!(box_model.width, PreferredSize::Px(123.0));
        assert_eq!(box_model.margin_left, LengthPct::Px(80.0));
        assert!(dependencies.uses_viewport_units());
    }

    #[test]
    fn lh_uses_parent_line_height_for_font_prerequisites_and_own_line_height_elsewhere() {
        let document = standards_document(
            "<html><body><main><aside>font</aside><section>line</section><article>ordinary</article></main></body></html>",
        );
        let css = "main { font-size: 50px; line-height: 1 } aside { font-size: 2lh; line-height: 42px; height: 1em } section { font-size: 42px; line-height: 2lh; width: 1lh } article { width: calc(1lh + 5px); line-height: 2; font-size: 25px }";
        let styled = style_document(document, &[css]);
        let element_style = |tag: &str| {
            let node = styled
                .document()
                .nodes()
                .find_map(|(id, node)| {
                    matches!(node, html_dom::NodeRef::Element(element) if element.tag() == tag)
                        .then_some(id)
                })
                .expect("test element");
            styled.style_for_node(node).expect("element style")
        };

        let aside = element_style("aside");
        assert_eq!(
            styled.font_style(aside).expect("aside font").font_size,
            100.0
        );
        assert_eq!(
            styled.box_model_style(aside).expect("aside box").height,
            PreferredSize::Px(100.0)
        );

        let section = element_style("section");
        assert_eq!(
            styled
                .text_style(section)
                .expect("section text")
                .line_height,
            100.0
        );
        assert_eq!(
            styled.box_model_style(section).expect("section box").width,
            PreferredSize::Px(100.0)
        );

        let article = element_style("article");
        assert_eq!(
            styled.font_style(article).expect("article font").font_size,
            25.0
        );
        assert_eq!(
            styled
                .text_style(article)
                .expect("article text")
                .line_height,
            50.0
        );
        assert_eq!(
            styled.box_model_style(article).expect("article box").width,
            PreferredSize::Calc {
                absolute_px: 55.0,
                percentage: 0.0,
                x_height_px: 0.0,
                ch_advance_px: 0.0,
                cap_height_px: 0.0,
                percentage_dependent: false
            }
        );
    }

    #[test]
    fn lh_uses_the_renderers_normal_line_height() {
        let styled = styled_paragraph("p { font-size: 20px; height: 1lh }");
        let indices = styled
            .style_for_node(paragraph_node(&styled))
            .expect("paragraph style");
        assert_eq!(
            styled
                .box_model_style(indices)
                .expect("paragraph box")
                .height,
            PreferredSize::Px(24.0)
        );
    }

    #[test]
    fn rlh_uses_the_final_root_line_height_in_direct_and_calculated_lengths() {
        let document = standards_document("<html><body><p>text</p></body></html>");
        let styled = style_document(
            document,
            &[
                "html { line-height: 50px; width: 1rlh } p { line-height: 10px; width: calc(2rlh - 50px) }",
            ],
        );
        let root = styled.document().dom_root().expect("document element");
        let root_indices = styled.style_for_node(root).expect("root style");
        let paragraph_indices = styled
            .style_for_node(paragraph_node(&styled))
            .expect("paragraph style");

        assert_eq!(
            styled
                .box_model_style(root_indices)
                .expect("root box")
                .width,
            PreferredSize::Px(50.0)
        );
        assert_eq!(
            styled
                .box_model_style(paragraph_indices)
                .expect("paragraph box")
                .width,
            PreferredSize::Calc {
                absolute_px: 50.0,
                percentage: 0.0,
                x_height_px: 0.0,
                ch_advance_px: 0.0,
                cap_height_px: 0.0,
                percentage_dependent: false
            }
        );
    }

    #[test]
    fn cap_lengths_resolve_from_the_selected_fonts_cap_height() {
        let styled =
            styled_paragraph("p { font-size: 50px; width: 2.5cap; height: calc(180px - 2cap) }");
        let indices = styled
            .style_for_node(paragraph_node(&styled))
            .expect("paragraph style");
        let box_model = styled.box_model_style(indices).expect("paragraph box");

        assert_eq!(box_model.width, PreferredSize::Cap(125.0));
        assert_eq!(
            box_model.height,
            PreferredSize::Calc {
                absolute_px: 180.0,
                percentage: 0.0,
                x_height_px: 0.0,
                ch_advance_px: 0.0,
                cap_height_px: -100.0,
                percentage_dependent: false
            }
        );
        assert!(
            styled
                .styles()
                .view(indices)
                .expect("style view")
                .requires_font_metrics()
        );

        let used = styled
            .styles()
            .used_view(indices, 0.5, 0.5, 0.8)
            .expect("selected font metrics");
        assert_eq!(used.width(), html_style_model::UsedPreferredSize::Px(100.0));
        assert_eq!(
            used.height(),
            html_style_model::UsedPreferredSize::Calc {
                absolute_px: 100.0,
                percentage: 0.0,
                percentage_dependent: false
            }
        );
    }

    #[test]
    fn conditional_layer_order_uses_the_renderer_environment() {
        let css = "@media (min-width: 500px) { @layer second, first; } @layer first { p { color: red } } @layer second { p { color: green } }";
        let narrow_document = standards_document("<html><body><p>text</p></body></html>");
        let wide_document = standards_document("<html><body><p>text</p></body></html>");

        let (narrow, _) = style_document_with_environment(
            narrow_document,
            &[css],
            MediaEnvironment::screen(400.0, Some(800.0)).unwrap(),
        );
        let (wide, _) = style_document_with_environment(
            wide_document,
            &[css],
            MediaEnvironment::screen(600.0, Some(800.0)).unwrap(),
        );

        assert_eq!(paragraph_color(&narrow), 0x008000FF);
        assert_eq!(paragraph_color(&wide), 0xFF0000FF);
    }

    #[test]
    fn normal_layer_order_and_unlayered_precedence_are_honored() {
        let styled = styled_paragraph(
            "@layer first, second; @layer second { p { color: red } } @layer first { p { color: blue } } p { color: green }",
        );
        assert_eq!(paragraph_color(&styled), 0x008000FF);
    }

    #[test]
    fn important_layer_order_is_reversed() {
        let styled = styled_paragraph(
            "@layer first, second; @layer second { p { color: red !important } } @layer first { p { color: green !important } } p { color: blue !important }",
        );
        assert_eq!(paragraph_color(&styled), 0x008000FF);
    }

    #[test]
    fn declarations_directly_in_a_layer_outrank_its_nested_layers() {
        let styled = styled_paragraph(
            "@layer outer { p { color: green } @layer inner { p { color: red } } }",
        );
        assert_eq!(paragraph_color(&styled), 0x008000FF);
    }

    #[test]
    fn later_white_space_declaration_resets_the_compatibility_state() {
        let styled =
            styled_paragraph("p { white-space: preserve-breaks nowrap; white-space: normal; }");
        assert_eq!(paragraph_white_space(&styled), WhiteSpace::Normal);
    }

    #[test]
    fn native_spacing_after_percentage_clears_internal_cascade_state() {
        let styled =
            styled_paragraph("p { font-size: 20px; letter-spacing: 120%; letter-spacing: 2em; }");
        let paragraph = paragraph_node(&styled);
        let indices = styled.style_for_node(paragraph).expect("paragraph style");
        let text = styled.text_style(indices).expect("validated text style");

        assert_eq!(text.letter_spacing.absolute_px(), 40.0);
        assert_eq!(text.letter_spacing.font_size_fraction(), 0.0);
    }

    #[test]
    fn metric_dependent_ch_tab_size_does_not_guess_a_computed_length() {
        let document = standards_document("<html><body><p><span>text</span></p></body></html>");
        let styled = style_document(
            document,
            &["p { tab-size: 2.5ch; } span { font-size: 40px; }"],
        );
        let span = styled
            .document()
            .nodes()
            .find_map(|(id, node)| {
                matches!(node, html_dom::NodeRef::Element(element) if element.tag() == "span")
                    .then_some(id)
            })
            .expect("span node");
        let indices = styled.style_for_node(span).expect("span style");
        let tab_size = styled
            .text_style(indices)
            .expect("validated text style")
            .tab_size;

        assert_eq!(tab_size.kind(), TabSizeKind::Spaces);
        assert_eq!(
            tab_size.value(),
            8.0,
            "unsupported ch lengths must preserve the inherited initial value instead of using a guessed 0.5em width"
        );
    }

    #[test]
    fn invalid_tab_size_cannot_override_a_prior_valid_declaration() {
        let styled = styled_paragraph("p { tab-size: 3; tab-size: nonsense; }");
        let indices = styled
            .style_for_node(paragraph_node(&styled))
            .expect("paragraph style");
        let tab_size = styled
            .text_style(indices)
            .expect("validated text style")
            .tab_size;
        assert_eq!(tab_size.kind(), TabSizeKind::Spaces);
        assert_eq!(tab_size.value(), 3.0);
    }

    #[test]
    fn computed_negative_calc_tab_length_clamps_to_zero() {
        let styled = styled_paragraph("p { font-size: 20px; tab-size: calc(5px - 0.5em); }");
        let indices = styled
            .style_for_node(paragraph_node(&styled))
            .expect("paragraph style");
        let tab_size = styled
            .text_style(indices)
            .expect("validated text style")
            .tab_size;
        assert_eq!(tab_size.kind(), TabSizeKind::LengthPx);
        assert_eq!(tab_size.value(), 0.0);
    }

    #[test]
    fn computed_number_calc_keeps_number_semantics() {
        let styled = styled_paragraph("p { tab-size: calc(2 + 3); }");
        let indices = styled
            .style_for_node(paragraph_node(&styled))
            .expect("paragraph style");
        let tab_size = styled
            .text_style(indices)
            .expect("validated text style")
            .tab_size;
        assert_eq!(tab_size.kind(), TabSizeKind::Spaces);
        assert_eq!(tab_size.value(), 5.0);
    }

    #[test]
    fn tab_size_marker_preserves_important_and_initial_cascade_semantics() {
        let document = standards_document("<html><body><p><span>text</span></p></body></html>");
        let styled = style_document(
            document,
            &["p { tab-size: 2 !important; tab-size: 4; } span { tab-size: initial; }"],
        );
        let paragraph = paragraph_node(&styled);
        let paragraph_tab = styled
            .text_style(styled.style_for_node(paragraph).expect("paragraph style"))
            .expect("paragraph text style")
            .tab_size;
        let span = styled
            .document()
            .nodes()
            .find_map(|(id, node)| {
                matches!(node, html_dom::NodeRef::Element(element) if element.tag() == "span")
                    .then_some(id)
            })
            .expect("span node");
        let span_tab = styled
            .text_style(styled.style_for_node(span).expect("span style"))
            .expect("span text style")
            .tab_size;

        assert_eq!(paragraph_tab.value(), 2.0);
        assert_eq!(span_tab, html_style_model::TabSize::DEFAULT);
    }

    #[test]
    fn mixed_length_percentage_calc_survives_computed_style() {
        let styled = styled_paragraph(
            "p { font-size: 10px; width: calc(50% - 3px); min-height: calc(50% - 100px); \
             margin-left: calc(10% - 4px); padding-right: calc(5% + 1px); \
             right: calc(20% - 7px); text-indent: calc(30% + 6px); }",
        );
        let indices = styled
            .style_for_node(paragraph_node(&styled))
            .expect("paragraph style");
        let box_model = styled
            .box_model_style(indices)
            .expect("validated box style");
        let layout = styled
            .styles()
            .layout_style(indices)
            .expect("validated layout style");
        let text = styled.text_style(indices).expect("validated text style");

        assert_eq!(
            box_model.width,
            PreferredSize::Calc {
                absolute_px: -3.0,
                percentage: 0.5,
                x_height_px: 0.0,
                ch_advance_px: 0.0,
                cap_height_px: 0.0,
                percentage_dependent: true
            }
        );
        assert_eq!(
            box_model.min_height,
            PreferredSize::Calc {
                absolute_px: -100.0,
                percentage: 0.5,
                x_height_px: 0.0,
                ch_advance_px: 0.0,
                cap_height_px: 0.0,
                percentage_dependent: true
            }
        );
        assert_eq!(
            box_model.margin_left,
            LengthPct::Calc {
                absolute_px: -4.0,
                percentage: 0.1,
                x_height_px: 0.0,
                ch_advance_px: 0.0,
                cap_height_px: 0.0,
                percentage_dependent: true
            }
        );
        assert_eq!(
            box_model.padding_right,
            LengthPct::Calc {
                absolute_px: 1.0,
                percentage: 0.05,
                x_height_px: 0.0,
                ch_advance_px: 0.0,
                cap_height_px: 0.0,
                percentage_dependent: true
            }
        );
        assert_eq!(
            layout.inset_right,
            Some(LengthPct::Calc {
                absolute_px: -7.0,
                percentage: 0.2,
                x_height_px: 0.0,
                ch_advance_px: 0.0,
                cap_height_px: 0.0,
                percentage_dependent: true
            })
        );
        assert_eq!(
            text.text_indent,
            LengthPct::Calc {
                absolute_px: 6.0,
                percentage: 0.3,
                x_height_px: 0.0,
                ch_advance_px: 0.0,
                cap_height_px: 0.0,
                percentage_dependent: true
            }
        );
    }

    #[test]
    fn mixed_length_percentage_comparisons_resolve_against_the_containing_block() {
        let styled = styled_paragraph(
            "p { width: max(100px, 25% + 100px, 150px + 10%); height: min(300px, 25% + 100px, 50px + 50%); min-height: clamp(100px, 50%, 300px) }",
        );
        let indices = styled
            .style_for_node(paragraph_node(&styled))
            .expect("paragraph style");
        let box_model = styled
            .box_model_style(indices)
            .expect("validated box style");

        assert!(matches!(box_model.width, PreferredSize::Comparison(_)));
        assert!(matches!(box_model.height, PreferredSize::Comparison(_)));
        assert!(matches!(box_model.min_height, PreferredSize::Comparison(_)));

        let used = styled
            .styles()
            .used_view(indices, 0.5, 0.5, 0.8)
            .expect("font metrics resolve");
        assert!(matches!(
            used.width(),
            html_style_model::UsedPreferredSize::Comparison {
                kind: SizeComparison::Max,
                count: 3,
                ..
            }
        ));
        assert!(matches!(
            used.height(),
            html_style_model::UsedPreferredSize::Comparison {
                kind: SizeComparison::Min,
                count: 3,
                ..
            }
        ));
        assert!(matches!(
            used.min_height(),
            html_style_model::UsedPreferredSize::Comparison {
                kind: SizeComparison::Clamp,
                count: 3,
                ..
            }
        ));
        assert_eq!(resolve_used_preferred_size(used.width(), 0.0, 400.0), 200.0);
        assert_eq!(
            resolve_used_preferred_size(used.height(), 0.0, 400.0),
            200.0
        );
        assert_eq!(
            resolve_used_preferred_size(used.min_height(), 0.0, 400.0),
            200.0
        );
    }

    #[test]
    fn finished_styles_are_dense_and_drop_construction_indexes() {
        let document = standards_document(
            "<html><body><p class='same'>one</p><p class='same'>two</p></body></html>",
        );
        let styled = style_document(document, &[".same { color: red; margin-left: 1em; }"]);

        assert_eq!(styled.styles().node_count(), styled.document().node_count());
        let report = styled.styles().memory_usage_report();
        for entry in report
            .entries
            .iter()
            .filter(|entry| entry.label.contains("_dedup.storage"))
        {
            assert_eq!(
                entry.count, 0,
                "{} retained construction entries",
                entry.label
            );
            assert_eq!(
                entry.bytes, 0,
                "{} retained construction capacity",
                entry.label
            );
        }
    }

    #[test]
    fn css_strings_belong_to_style_output_not_the_parsed_dom() {
        let document = standards_document("<html><body><p>text</p></body></html>");
        assert!(document.lookup_string("Boundary Family").is_none());

        let styled = style_document(document, &["p { font-family: 'Boundary Family'; }"]);
        let paragraph = styled
            .document()
            .nodes()
            .find_map(|(id, node)| {
                matches!(node, html_dom::NodeRef::Element(element) if element.tag() == "p")
                    .then_some(id)
            })
            .expect("paragraph node");
        let indices = styled.style_for_node(paragraph).expect("paragraph style");
        let family = styled
            .font_style(indices)
            .expect("validated style handle")
            .font_family
            .expect("authored family");

        assert!(styled.document().lookup_string("Boundary Family").is_none());
        assert!(
            styled
                .styles()
                .string(family)
                .expect("style-owned family")
                .contains("Boundary Family")
        );
    }

    fn styled_paragraph(css: &str) -> super::StyledDocument {
        let document = standards_document("<html><body><p>text</p></body></html>");
        style_document(document, &[css])
    }

    fn paragraph_color(styled: &super::StyledDocument) -> u32 {
        let paragraph = paragraph_node(styled);
        let indices = styled.style_for_node(paragraph).expect("paragraph style");
        styled
            .text_style(indices)
            .expect("validated text style")
            .color
    }

    fn paragraph_white_space(styled: &super::StyledDocument) -> WhiteSpace {
        let paragraph = paragraph_node(styled);
        let indices = styled.style_for_node(paragraph).expect("paragraph style");
        styled
            .text_style(indices)
            .expect("validated text style")
            .white_space
    }

    fn paragraph_node(styled: &super::StyledDocument) -> html_dom::DomNodeId {
        styled
            .document()
            .nodes()
            .find_map(|(id, node)| {
                matches!(node, html_dom::NodeRef::Element(element) if element.tag() == "p")
                    .then_some(id)
            })
            .expect("paragraph node")
    }
}
