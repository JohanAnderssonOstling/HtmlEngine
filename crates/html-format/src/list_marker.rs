use html_style_model::{CounterStyle, ListStyleType};
use std::fmt::Write;

/// The text of a list item's marker: a bullet glyph for the symbolic types, or
/// the counter value followed by its `.` suffix for the numeric/alphabetic
/// types. `None` for `list-style-type: none`.
///
/// The spacing between marker and content is supplied by layout (the outside
/// hang gap, or the inline advance for inside markers), not by this string —
/// matching how browsers position the `::marker` box rather than padding text.
pub fn marker_text(list_style_type: ListStyleType, ordinal: i64) -> Option<String> {
    let style = match list_style_type {
        ListStyleType::None => return None,
        ListStyleType::Disc => CounterStyle::Disc,
        ListStyleType::Circle => CounterStyle::Circle,
        ListStyleType::Square => CounterStyle::Square,
        ListStyleType::Decimal => CounterStyle::Decimal,
        ListStyleType::DecimalLeadingZero => CounterStyle::DecimalLeadingZero,
        ListStyleType::LowerAlpha => CounterStyle::LowerAlpha,
        ListStyleType::UpperAlpha => CounterStyle::UpperAlpha,
        ListStyleType::LowerRoman => CounterStyle::LowerRoman,
        ListStyleType::UpperRoman => CounterStyle::UpperRoman,
    };
    let mut text = counter_text(style, ordinal);
    if !list_style_type.is_bullet() {
        text.push('.');
    }
    Some(text)
}

pub fn counter_text(style: CounterStyle, value: i64) -> String {
    let mut output = String::new();
    append_counter(style, value, &mut output);
    output
}

pub(super) fn append_counter(style: CounterStyle, value: i64, output: &mut String) {
    match style {
        CounterStyle::Decimal => decimal(value, output),
        CounterStyle::DecimalLeadingZero => decimal_leading_zero(value, output),
        CounterStyle::LowerRoman => roman(value, false, output),
        CounterStyle::UpperRoman => roman(value, true, output),
        CounterStyle::LowerGreek => alphabetic_chars(
            value,
            &[
                'α', 'β', 'γ', 'δ', 'ε', 'ζ', 'η', 'θ', 'ι', 'κ', 'λ', 'μ', 'ν', 'ξ', 'ο', 'π',
                'ρ', 'σ', 'τ', 'υ', 'φ', 'χ', 'ψ', 'ω',
            ],
            output,
        ),
        CounterStyle::LowerAlpha => alphabetic(value, false, output),
        CounterStyle::UpperAlpha => alphabetic(value, true, output),
        CounterStyle::Armenian => additive(value, &ARMENIAN_NUMERALS, output),
        CounterStyle::Georgian => additive(value, &GEORGIAN_NUMERALS, output),
        CounterStyle::Disc => output.push('\u{2022}'),
        CounterStyle::Circle => output.push('\u{25e6}'),
        CounterStyle::Square => output.push('\u{25aa}'),
    }
}

fn decimal(n: i64, output: &mut String) {
    write!(output, "{n}").expect("writing to a string cannot fail");
}

/// `decimal-leading-zero`: pad to at least two digits (CSS pads to the width of
/// the list's largest value, but two-digit padding matches the common case and
/// every UA for n < 100).
fn decimal_leading_zero(n: i64, output: &mut String) {
    if n < 0 {
        output.push('-');
    }
    write!(output, "{:02}", n.unsigned_abs()).expect("writing to a string cannot fail");
}

/// Bijective base-26 (`a`..`z`, `aa`, `ab`, …). Defined only for n >= 1; other
/// values fall back to `decimal` per CSS counter-style fallback.
fn alphabetic(n: i64, upper: bool, output: &mut String) {
    if n < 1 {
        return decimal(n, output);
    }
    let base = if upper { b'A' } else { b'a' };
    alphabetic_digits(n as u64, 26, &mut |digit| {
        output.push((base + digit as u8) as char)
    });
}

