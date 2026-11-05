//! Unicode/CSS wrap opportunities in logical source order. Preparation records
//! decisions in retained wrap policies; layout still checks native clusters.

use html_style_model::language;
use super::get_style;
use crate::{InlineContent, InlineItemKind, TextStructure, WhitespaceWrapOverride};
use icu_segmenter::LineSegmenter;
use html_style_model::{ComputedStyles, ContentLanguage, LineBreak, StyleView, WordBreak};
use rustc_data_structures::fx::{FxHashMap, FxHashSet};
use unicode_linebreak::{BreakClass, break_property, linebreaks};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthChar;

/// Broad East Asian ranges, including supplementary Han extensions. This is
/// a cheap range test after the Latin run reduction.
/// Detailed Unicode classification is confined to candidate paragraphs.
pub fn is_cjk(character: char) -> bool {
    matches!(character as u32,
        0x1100..=0x11ff | 0x2e80..=0xa4cf | 0xa960..=0xa97f
        | 0xac00..=0xd7ff | 0xf900..=0xfaff | 0xfe10..=0xfe1f
        | 0xfe30..=0xfe6f | 0xff01..=0xffef | 0x16fe0..=0x18dff
        | 0x1aff0..=0x1b2ff | 0x20000..=0x3ffff)
}

/// Keep ordinary spaced Latin text on the cheap path, including accented
/// letters. Dashes, joining controls and other implicit opportunities still
/// require the paragraph-wide Unicode rules outside East Asian scripts.
pub(super) fn needs_unicode_breaks(character: char) -> bool {
    is_cjk(character)
        || needs_contextual_breaks(character)
        || (character != '\u{00ad}' && matches!(
            break_property(character as u32),
            BreakClass::BeforeAndAfter | BreakClass::After | BreakClass::Before
                | BreakClass::Hyphen | BreakClass::WordJoiner
                | BreakClass::NonBreakingGlue | BreakClass::ZeroWidthJoiner
                | BreakClass::Symbol | BreakClass::RegionalIndicator
                | BreakClass::EmojiBase | BreakClass::EmojiModifier
        ))
}

/// UAX14 SA scripts require lexical analysis. Javanese's aksara rules were
/// introduced after unicode-linebreak's tables; ICU supplies those rules too.
fn needs_contextual_breaks(character: char) -> bool {
    break_property(character as u32) == BreakClass::ComplexContext
        || matches!(character as u32, 0xa980..=0xa9df)
}

/// Conservative segment-break joining for scripts conventionally written
/// without spaces. Korean syllables and jamo intentionally retain spaces.
pub fn is_unspaced_cjk(character: char) -> bool {
    is_cjk(character)
        && !matches!(character as u32,
        0x1100..=0x11ff | 0xa960..=0xa97f | 0xac00..=0xd7ff | 0xffa0..=0xffdc)
}

/// Explicit script subtags take precedence over the language's usual script.
#[derive(Clone, Copy, PartialEq)]
pub enum CjkWritingSystem {
    ChineseJapanese,
    Korean,
    Other,
}

impl CjkWritingSystem {
    pub fn for_language(language: Option<&ContentLanguage>) -> Self {
        let Some(tag) = language.and_then(language::content_language_tag) else {
            return Self::Other;
        };
        match tag.script() {
            Some(
                "Hans" | "Hant" | "Hani" | "Hanb" | "Bopo" | "Jpan" | "Hrkt" | "Hira" | "Kana",
            ) => Self::ChineseJapanese,
            Some("Kore" | "Hang" | "Jamo") => Self::Korean,
            Some(_) => Self::Other,
            None => match tag.primary_language() {
                "ja" | "zh" | "cmn" | "yue" | "nan" | "wuu" | "gan" | "hak" | "hsn" => {
                    Self::ChineseJapanese
                }
                "ko" => Self::Korean,
                _ => Self::Other,
            },
        }
    }
}

#[derive(Clone, Copy)]
struct Policy {
    word_break: WordBreak,
    line_break: LineBreak,
    wraps: bool,
    preserves_spaces: bool,
    chinese_or_japanese: bool,
}

