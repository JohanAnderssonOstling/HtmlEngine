use super::*;

pub(in crate::style::cascade::resolver) fn css_color_to_u32(
    color: &CssColor,
    current_color: u32,
) -> u32 {
    match color {
        CssColor::RGBA(rgba) => rgba_to_u32(rgba),
        CssColor::CurrentColor => current_color,
        CssColor::System(system) => system_color_to_u32(system),
        // This renderer has a fixed light color scheme. Conversion is paid
        // only for authored non-sRGB colors; the common RGBA path above stays
        // a direct integer pack.
        CssColor::LightDark(light, _) => css_color_to_u32(light, current_color),
        other => match other.to_rgb() {
            Ok(CssColor::RGBA(rgba)) => rgba_to_u32(&rgba),
            Ok(CssColor::LightDark(light, _)) => css_color_to_u32(&light, current_color),
            _ => 0x000000FF,
        },
    }
}

pub(in crate::style::cascade::resolver) fn rgba_to_u32(
    rgba: &lightningcss::values::color::RGBA,
) -> u32 {
    ((rgba.red as u32) << 24)
        | ((rgba.green as u32) << 16)
        | ((rgba.blue as u32) << 8)
        | (rgba.alpha as u32)
}

/// Resolve CSS system colors to fixed light-theme values (the renderer has no
/// OS theme to consult). Unlisted ones are text colors and fall back to black.
pub(in crate::style::cascade::resolver) fn system_color_to_u32(color: &SystemColor) -> u32 {
    use SystemColor as S;
    match color {
        S::Mark => 0xFFFF00FF,
        S::Canvas
        | S::Field
        | S::Window
        | S::InfoBackground
        | S::Menu
        | S::ButtonFace
        | S::Scrollbar
        | S::AppWorkspace
        | S::ActiveCaption
        | S::Background
        | S::ButtonHighlight
        | S::ButtonShadow
        | S::InactiveCaption
        | S::ThreeDFace => 0xFFFFFFFF,
        S::LinkText => 0x0000EEFF,
        S::VisitedText => 0x551A8BFF,
        S::ActiveText => 0xEE0000FF,
        S::GrayText | S::InactiveCaptionText => 0x808080FF,
        S::Highlight | S::SelectedItem | S::AccentColor => 0x0078D7FF,
        S::HighlightText | S::SelectedItemText | S::AccentColorText => 0xFFFFFFFF,
        _ => 0x000000FF,
    }
}
