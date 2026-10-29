use html_dom::{Document, DocumentBuilder};
use html_style_model::{
    ComputedStyles, ComputedStylesBuilder, InheritedText, StyleIndices, TextTransform, WordBreak,
};
use html_text::{
    InlineContent, InlineItem, InlineItemKind, PreparedText, TextStructure, WhitespaceWrapOverride,
};

struct Context {
    parent: Option<usize>,
    style: StyleIndices,
    inline: bool,
}

struct Structure(Vec<Context>);

impl TextStructure for Structure {
    fn get_box_parent(&self, context: usize) -> Option<usize> {
        self.0[context].parent
    }
    fn get_box_style_indices(&self, context: usize) -> Option<StyleIndices> {
        Some(self.0[context].style)
    }
    fn get_box_dom_element(&self, _: usize) -> Option<u32> {
        None
    }
    fn is_inline(&self, context: usize) -> bool {
        self.0[context].inline
    }
    fn is_flex_or_grid(&self, _: usize) -> bool {
        false
    }
}

fn inputs(text_style: InheritedText) -> (Document, ComputedStyles, Structure) {
    let document = DocumentBuilder::new().finish_without_root().unwrap();
    let mut builder = ComputedStylesBuilder::new(&document);
    let style = builder
        .push(
            Default::default(),
            text_style,
            Default::default(),
            Default::default(),
            Default::default(),
        )
        .unwrap();
    let styles = builder.finish().unwrap();
    let structure = Structure(vec![
        Context {
            parent: None,
            style,
            inline: false,
        },
        Context {
            parent: Some(0),
            style,
            inline: true,
        },
        Context {
            parent: Some(0),
            style,
            inline: true,
        },
    ]);
    (document, styles, structure)
}

fn append(content: &mut InlineContent, owner: u32, text: &str) {
    let start = content.characters().len() as u32;
    let mut offset = 0;
    for character in text.chars() {
        content.push_source_character(character, offset);
        offset += character.len_utf16() as u32;
    }
    let end = content.characters().len() as u32;
    content.push_inline_item(InlineItem {
        kind: InlineItemKind::Text { glyphs: start..end },
        box_idx: owner,
        dom_text_node: Some(owner),
    });
}

fn plain(text: &PreparedText) -> String {
    text.content()
        .characters()
        .iter()
        .copied()
        .collect()
}

#[test]
fn collapses_across_inline_owners_and_moves_source_items_without_copying() {
    // Every input except the owned source is dropped before the result is read.
    let (text, original_items) = {
        let (document, styles, structure) = inputs(InheritedText::default());
        let mut content = InlineContent::default();
        append(&mut content, 1, "a ");
        append(&mut content, 2, "  b");
        let original_items = content.inline_items().as_ptr();
        (
            PreparedText::new(&document, &styles, &structure, content),
            original_items,
        )
    };
    assert_eq!(plain(&text), "a b");
    assert_eq!(text.content().inline_items().as_ptr(), original_items);
    assert_eq!(text.content().inline_items()[1].dom_text_node, Some(2));
    assert_eq!(text.content().glyph_source_offset(2), Some(2));
}

#[test]
fn case_expansion_preserves_utf16_source_correspondence() {
    let (document, styles, structure) = inputs(InheritedText {
        text_transform: TextTransform::Uppercase,
        ..Default::default()
    });
    let mut content = InlineContent::default();
    append(&mut content, 1, "ß😀x");
    let text = PreparedText::new(&document, &styles, &structure, content);
    assert_eq!(plain(&text), "SS😀X");
    let offsets: Vec<_> = (0..4)
        .map(|index| text.content().glyph_source_offset(index))
        .collect();
    assert_eq!(offsets, [Some(0), Some(0), Some(1), Some(3)]);
    assert!(
        matches!(&text.content().inline_items()[0].kind, InlineItemKind::Text { glyphs } if glyphs == &(0..4))
    );
}

#[test]
fn records_wrap_opportunities_without_splitting_combining_sequences() {
    let (document, styles, structure) = inputs(InheritedText {
        word_break: WordBreak::BreakAll,
        ..Default::default()
    });
    let mut content = InlineContent::default();
    append(&mut content, 1, "a\u{301}b");
    let text = PreparedText::new(&document, &styles, &structure, content);
    assert_eq!(plain(&text), "a\u{301}b");
    assert_eq!(
        text.content().whitespace_wrap_before(1),
        WhitespaceWrapOverride::UnicodeSuppress
    );
    assert_eq!(
        text.content().whitespace_wrap_before(2),
        WhitespaceWrapOverride::UnicodeAllow
    );
}

#[test]
fn separate_formatting_contexts_do_not_share_pending_spaces() {
    let (document, styles, mut structure) = inputs(InheritedText::default());
    structure.0[2].parent = None;
    structure.0[2].inline = false;
    let mut content = InlineContent::default();
    append(&mut content, 1, "A ");
    append(&mut content, 2, " B");
    let text = PreparedText::new(&document, &styles, &structure, content);
    assert_eq!(plain(&text), "AB");
}