impl Policy {
    fn new(style: StyleView<'_>, styles: &ComputedStyles) -> Self {
        let chinese_or_japanese =
            CjkWritingSystem::for_language(style.language().and_then(|id| styles.language(id)))
                == CjkWritingSystem::ChineseJapanese;
        Self {
            word_break: style.word_break(),
            line_break: style.line_break(),
            wraps: style.white_space().allows_wrap(),
            preserves_spaces: style.white_space().preserves_spaces(),
            chinese_or_japanese,
        }
    }
}

struct Character {
    owner: usize,
    glyph: Option<usize>,
    byte: usize,
    character: char,
    class: BreakClass,
    letter: bool,
    policy: Policy,
}

#[derive(Default)]
struct Paragraph {
    text: String,
    characters: Vec<Character>,
}

impl Paragraph {
    fn push(&mut self, owner: usize, glyph: Option<usize>, character: char, policy: Policy) {
        let class = class(character);
        let previous = self.characters.last();
        let letter = if matches!(
            class,
            BreakClass::CombiningMark | BreakClass::ZeroWidthJoiner
        ) {
            previous.is_some_and(|previous| previous.letter)
        } else {
            character.is_alphabetic()
                || matches!(
                    class,
                    BreakClass::Numeric
                        | BreakClass::Alphabetic
                        | BreakClass::Ambiguous
                        | BreakClass::Ideographic
                )
        };
        let tailored = tailor(character, class, policy, previous);
        let byte = self.text.len();
        self.text.push(tailored);
        self.characters.push(Character {
            owner,
            glyph,
            byte,
            character,
            class,
            letter,
            policy,
        });
    }

