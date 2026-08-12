use html_style_model::{CounterStyle, ListStyleType};

/// Bullet glyph used for the symbolic list-style-types.
fn bullet_char(list_style_type: ListStyleType) -> Option<char> {
    match list_style_type {
        ListStyleType::Disc => Some('\u{2022}'),   // •
        ListStyleType::Circle => Some('\u{25E6}'), // ◦
        ListStyleType::Square => Some('\u{25AA}'), // ▪
        _ => None,
    }
}

/// The text of a list item's marker: a bullet glyph for the symbolic types, or
/// the counter value followed by its `.` suffix for the numeric/alphabetic
/// types. `None` for `list-style-type: none`.
///
/// The spacing between marker and content is supplied by layout (the outside
/// hang gap, or the inline advance for inside markers), not by this string —
/// matching how browsers position the `::marker` box rather than padding text.
pub(crate) fn marker_text(list_style_type: ListStyleType, ordinal: i64) -> Option<String> {
    if let Some(bullet) = bullet_char(list_style_type) {
        return Some(bullet.to_string());
    }
    let body = match list_style_type {
        ListStyleType::None => return None,
        ListStyleType::Disc | ListStyleType::Circle | ListStyleType::Square => {
            unreachable!("handled by bullet_char")
        }
        ListStyleType::Decimal => decimal(ordinal),
        ListStyleType::DecimalLeadingZero => decimal_leading_zero(ordinal),
        ListStyleType::LowerAlpha => alphabetic(ordinal, false),
        ListStyleType::UpperAlpha => alphabetic(ordinal, true),
        ListStyleType::LowerRoman => roman(ordinal, false),
        ListStyleType::UpperRoman => roman(ordinal, true),
    };
    Some(format!("{body}."))
}

pub(crate) fn counter_text(style: CounterStyle, value: i64) -> String {
    match style {
        CounterStyle::Decimal => decimal(value),
        CounterStyle::DecimalLeadingZero => decimal_leading_zero(value),
        CounterStyle::LowerRoman => roman(value, false),
        CounterStyle::UpperRoman => roman(value, true),
        CounterStyle::LowerGreek => alphabetic_chars(
            value,
            &[
                'α', 'β', 'γ', 'δ', 'ε', 'ζ', 'η', 'θ', 'ι', 'κ', 'λ', 'μ', 'ν', 'ξ', 'ο', 'π',
                'ρ', 'σ', 'τ', 'υ', 'φ', 'χ', 'ψ', 'ω',
            ],
        ),
        CounterStyle::LowerAlpha => alphabetic(value, false),
        CounterStyle::UpperAlpha => alphabetic(value, true),
        CounterStyle::Armenian => additive(value, &ARMENIAN_NUMERALS),
        CounterStyle::Georgian => additive(value, &GEORGIAN_NUMERALS),
        CounterStyle::Disc => "\u{2022}".to_owned(),
        CounterStyle::Circle => "\u{25e6}".to_owned(),
        CounterStyle::Square => "\u{25aa}".to_owned(),
    }
}

fn decimal(n: i64) -> String {
    n.to_string()
}

/// `decimal-leading-zero`: pad to at least two digits (CSS pads to the width of
/// the list's largest value, but two-digit padding matches the common case and
/// every UA for n < 100).
fn decimal_leading_zero(n: i64) -> String {
    if n < 0 {
        return format!("-{:02}", n.unsigned_abs());
    }
    format!("{n:02}")
}

/// Bijective base-26 (`a`..`z`, `aa`, `ab`, …). Defined only for n >= 1; other
/// values fall back to `decimal` per CSS counter-style fallback.
fn alphabetic(n: i64, upper: bool) -> String {
    if n < 1 {
        return decimal(n);
    }
    let base = if upper { b'A' } else { b'a' };
    let mut value = n as u64;
    let mut letters = Vec::new();
    while value > 0 {
        value -= 1;
        letters.push(base + (value % 26) as u8);
        value /= 26;
    }
    letters.reverse();
    String::from_utf8(letters).expect("ascii letters")
}

fn alphabetic_chars(n: i64, alphabet: &[char]) -> String {
    if n < 1 {
        return decimal(n);
    }
    let mut value = n as u64;
    let radix = alphabet.len() as u64;
    let mut output = Vec::new();
    while value > 0 {
        value -= 1;
        output.push(alphabet[(value % radix) as usize]);
        value /= radix;
    }
    output.into_iter().rev().collect()
}

fn additive(n: i64, symbols: &[(i64, char)]) -> String {
    if !(1..=19_999).contains(&n) {
        return decimal(n);
    }
    let mut remaining = n;
    let mut output = String::new();
    for &(value, symbol) in symbols {
        while remaining >= value {
            output.push(symbol);
            remaining -= value;
        }
    }
    output
}

const ARMENIAN_NUMERALS: [(i64, char); 36] = [
    (9000, 'Ք'),
    (8000, 'Փ'),
    (7000, 'Ւ'),
    (6000, 'Ց'),
    (5000, 'Ր'),
    (4000, 'Տ'),
    (3000, 'Վ'),
    (2000, 'Ս'),
    (1000, 'Ռ'),
    (900, 'Ջ'),
    (800, 'Պ'),
    (700, 'Չ'),
    (600, 'Ո'),
    (500, 'Շ'),
    (400, 'Ն'),
    (300, 'Յ'),
    (200, 'Մ'),
    (100, 'Ճ'),
    (90, 'Ղ'),
    (80, 'Ձ'),
    (70, 'Հ'),
    (60, 'Կ'),
    (50, 'Ծ'),
    (40, 'Խ'),
    (30, 'Լ'),
    (20, 'Ի'),
    (10, 'Ժ'),
    (9, 'Թ'),
    (8, 'Ը'),
    (7, 'Է'),
    (6, 'Զ'),
    (5, 'Ե'),
    (4, 'Դ'),
    (3, 'Գ'),
    (2, 'Բ'),
    (1, 'Ա'),
];