fn alphabetic_chars(n: i64, alphabet: &[char], output: &mut String) {
    if n < 1 {
        return decimal(n, output);
    }
    alphabetic_digits(n as u64, alphabet.len() as u64, &mut |digit| {
        output.push(alphabet[digit])
    });
}

/// Emit bijective-base digits from most to least significant without a reversal buffer.
fn alphabetic_digits(value: u64, radix: u64, emit: &mut impl FnMut(usize)) {
    if value > 0 {
        let value = value - 1;
        alphabetic_digits(value / radix, radix, emit);
        emit((value % radix) as usize);
    }
}

fn additive(n: i64, symbols: &[(i64, char)], output: &mut String) {
    if !(1..=19_999).contains(&n) {
        return decimal(n, output);
    }
    additive_symbols(n, symbols, |symbol| output.push(symbol));
}

fn additive_symbols<T: Copy>(mut value: i64, symbols: &[(i64, T)], mut emit: impl FnMut(T)) {
    for &(weight, symbol) in symbols {
        while value >= weight {
            emit(symbol);
            value -= weight;
        }
    }
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
fn roman(n: i64, upper: bool, output: &mut String) {
    if !(1..=3999).contains(&n) {
        return decimal(n, output);
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
    additive_symbols(n, &TABLE, |numeral| {
        if upper {
            output.extend(
                numeral
                    .chars()
                    .map(|character| character.to_ascii_uppercase()),
            );
        } else {
            output.push_str(numeral);
        }
    });
}

#[cfg(test)]
mod marker_text_tests {
    use super::{append_counter, counter_text, marker_text};
    use html_style_model::CounterStyle;
    use html_style_model::ListStyleType::*;

    #[test]
    fn counter_writer_appends_without_temporary_allocations() {
        use crate::allocation_test_support::count_allocations;
        for (style, value, expected) in [
            (CounterStyle::Decimal, i64::MIN, "-9223372036854775808"),
            (CounterStyle::DecimalLeadingZero, -1, "-01"),
            (CounterStyle::DecimalLeadingZero, 0, "00"),
            (CounterStyle::LowerRoman, 49, "xlix"),
            (CounterStyle::UpperRoman, 3999, "MMMCMXCIX"),
            (CounterStyle::UpperRoman, 4000, "4000"),
            (CounterStyle::LowerAlpha, 703, "aaa"),
            (CounterStyle::UpperAlpha, 703, "AAA"),
            (CounterStyle::LowerGreek, 601, "ααα"),
            (CounterStyle::Armenian, 2024, "ՍԻԴ"),
            (CounterStyle::Georgian, 19_999, "ჵჰშჟთ"),
            (CounterStyle::Georgian, 20_000, "20000"),
            (CounterStyle::Disc, 1, "•"),
            (CounterStyle::Circle, 1, "◦"),
            (CounterStyle::Square, 1, "▪"),
        ] {
            let mut output = String::with_capacity(128);
            output.push_str("前:");
            let (_, allocations) = count_allocations(|| append_counter(style, value, &mut output));
            assert_eq!(output, format!("前:{expected}"));
            assert_eq!(allocations, 0, "{style:?} {value}");
        }
    }

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
        assert_eq!(marker_text(LowerAlpha, 702).as_deref(), Some("zz."));
        assert_eq!(marker_text(UpperAlpha, 703).as_deref(), Some("AAA."));
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
        assert_eq!(counter_text(CounterStyle::LowerGreek, 24), "ω");
        assert_eq!(counter_text(CounterStyle::LowerGreek, 25), "αα");
        assert_eq!(counter_text(CounterStyle::LowerGreek, 600), "ωω");
        assert_eq!(counter_text(CounterStyle::LowerGreek, 601), "ααα");
        assert_eq!(counter_text(CounterStyle::Armenian, 2024), "ՍԻԴ");
        assert_eq!(counter_text(CounterStyle::Georgian, 400), "ჳ");
        assert_eq!(counter_text(CounterStyle::Georgian, 19_999), "ჵჰშჟთ");
    }
}
