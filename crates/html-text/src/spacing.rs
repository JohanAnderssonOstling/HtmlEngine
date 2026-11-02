//! Character spacing policy shared by shaping, line layout, and paint queries.

use crate::{InlineContent, is_bidi_control, is_cursive_character};
use unicode_categories::UnicodeCategories;

/// CSS space separators other than ordinary and nonbreaking word spaces.
pub fn is_other_space_separator(character: char) -> bool {
    // Space and NBSP are the only space separators through U+00A0.
    character > '\u{00a0}' && character.is_separator_space()
}

/// Characters whose primary purpose is to separate words (CSS Text 8.1).
/// Fixed-width typographic spaces do not receive authored word spacing.
pub fn is_word_separator(character: char) -> bool {
    matches!(
        character,
        ' ' | '\u{00a0}' | '\u{1361}' | '\u{10100}' | '\u{10101}' | '\u{1039f}' | '\u{1091f}'
    )
}

/// Zero-width format controls and soft hyphens retain their source positions
/// and shaping effects, but contribute no ordinary advance or ink. Chosen
/// hyphens are painted as separate synthetic fragments.
pub fn suppresses_glyph(character: char) -> bool {
    matches!(character, '\u{00ad}' | '\u{200b}'..='\u{200d}'
        | '\u{2060}'..='\u{2064}' | '\u{206a}'..='\u{206f}' | '\u{feff}')
        || is_bidi_control(character)
}

/// Character eligibility for tracking at an otherwise valid cluster boundary.
/// No tracking follows a control or adjoins a discretionary hyphen.
pub fn tracking_boundary_allowed(current: char, next: char) -> bool {
    !suppresses_glyph(current)
        && current != '\n'
        && !is_cursive_character(current)
        && next != '\u{00ad}'
}

/// Whether a character boundary inside this line can receive letter spacing.
/// The caller supplies the authoritative shaping-cluster boundary decision.
pub fn tracking_boundary_after(
    content: &InlineContent,
    index: u32,
    end: u32,
    boundary: bool,
) -> bool {
    index + 1 < end
        && boundary
        && content
            .character_at(index as usize)
            .zip(content.character_at(index as usize + 1))
            .is_some_and(|(current, next)| tracking_boundary_allowed(current, next))
}

/// Apply line spacing to a natural character advance. Preserved tabs and other
/// position-dependent overrides are resolved by the caller before this step.
pub fn spaced_advance(
    natural: f64,
    character: char,
    tracking: bool,
    word_spacing: f64,
    letter_spacing: f64,
) -> f64 {
    natural
        + if tracking { letter_spacing } else { 0.0 }
        + if is_word_separator(character) { word_spacing } else { 0.0 }
}
