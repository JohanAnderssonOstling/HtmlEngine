use html_engine_model::DecorationPattern;
use html_engine_model::paint_program::BorderSide;
use html_style_model::{BorderStyle, UsedStyleView};

#[derive(Clone, Copy)]
pub struct PhysicalBorderEdge {
    pub style: BorderStyle,
    pub width: f64,
    pub color: u32,
    pub side: BorderSide,
}

impl PhysicalBorderEdge {
    pub fn solid(self) -> Option<(f64, u32)> {
        matches!(self.style, BorderStyle::Solid).then_some((self.width, self.color))
    }
}

#[derive(Clone, Copy)]
pub struct PhysicalBorders {
    pub top: PhysicalBorderEdge,
    pub right: PhysicalBorderEdge,
    pub bottom: PhysicalBorderEdge,
    pub left: PhysicalBorderEdge,
}

impl PhysicalBorders {
    pub fn solid_edges(self, start: bool, end: bool) -> [Option<(f64, u32)>; 4] {
        [
            self.top.solid(),
            end.then(|| self.right.solid()).flatten(),
            self.bottom.solid(),
            start.then(|| self.left.solid()).flatten(),
        ]
    }

    pub fn uniform(self) -> Option<PhysicalBorderEdge> {
        (self.top.width > 0.0
            && [self.right, self.bottom, self.left]
                .into_iter()
                .all(|edge| {
                    edge.style == self.top.style
                        && edge.width == self.top.width
                        && edge.color == self.top.color
                }))
        .then_some(self.top)
    }
}

pub fn physical_borders(style: &UsedStyleView<'_>) -> PhysicalBorders {
    PhysicalBorders {
        top: PhysicalBorderEdge {
            side: BorderSide::Top,
            style: style.border_top_style(),
            width: style.border_top_width() as f64,
            color: style.border_top_color(),
        },
        right: PhysicalBorderEdge {
            side: BorderSide::Right,
            style: style.border_right_style(),
            width: style.border_right_width() as f64,
            color: style.border_right_color(),
        },
        bottom: PhysicalBorderEdge {
            side: BorderSide::Bottom,
            style: style.border_bottom_style(),
            width: style.border_bottom_width() as f64,
            color: style.border_bottom_color(),
        },
        left: PhysicalBorderEdge {
            side: BorderSide::Left,
            style: style.border_left_style(),
            width: style.border_left_width() as f64,
            color: style.border_left_color(),
        },
    }
}

pub fn border_pattern(style: BorderStyle, horizontal: bool) -> Option<DecorationPattern> {
    match (style, horizontal) {
        (BorderStyle::Solid, _) => Some(DecorationPattern::Solid),
        (BorderStyle::Dotted, true) => Some(DecorationPattern::DottedHorizontal),
        (BorderStyle::Dashed, true) => Some(DecorationPattern::DashedHorizontal),
        (BorderStyle::Dotted, false) => Some(DecorationPattern::DottedVertical),
        (BorderStyle::Dashed, false) => Some(DecorationPattern::DashedVertical),
        (BorderStyle::Groove | BorderStyle::Ridge, _) => Some(DecorationPattern::Solid),
        (BorderStyle::None | BorderStyle::Hidden, _) => None,
    }
}
