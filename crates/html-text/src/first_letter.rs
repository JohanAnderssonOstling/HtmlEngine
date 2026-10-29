//! Character policy shared by floated first-letter construction and text preparation.

use std::ops::Range;
use unicode_categories::UnicodeCategories;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FirstLetterPart {
    /// Ignored by pseudo styling; formatting may retain it for whitespace collapse.
    LeadingSpace,
    Selected,
    After,
}

#[derive(Clone, Copy, Default)]
enum Phase {
    #[default]
    Leading,
    Punctuation,
    Letter,
    Trailing,
    Done,
}

/// Select one first-letter unit, including adjacent CSS punctuation and its
/// combining marks. State survives source-node and inline-element boundaries.
#[derive(Clone, Copy, Default)]
pub struct FirstLetterSelection {
    phase: Phase,
}

impl FirstLetterSelection {
    pub fn push(&mut self, character: char) -> FirstLetterPart {
        match self.phase {
            Phase::Leading if character.is_whitespace() => return FirstLetterPart::LeadingSpace,
            Phase::Leading if is_punctuation(character) => self.phase = Phase::Punctuation,
            Phase::Leading => self.phase = Phase::Letter,
            Phase::Punctuation if is_punctuation(character) || character.is_mark() => {}
            Phase::Punctuation if !character.is_whitespace() => self.phase = Phase::Letter,
            Phase::Letter if character.is_mark() => {}
            Phase::Letter if is_punctuation(character) => self.phase = Phase::Trailing,
            Phase::Trailing if is_punctuation(character) || character.is_mark() => {}
            _ => {
                self.phase = Phase::Done;
                return FirstLetterPart::After;
            }
        }
        FirstLetterPart::Selected
    }

    pub fn is_done(&self) -> bool {
        matches!(self.phase, Phase::Done)
    }
}

pub(crate) fn first_letter_range(characters: impl IntoIterator<Item = char>) -> Range<usize> {
    let mut selection = FirstLetterSelection::default();
    let mut start = None;
    let mut end = 0;
    for (index, character) in characters.into_iter().enumerate() {
        match selection.push(character) {
            FirstLetterPart::LeadingSpace => {}
            FirstLetterPart::Selected => {
                start.get_or_insert(index);
                end = index + 1;
            }
            FirstLetterPart::After => break,
        }
    }
    start.unwrap_or(end)..end
}

fn is_punctuation(character: char) -> bool {
    character.is_punctuation_open()
        || character.is_punctuation_close()
        || character.is_punctuation_initial_quote()
        || character.is_punctuation_final_quote()
        || character.is_punctuation_other()
}

#[cfg(test)]
mod tests {
    use super::first_letter_range;

    #[test]
    fn selects_punctuation_and_combining_sequences_without_crossing_spaces() {
        for (source, selected) in [
            ("", ""),
            ("  ", ""),
            ("  A rest", "A"),
            ("\u{201c}A\u{301}!\u{301}rest", "\u{201c}A\u{301}!\u{301}"),
            ("\u{201c}\u{301}A rest", "\u{201c}\u{301}A"),
            ("!?", "!?"),
            ("! A", "!"),
            ("A B", "A"),
        ] {
            let range = first_letter_range(source.chars());
            assert_eq!(
                source
                    .chars()
                    .skip(range.start)
                    .take(range.len())
                    .collect::<String>(),
                selected,
                "{source:?}"
            );
        }
    }
}
