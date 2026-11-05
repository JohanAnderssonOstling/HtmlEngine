/// Collapse Unicode whitespace to single ASCII spaces, trimming both ends.
/// This is plain-text normalization, independent of CSS whitespace rules.
pub fn collapse_whitespace(text: &str) -> String {
    let words = text.split_whitespace();
    // Size the result without retaining words or reserving discarded whitespace.
    let length = words.clone().fold(0, |length, word| {
        length + word.len() + usize::from(length != 0)
    });
    let mut output = String::with_capacity(length);
    for word in words {
        if !output.is_empty() {
            output.push(' ');
        }
        output.push_str(word);
    }
    output
}
