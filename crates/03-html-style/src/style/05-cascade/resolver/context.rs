//! Inputs and dependency phases shared by computed-value conversion.
use super::state::WorkingStyle;
use crate::style::environment::MediaEnvironment;
use html_style_model::ComputedStylesBuilder;
use lightningcss::properties::Property;

pub(super) struct ResolutionContext {
    pub(super) environment: MediaEnvironment,
    pub(super) font_size_scale: f32,
    pub(super) uses_viewport_units: bool,
    pub(super) line_height: Option<f32>,
    pub(super) root_line_height: Option<f32>,
}

impl ResolutionContext {
    pub(super) fn new(environment: MediaEnvironment) -> Self {
        Self {
            environment,
            font_size_scale: 1.0,
            uses_viewport_units: false,
            line_height: None,
            root_line_height: None,
        }
    }

    /// Evaluate typography in canonical coordinates before scaling the result.
    /// Parent and root sizes have already been scaled by earlier resolutions.
    pub(super) fn with_canonical_font_sizes<T>(
        &mut self,
        resolve: impl FnOnce(&mut Self) -> T,
    ) -> T {
        if self.font_size_scale == 1.0 {
            return resolve(self);
        }
        let mut canonical = Self {
            environment: self.environment,
            font_size_scale: 1.0,
            uses_viewport_units: false,
            line_height: self.line_height.map(|value| value / self.font_size_scale),
            root_line_height: self
                .root_line_height
                .map(|value| value / self.font_size_scale),
        };
        let result = resolve(&mut canonical);
        if canonical.uses_viewport_units {
            self.uses_viewport_units = true;
        }
        result
    }
}

pub(super) use crate::style::property_catalogue::CascadePhase;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ApplyResult {
    Applied,
    Invalid,
    Unhandled,
}

pub(super) struct PropertyContext<'a> {
    pub(super) is_root: bool,
    pub(super) resolution: &'a mut ResolutionContext,
    pub(super) styles: &'a mut ComputedStylesBuilder,
    pub(super) style: &'a mut WorkingStyle,
    pub(super) root_font_size: f32,
    pub(super) parent: &'a WorkingStyle,
}

/// Context-independent declaration work retained across phases and retries.
/// A missing owned value borrows the original event's native property.
#[derive(Default)]
pub(super) struct ResolvedDeclaration<'css> {
    pub(super) value: Option<Box<Property<'css>>>,
    pub(super) longhands: Vec<Property<'css>>,
    pub(super) expanded: bool,
    pub(super) substituted: bool,
    pub(super) valid: bool,
    pub(super) keyword: Option<&'static str>,
}
