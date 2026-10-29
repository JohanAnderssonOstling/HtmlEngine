//! Decoration inputs retained after line breaking and placement have finished.

use crate::{DecorationPattern, OverflowClip};
use html_dom::MemoryUsageReport;
use html_style_model::StyleIndices;
use kurbo::{Rect, Vec2};

#[derive(Clone, Copy, Debug)]
pub enum PaintStyleSource {
    Box(u32),
    Style { indices: StyleIndices, context: u32 },
}

#[derive(Clone, Copy)]
pub enum PaintOperation {
    Background {
        owner: u32,
        rect: Rect,
    },
    BorderOutline {
        owner: u32,
        rect: Rect,
    },
    BackgroundRect {
        owner: u32,
        source: PaintStyleSource,
        rect: Rect,
    },
    CollapsedBorder {
        owner: u32,
        source: PaintStyleSource,
        side: u8,
        pattern: DecorationPattern,
        rect: Rect,
    },
}

impl PaintOperation {
    fn translate(&mut self, offset: Vec2) {
        let (Self::Background { rect, .. }
        | Self::BorderOutline { rect, .. }
        | Self::BackgroundRect { rect, .. }
        | Self::CollapsedBorder { rect, .. }) = self;
        *rect = *rect + offset;
    }
}

#[derive(Clone, Copy)]
pub struct PaintCommand {
    pub operation: PaintOperation,
    pub layers: PaintLayers,
    /// Stable line ID, or `u32::MAX` for a line-independent operation.
    pub line: u32,
    pub paint_order: u32,
    group: u32,
}

const POSITIONED: u8 = 1;
const NEGATIVE: u8 = 2;
const INDEPENDENT: u8 = 4;

/// Shared layer flags for retained commands, published lines and decorations.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PaintLayers(u8);

impl PaintLayers {
    pub fn positioned(negative: bool, independent: bool) -> Self {
        Self(
            POSITIONED
                | if negative { NEGATIVE } else { 0 }
                | if independent { INDEPENDENT } else { 0 },
        )
    }

    pub fn include(&mut self, layers: Self) {
        self.0 |= layers.0;
    }

    pub fn is_positioned(self) -> bool {
        self.0 & POSITIONED != 0
    }

    pub fn is_negative(self) -> bool {
        self.0 & NEGATIVE != 0
    }

    pub fn is_independent(self) -> bool {
        self.0 & INDEPENDENT != 0
    }

    pub fn for_decoration(self, foreground: bool) -> Self {
        Self(self.0 & if foreground { !INDEPENDENT } else { u8::MAX })
    }
}

#[derive(Clone, Default)]
pub struct PaintProgram {
    pub commands: Vec<PaintCommand>,
    /// Geometry-owned overflow resolution. Paint only selects the retained
    /// parent clip for each new primitive; it never rebuilds line clipping.
    pub content_clips: Vec<Option<OverflowClip>>,
}

impl PaintProgram {
    pub fn clear(&mut self) {
        self.commands.clear();
        self.content_clips.clear();
    }
    pub fn record(&mut self, operation: PaintOperation, group: u32) {
        self.commands.push(PaintCommand {
            operation,
            group,
            layers: PaintLayers::default(),
            line: u32::MAX,
            paint_order: u32::MAX,
        });
    }
    pub fn record_line_since(&mut self, start: usize, line: usize, paint_order: u32) {
        for command in &mut self.commands[start..] {
            command.line = u32::try_from(line).expect("paint line index exceeds capacity");
            command.paint_order = paint_order;
        }
    }
    pub fn publish(&mut self, offsets: &[Vec2]) {
        for command in &mut self.commands {
            if let Some(&offset) = offsets
                .get(command.group as usize)
                .filter(|&&offset| offset != Vec2::ZERO)
            {
                command.operation.translate(offset);
            }
            command.group = 0;
        }
    }
    pub fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        report.add_slice_storage::<PaintCommand>(
            "PaintProgram.commands",
            self.commands.capacity(),
            self.commands.len(),
        );
        report.add_slice_storage::<Option<OverflowClip>>(
            "PaintProgram.content_clips",
            self.content_clips.capacity(),
            self.content_clips.len(),
        );
        report
    }
}
