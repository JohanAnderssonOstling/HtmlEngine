//! Character spacing policy shared by shaping, line layout, and paint queries.

use crate::{InlineContent, is_bidi_control};

/// Soft hyphens and bidi controls stay in logical source but their ordinary
/// glyphs contribute neither advance nor ink. Chosen hyphens are painted as
/// separate synthetic fragments.
pub fn suppresses_glyph(character: char) -> bool {
    character == '\u{00ad}' || is_bidi_control(character)
}

/// Character eligibility for tracking at an otherwise valid cluster boundary.
/// No tracking follows a control or adjoins a discretionary hyphen.
pub fn tracking_boundary_allowed(current: char, next: char) -> bool {
    !suppresses_glyph(current) && next != '\u{00ad}'
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
        + if character == ' ' { word_spacing } else { 0.0 }
}
