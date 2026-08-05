//! Shared punctuation-aware glue policy for optimal line breaking and final
//! justified-space placement.

pub(super) const NORMAL_STRETCH_FRACTION: f64 = 0.5;
pub(super) const NORMAL_SHRINK_FRACTION: f64 = 0.33;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(u8)]
pub(super) enum SpaceGlueClass {
    #[default]
    Ordinary = 0,
    OpeningOrClosing = 1,
    Dash = 2,
    Sentence = 3,
    Clause = 4,
}

impl SpaceGlueClass {
    pub(super) fn from_packed(value: u32) -> Self {
        match value {
            1 => Self::OpeningOrClosing,
            2 => Self::Dash,
            3 => Self::Sentence,
            4 => Self::Clause,
            _ => Self::Ordinary,
        }
    }

    fn stretch_factor(self) -> f64 {
        match self {
            Self::Ordinary => 1.0,
            Self::OpeningOrClosing => 0.6,
            Self::Dash => 0.75,
            Self::Sentence => 1.25,
            Self::Clause => 1.1,
        }
    }

    fn shrink_factor(self) -> f64 {
        match self {
            Self::Ordinary => 1.0,
            Self::OpeningOrClosing => 0.6,
            Self::Dash => 0.9,
            Self::Sentence => 0.85,
            Self::Clause => 0.95,
        }
    }
}

pub(super) fn classify_space(previous: Option<char>, next: Option<char>) -> SpaceGlueClass {
    let adjacent_to_opening = previous.is_some_and(|ch| matches!(ch, '(' | '[' | '{' | '“' | '‘' | '«' | '‹'));
    let adjacent_to_closing = next.is_some_and(|ch| matches!(ch, ')' | ']' | '}' | '”' | '’' | '»' | '›' | '"' | '\''));
    if adjacent_to_opening || adjacent_to_closing {
        return SpaceGlueClass::OpeningOrClosing;
    }
    if previous.is_some_and(|ch| matches!(ch, '—' | '–')) || next.is_some_and(|ch| matches!(ch, '—' | '–')) {
        return SpaceGlueClass::Dash;
    }
    if previous.is_some_and(|ch| matches!(ch, '.' | '!' | '?' | '…' | '”' | '’' | '»' | '›' | '"' | '\'')) {
        return SpaceGlueClass::Sentence;
    }
    if previous.is_some_and(|ch| matches!(ch, ',' | ';' | ':')) {
        return SpaceGlueClass::Clause;
    }
    SpaceGlueClass::Ordinary
}

pub(super) fn glue_capacities(space_width: f64, class: SpaceGlueClass, punctuation_aware: bool) -> (f64, f64) {
    let width = space_width.max(0.0);
    let class = if punctuation_aware { class } else { SpaceGlueClass::Ordinary };
    (width * NORMAL_STRETCH_FRACTION * class.stretch_factor(), width * NORMAL_SHRINK_FRACTION * class.shrink_factor())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn punctuation_classes_have_bounded_distinct_glue_capacities() {
        let ordinary = glue_capacities(10.0, SpaceGlueClass::Ordinary, true);
        let clause = glue_capacities(10.0, SpaceGlueClass::Clause, true);
        let dash = glue_capacities(10.0, SpaceGlueClass::Dash, true);

        assert!(clause.0 > ordinary.0);
        assert!(dash.0 < ordinary.0);
        assert!(dash.1 < ordinary.1);
        assert_eq!(glue_capacities(10.0, SpaceGlueClass::Clause, false), ordinary, "web-compatible glue ignores punctuation classes");
    }
}
