use html_style_model::GridAutoFlow;

pub(in crate::style::cascade::resolver) fn grid_auto_flow(
    value: lightningcss::properties::grid::GridAutoFlow,
) -> GridAutoFlow {
    use lightningcss::properties::grid::GridAutoFlow as Lc;
    match (value.contains(Lc::Column), value.contains(Lc::Dense)) {
        (false, false) => GridAutoFlow::Row,
        (true, false) => GridAutoFlow::Column,
        (false, true) => GridAutoFlow::RowDense,
        (true, true) => GridAutoFlow::ColumnDense,
    }
}
