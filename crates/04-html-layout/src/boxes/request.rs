use super::model::UsedBorderInsets;
use kurbo::Size;

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct AssignedBorderSize {
    pub(crate) width: Option<f64>,
    pub(crate) height: Option<f64>,
    pub(crate) height_is_definite: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum BoxLayoutPurpose {
    Ordinary,
    TableCell {
        used_borders: Option<UsedBorderInsets>,
    },
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum BoxMeasurementMode {
    Layout,
    TableIntrinsic,
    IntrinsicBlock,
}

#[derive(Clone, Copy, PartialEq)]
pub struct BoxLayoutRequest {
    pub(crate) box_idx: usize,
    pub(crate) available_width: f64,
    /// Optional inline space left by adjacent floats. Percentages and margins
    /// still resolve against `available_width`; only automatic/fit-content
    /// sizing consumes this narrower limit.
    pub(crate) auto_width_limit: Option<f64>,
    pub(crate) parent_content_height: Option<f64>,
    pub(crate) first_line_indent: f64,
    pub(crate) assigned_border_size: Option<AssignedBorderSize>,
    purpose: BoxLayoutPurpose,
    measurement_mode: BoxMeasurementMode,
}

impl BoxLayoutRequest {
    pub fn box_idx(self) -> usize {
        self.box_idx
    }

    pub fn normal(
        box_idx: usize,
        available_width: f64,
        parent_content_height: Option<f64>,
    ) -> Self {
        Self {
            box_idx,
            available_width,
            auto_width_limit: None,
            parent_content_height,
            first_line_indent: 0.0,
            assigned_border_size: None,
            purpose: BoxLayoutPurpose::Ordinary,
            measurement_mode: BoxMeasurementMode::Layout,
        }
    }

    pub fn with_auto_width_limit(mut self, auto_width_limit: f64) -> Self {
        self.auto_width_limit = Some(auto_width_limit.max(0.0));
        self
    }

    pub fn table_cell(
        box_idx: usize,
        available_width: f64,
        parent_content_height: Option<f64>,
        used_borders: Option<UsedBorderInsets>,
    ) -> Self {
        Self {
            purpose: BoxLayoutPurpose::TableCell { used_borders },
            ..Self::normal(box_idx, available_width, parent_content_height)
        }
    }

    pub fn for_table_intrinsic_measurement(mut self) -> Self {
        self.measurement_mode = BoxMeasurementMode::TableIntrinsic;
        self
    }

    pub fn for_intrinsic_block_measurement(mut self) -> Self {
        self.measurement_mode = BoxMeasurementMode::IntrinsicBlock;
        self
    }

    /// Assign the cell's border height and use the row/span height as its
    /// containing-height basis for percentage sizing.
    pub fn with_table_cell_height(mut self, height: f64) -> Self {
        self.parent_content_height = Some(height.max(0.0));
        self.with_assigned_size(None, Some(height.max(0.0)), true)
    }

    /// Final geometry for an atomic inline or Flex/Grid item. An assigned height
    /// fixes geometry without necessarily establishing a definite percentage
    /// basis for descendants.
    pub fn assigned(
        box_idx: usize,
        containing_width: f64,
        containing_height: Option<f64>,
        assigned: Size,
        height_is_definite: bool,
    ) -> Self {
        Self::normal(box_idx, containing_width, containing_height).with_assigned_size(
            Some(assigned.width),
            Some(assigned.height),
            height_is_definite,
        )
    }

    pub fn width_assigned(
        box_idx: usize,
        containing_width: f64,
        assigned_width: f64,
        parent_content_height: Option<f64>,
    ) -> Self {
        Self::normal(box_idx, containing_width, parent_content_height).with_assigned_size(
            Some(assigned_width),
            None,
            false,
        )
    }

    pub fn absolute_assigned(
        box_idx: usize,
        containing_width: f64,
        containing_height: f64,
        assigned_width: Option<f64>,
        assigned_height: Option<f64>,
    ) -> Self {
        Self::normal(box_idx, containing_width, Some(containing_height)).with_assigned_size(
            assigned_width,
            assigned_height,
            assigned_height.is_some(),
        )
    }

    fn with_assigned_size(
        mut self,
        width: Option<f64>,
        height: Option<f64>,
        height_is_definite: bool,
    ) -> Self {
        self.assigned_border_size = Some(AssignedBorderSize {
            width,
            height,
            height_is_definite,
        });
        self
    }

    pub(crate) fn used_borders(self) -> Option<UsedBorderInsets> {
        match self.purpose {
            BoxLayoutPurpose::Ordinary => None,
            BoxLayoutPurpose::TableCell { used_borders } => used_borders,
        }
    }

    pub(crate) fn splits_content_placement(self) -> bool {
        matches!(self.purpose, BoxLayoutPurpose::TableCell { .. })
    }

    pub(crate) fn suppresses_authored_height_basis(self) -> bool {
        self.measurement_mode != BoxMeasurementMode::Layout
    }

    pub(crate) fn is_intrinsic_block_measurement(self) -> bool {
        self.measurement_mode == BoxMeasurementMode::IntrinsicBlock
    }
}
