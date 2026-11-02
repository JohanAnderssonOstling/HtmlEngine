//! Append escaped markup into a caller-owned output buffer.

/// Escape HTML text (`&`, `<`, and `>`), retaining literal quotes.
#[inline]
pub fn write_escaped_html_text(output: &mut String, value: &str) {
    write_escaped::<false, false>(output, value);
}

/// Escape HTML markup and double quotes for quoted attribute values.
#[inline]
pub fn write_escaped_html_attribute(output: &mut String, value: &str) {
    write_escaped::<true, false>(output, value);
}

/// Escape XML markup, double quotes, and apostrophes.
#[inline]
pub fn write_escaped_xml(output: &mut String, value: &str) {
    write_escaped::<true, true>(output, value);
}

fn write_escaped<const QUOTES: bool, const APOSTROPHES: bool>(output: &mut String, value: &str) {
    let mut start = 0;
    for (index, byte) in value.bytes().enumerate() {
        let escaped = match byte {
            b'&' => "&amp;",
            b'<' => "&lt;",
            b'>' => "&gt;",
            b'"' if QUOTES => "&quot;",
            b'\'' if APOSTROPHES => "&apos;",
            _ => continue,
        };
        output.push_str(&value[start..index]);
        output.push_str(escaped);
        start = index + 1;
    }
    output.push_str(&value[start..]);
}

#[cfg(test)]
mod tests {
    use super::{write_escaped_html_attribute, write_escaped_html_text, write_escaped_xml};

    #[test]
    fn escaping_preserves_unicode_prefixes_and_each_quote_policy() {
        let cases: [(fn(&mut String, &str), &str); 3] = [
            (
                write_escaped_html_text,
                "café 🙂 &lt;&amp;&gt; \"' &amp;amp;",
            ),
            (
                write_escaped_html_attribute,
                "café 🙂 &lt;&amp;&gt; &quot;' &amp;amp;",
            ),
            (
                write_escaped_xml,
                "café 🙂 &lt;&amp;&gt; &quot;&apos; &amp;amp;",
            ),
        ];
        for (write, expected) in cases {
            let mut output = String::with_capacity(256);
            output.push_str("prefix:");
            let pointer = output.as_ptr();
            write(&mut output, "café 🙂 <&> \"' &amp;");
            assert_eq!(output, format!("prefix:{expected}"));
            assert_eq!(output.as_ptr(), pointer);
            write(&mut output, "");
            write(&mut output, "漢字");
            assert!(output.ends_with("漢字"));
        }
    }
}
