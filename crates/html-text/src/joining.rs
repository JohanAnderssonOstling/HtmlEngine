//! Unicode cursive context shared by shaping and character spacing.

use icu_properties::{
    CodePointMapData,
    props::{JoiningType, Script},
    script::ScriptWithExtensions,
};

/// Whether the two logical text runs connect across their common boundary.
/// Transparent combining marks do not interrupt the joining characters.
pub fn joins_cursively(
    mut before: impl DoubleEndedIterator<Item = char>,
    mut after: impl Iterator<Item = char>,
) -> bool {
    let types = CodePointMapData::<JoiningType>::new();
    let left = before.rfind(|&character| types.get(character) != JoiningType::Transparent);
    let right = after.find(|&character| types.get(character) != JoiningType::Transparent);
    left.is_some_and(|character| {
        matches!(
            types.get(character),
            JoiningType::DualJoining | JoiningType::LeftJoining | JoiningType::JoinCausing
        )
    }) && right.is_some_and(|character| {
        matches!(
            types.get(character),
            JoiningType::DualJoining | JoiningType::RightJoining | JoiningType::JoinCausing
        )
    })
}

/// CSS cursive scripts cannot receive ordinary inter-letter gaps.
pub fn is_cursive_character(character: char) -> bool {
    // ASCII contains no cursive scripts and needs no Unicode table lookup.
    if character.is_ascii() {
        return false;
    }
    ScriptWithExtensions::new()
        .get_script_extensions_val(character)
        .iter()
        .any(|script| {
            matches!(
                script,
                Script::Arabic
                    | Script::HanifiRohingya
                    | Script::Mandaic
                    | Script::Mongolian
                    | Script::Nko
                    | Script::PhagsPa
                    | Script::Syriac
            )
        })
}
