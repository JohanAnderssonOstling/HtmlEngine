pub(in crate::style::cascade::resolver) fn map_list_style_position(
    pos: &lightningcss::properties::list::ListStylePosition,
) -> html_style_model::ListStylePosition {
    use html_style_model::ListStylePosition as Out;
    use lightningcss::properties::list::ListStylePosition as Lc;
    match pos {
        Lc::Inside => Out::Inside,
        Lc::Outside => Out::Outside,
    }
}