const GEORGIAN_NUMERALS: [(i64, char); 37] = [
    (10_000, 'ჵ'),
    (9000, 'ჰ'),
    (8000, 'ჯ'),
    (7000, 'ჴ'),
    (6000, 'ხ'),
    (5000, 'ჭ'),
    (4000, 'წ'),
    (3000, 'ძ'),
    (2000, 'ც'),
    (1000, 'ჩ'),
    (900, 'შ'),
    (800, 'ყ'),
    (700, 'ღ'),
    (600, 'ქ'),
    (500, 'ფ'),
    (400, 'ჳ'),
    (300, 'ტ'),
    (200, 'ს'),
    (100, 'რ'),
    (90, 'ჟ'),
    (80, 'პ'),
    (70, 'ო'),
    (60, 'ჲ'),
    (50, 'ნ'),
    (40, 'მ'),
    (30, 'ლ'),
    (20, 'კ'),
    (10, 'ი'),
    (9, 'თ'),
    (8, 'ჱ'),
    (7, 'ზ'),
    (6, 'ვ'),
    (5, 'ე'),
    (4, 'დ'),
    (3, 'გ'),
    (2, 'ბ'),
    (1, 'ა'),
];

/// Roman numerals, defined for 1..=3999; other values fall back to `decimal`.
fn roman(n: i64, upper: bool) -> String {
    if !(1..=3999).contains(&n) {
        return decimal(n);
    }
    const TABLE: [(i64, &str); 13] = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut remaining = n;
    let mut out = String::new();
    for (value, numeral) in TABLE {
        while remaining >= value {
            out.push_str(numeral);
            remaining -= value;
        }
    }
    if upper { out.to_uppercase() } else { out }
}

#[cfg(test)]
mod marker_text_tests {
    use super::{counter_text, marker_text};
    use html_style_model::CounterStyle;
    use html_style_model::ListStyleType::*;

    #[test]
    fn bullets_ignore_ordinal() {
        assert_eq!(marker_text(Disc, 7).as_deref(), Some("\u{2022}"));
        assert_eq!(marker_text(Circle, 1).as_deref(), Some("\u{25E6}"));
        assert_eq!(marker_text(Square, 99).as_deref(), Some("\u{25AA}"));
    }

    #[test]
    fn none_has_no_marker() {
        assert_eq!(marker_text(None, 3), Option::None);
    }

    #[test]
    fn decimal_and_leading_zero() {
        assert_eq!(marker_text(Decimal, 1).as_deref(), Some("1."));
        assert_eq!(marker_text(Decimal, 42).as_deref(), Some("42."));
        assert_eq!(marker_text(DecimalLeadingZero, 1).as_deref(), Some("01."));
        assert_eq!(marker_text(DecimalLeadingZero, 9).as_deref(), Some("09."));
        assert_eq!(marker_text(DecimalLeadingZero, 10).as_deref(), Some("10."));
    }

    #[test]
    fn alphabetic_is_bijective_base_26() {
        assert_eq!(marker_text(LowerAlpha, 1).as_deref(), Some("a."));
        assert_eq!(marker_text(LowerAlpha, 26).as_deref(), Some("z."));
        assert_eq!(marker_text(LowerAlpha, 27).as_deref(), Some("aa."));
        assert_eq!(marker_text(LowerAlpha, 52).as_deref(), Some("az."));
        assert_eq!(marker_text(UpperAlpha, 28).as_deref(), Some("AB."));
        // Out of range falls back to decimal.
        assert_eq!(marker_text(LowerAlpha, 0).as_deref(), Some("0."));
    }

    #[test]
    fn roman_numerals() {
        assert_eq!(marker_text(LowerRoman, 1).as_deref(), Some("i."));
        assert_eq!(marker_text(LowerRoman, 4).as_deref(), Some("iv."));
        assert_eq!(marker_text(LowerRoman, 9).as_deref(), Some("ix."));
        assert_eq!(marker_text(LowerRoman, 49).as_deref(), Some("xlix."));
        assert_eq!(marker_text(UpperRoman, 2024).as_deref(), Some("MMXXIV."));
        assert_eq!(marker_text(UpperRoman, 3999).as_deref(), Some("MMMCMXCIX."));
        // Out of range falls back to decimal.
        assert_eq!(marker_text(LowerRoman, 4000).as_deref(), Some("4000."));
    }

    #[test]
    fn generated_counter_formats_cover_css2_additive_and_alphabetic_styles() {
        assert_eq!(counter_text(CounterStyle::LowerGreek, 25), "αα");
        assert_eq!(counter_text(CounterStyle::Armenian, 2024), "ՍԻԴ");
        assert_eq!(counter_text(CounterStyle::Georgian, 400), "ჳ");
        assert_eq!(counter_text(CounterStyle::Georgian, 19_999), "ჵჰშჟთ");
    }
}
