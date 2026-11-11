use html_style_model::BorderStyle;
use lightningcss::properties::border::LineStyle;

pub(in crate::style::cascade::resolver) fn line_style_to_border_style(
    style: &LineStyle,
) -> BorderStyle {
    match style {
        LineStyle::None => BorderStyle::None,
        LineStyle::Hidden => BorderStyle::Hidden,
        LineStyle::Solid => BorderStyle::Solid,
        LineStyle::Dashed => BorderStyle::Dashed,
        LineStyle::Dotted => BorderStyle::Dotted,
        LineStyle::Groove => BorderStyle::Groove,
        LineStyle::Ridge => BorderStyle::Ridge,
        // The 3D styles (and `double`) paint as a plain line rather than
        // disappearing; only `none`/`hidden` suppress the border.
        LineStyle::Double | LineStyle::Inset | LineStyle::Outset => BorderStyle::Solid,
    }
}