    fn resolve(
        &self,
        content: &mut InlineContent,
        styles: &ComputedStyles,
        tree: &(impl TextStructure + ?Sized),
    ) {
        // Graphemes can span differently styled inline runs. Check the whole
        // logical paragraph as well as the backend's later cluster checks.
        let original = self
            .characters
            .iter()
            .map(|entry| entry.character)
            .collect::<String>();
        let mut graphemes = original.graphemes(true);
        let mut next_grapheme = 0;
        let mut opportunities = linebreaks(&self.text).peekable();
        // Construct the data-backed segmenter only for paragraphs containing
        // scripts that need it. This detects the script independently of lang,
        // so absent or incorrect language metadata cannot disable wrapping.
        let contextual = self.characters.iter().any(|entry| needs_contextual_breaks(entry.character))
            .then(|| LineSegmenter::new_auto(Default::default())
                .segment_str(&self.text).collect::<FxHashSet<usize>>());
        let mut wrap_overrides = content.unicode_wrap_writer();
        for (index, current) in self.characters.iter().enumerate() {
            let boundary = index == next_grapheme;
            if boundary {
                next_grapheme += graphemes
                    .next()
                    .map_or(0, |cluster| cluster.chars().count());
            }
            while opportunities
                .peek()
                .is_some_and(|&(byte, _)| byte < current.byte)
            {
                opportunities.next();
            }
            let Some(glyph) = current.glyph else { continue };
            if matches!(
                current.character,
                '\n' | '\u{200b}' | '\u{00ad}' | '\u{200c}' | '\u{200d}'
            ) || (matches!(current.character, ' ' | '\t')
                && (current.policy.line_break != LineBreak::Anywhere
                    || !current.policy.preserves_spaces))
            {
                continue;
            }
            let Some(previous) = index.checked_sub(1).map(|index| &self.characters[index]) else {
                wrap_overrides.set(glyph, false);
                continue;
            };
            let anywhere = current.policy.line_break == LineBreak::Anywhere;
            let mut allowed = anywhere
                || (previous.character != '\u{00ad}' && opportunities
                    .peek()
                    .is_some_and(|&(byte, _)| byte == current.byte));
            if !anywhere {
                if current.policy.word_break != WordBreak::BreakAll
                    && previous.policy.word_break != WordBreak::BreakAll
                    && (needs_contextual_breaks(current.character) || needs_contextual_breaks(previous.character))
                    && let Some(contextual) = &contextual
                {
                    allowed = contextual.contains(&current.byte);
                }
                if (current.policy.word_break == WordBreak::KeepAll
                    || previous.policy.word_break == WordBreak::KeepAll)
                    && (is_cjk(current.character) || is_cjk(previous.character))
                    && current.letter
                    && previous.letter
                {
                    allowed = false;
                }
                // Loose permits breaks *between* IN characters, rather than
                // making an ellipsis generally eligible to start a line.
                if current.class == BreakClass::Inseparable
                    && current.policy.line_break == LineBreak::Loose
                    && previous.class != BreakClass::Inseparable
                    && previous.class != BreakClass::Space
                {
                    allowed = false;
                }
            }
            let supplied_by_space = previous.character == ' ' && !previous.policy.preserves_spaces;
            // A disappearing space supplies its own opportunity, controlled
            // by that space's style rather than the adjacent boxes' ancestor.
            let wraps = if supplied_by_space {
                previous
                    .glyph
                    .is_some_and(|glyph| match wrap_overrides.get(glyph) {
                        WhitespaceWrapOverride::Style => previous.policy.wraps,
                        WhitespaceWrapOverride::Allow | WhitespaceWrapOverride::UnicodeAllow => {
                            true
                        }
                        WhitespaceWrapOverride::Suppress
                        | WhitespaceWrapOverride::UnicodeSuppress => false,
                    })
            } else if current.owner == previous.owner {
                current.policy.wraps
            } else {
                common_ancestor(tree, current.owner, previous.owner)
                    .is_some_and(|owner| get_style(styles, tree, owner).white_space().allows_wrap())
            };
            allowed &= boundary && wraps;
            // Collapsible spaces already supply a discardable opportunity.
            // An additional retained opportunity after one would prevent the
            // line-edge trimmer from reaching and discarding that space.
            wrap_overrides.set(glyph, allowed && !supplied_by_space);

            // A discarded space is represented by a break before that space
            // in the Latin path. In a Unicode paragraph its following context
            // can prohibit that break (e.g. before closing punctuation).
            if !allowed
                && previous.character == ' '
                && let Some(space) = previous.glyph
                && matches!(
                    wrap_overrides.get(space),
                    WhitespaceWrapOverride::Allow | WhitespaceWrapOverride::Style
                )
            {
                wrap_overrides.set(space, false);
            }
        }
    }
}

fn common_ancestor(
    tree: &(impl TextStructure + ?Sized),
    mut left: usize,
    mut right: usize,
) -> Option<usize> {
    let depth = |mut owner| {
        let mut depth = 0;
        while let Some(parent) = tree.get_box_parent(owner) {
            owner = parent;
            depth += 1;
        }
        depth
    };
    let left_depth = depth(left);
    let right_depth = depth(right);
    for _ in right_depth..left_depth {
        left = tree.get_box_parent(left)?;
    }
    for _ in left_depth..right_depth {
        right = tree.get_box_parent(right)?;
    }
    while left != right {
        left = tree.get_box_parent(left)?;
        right = tree.get_box_parent(right)?;
    }
    Some(left)
}

/// CSS gives atomic inlines ordinary wrap opportunities, but non-tailorable
/// Unicode controls and the boundary's containing white-space policy still
/// apply. NBSP is the explicit web-compatibility exception for atomic boxes.
pub fn atomic_boundary_wraps(
    styles: &ComputedStyles,
    tree: &(impl TextStructure + ?Sized),
    left: (usize, char),
    right: (usize, char),
) -> bool {
    let Some(owner) = common_ancestor(tree, left.0, right.0) else {
        return false;
    };
    if !get_style(styles, tree, owner).white_space().allows_wrap() {
        return false;
    }
    if [left.0, right.0]
        .into_iter()
        .any(|owner| get_style(styles, tree, owner).line_break() == LineBreak::Anywhere)
    {
        return true;
    }
    [left.1, right.1].into_iter().all(|character| {
        character == '\u{00a0}'
            || !matches!(
                class(character),
                BreakClass::NonBreakingGlue | BreakClass::WordJoiner | BreakClass::ZeroWidthJoiner
            )
    })
}

fn class(character: char) -> BreakClass {
    let class = break_property(character as u32);
    // Unicode 15's tables predate some Han extensions. Their assigned and
    // reserved ideographic ranges share ID behavior in newer Unicode data.
    if class == BreakClass::Unknown && matches!(character as u32, 0x20000..=0x3ffff) {
        BreakClass::Ideographic
    } else {
        class
    }
}

fn iteration_mark(character: char) -> bool {
    matches!(
        character,
        '\u{3005}' | '\u{303b}' | '\u{309d}' | '\u{309e}' | '\u{30fd}' | '\u{30fe}'
    )
}

fn centered_punctuation(character: char) -> bool {
    matches!(
        character,
        '\u{30fb}'
            | '\u{ff1a}'
            | '\u{ff1b}'
            | '\u{ff65}'
            | '\u{203c}'
            | '\u{2047}'
            | '\u{2048}'
            | '\u{2049}'
            | '\u{ff01}'
            | '\u{ff1f}'
    )
}

fn tailor(
    character: char,
    class: BreakClass,
    policy: Policy,
    previous: Option<&Character>,
) -> char {
    let loose = policy.line_break == LineBreak::Loose;
    if iteration_mark(character) {
        return if loose { '一' } else { 'ぁ' };
    }
    if matches!(character, '\u{301c}' | '\u{30a0}') {
        return if policy.chinese_or_japanese && policy.line_break != LineBreak::Strict {
            '一'
        } else {
            'ぁ'
        };
    }
    if loose {
        if matches!(
            class,
            BreakClass::ConditionalJapaneseStarter | BreakClass::Inseparable
        ) {
            return '一';
        }
        if policy.chinese_or_japanese
            && (centered_punctuation(character)
                || (matches!(class, BreakClass::Prefix | BreakClass::Postfix)
                    && character.width_cjk() == Some(2)))
        {
            return '一';
        }
        if matches!(character, '\u{2010}' | '\u{2013}')
            && previous.is_some_and(|previous| {
                previous.class == BreakClass::Ideographic
                    || (previous.policy.word_break == WordBreak::BreakAll && previous.letter)
            })
        {
            return '一';
        }
    }
    if class == BreakClass::Ideographic && break_property(character as u32) == BreakClass::Unknown {
        return '一';
    }
    if policy.word_break == WordBreak::BreakAll
        && matches!(
            class,
            BreakClass::Alphabetic
                | BreakClass::Ambiguous
                | BreakClass::Numeric
                | BreakClass::ComplexContext
                | BreakClass::HebrewLetter
        )
    {
        return '一';
    }
    character
}

pub fn resolve(
    styles: &ComputedStyles,
    tree: &(impl TextStructure + ?Sized),
    content: &mut InlineContent,
    roots: &FxHashSet<usize>,
) {
    if roots.is_empty() {
        return;
    }
    let mut paragraphs = FxHashMap::<usize, Paragraph>::default();
    for item in content.logical_flow_items() {
        let owner = item.box_idx as usize;
        let root = item.paragraph_root(tree);
        if !roots.contains(&root) {
            continue;
        }
        let paragraph = paragraphs.entry(root).or_default();
        let policy = Policy::new(get_style(styles, tree, owner), styles);
        match &item.kind {
            InlineItemKind::Text { glyphs } | InlineItemKind::Marker { glyphs } => {
                for glyph in glyphs.clone() {
                    let character = content
                        .character_at(glyph as usize)
                        .unwrap_or('\u{fffd}');
                    paragraph.push(owner, Some(glyph as usize), character, policy);
                }
            }
            InlineItemKind::Break { .. } => paragraph.push(owner, None, '\n', policy),
            _ => paragraph.push(owner, None, '\u{fffc}', policy),
        }
    }
    for paragraph in paragraphs.values() {
        paragraph.resolve(content, styles, tree);
    }
}
