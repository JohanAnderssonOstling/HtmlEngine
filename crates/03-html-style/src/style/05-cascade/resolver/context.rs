//! Inputs and dependency phases shared by computed-value conversion.
use super::state::{ParentStyle, WorkingStyle};
use crate::style::cascade::resolver::prepared_native::PreparedNativeValue;
use crate::style::environment::MediaEnvironment;
use crate::style::syntax::prepared_length::finite;
use crate::style::syntax::prepared_renderer::PreparedRendererValue;
use crate::style::syntax::wide_keywords::CssWideKeyword;
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
            line_height: self
                .line_height
                .map(|value| finite(f64::from(value) / f64::from(self.font_size_scale))),
            root_line_height: self
                .root_line_height
                .map(|value| finite(f64::from(value) / f64::from(self.font_size_scale))),
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
    Unhandled,
}

pub(super) struct PropertyContext<'a> {
    pub(super) is_root: bool,
    pub(super) resolution: &'a mut ResolutionContext,
    pub(super) styles: &'a mut ComputedStylesBuilder,
    pub(super) style: &'a mut WorkingStyle,
    pub(super) root_font_size: f32,
    pub(super) parent: &'a ParentStyle,
}

/// Context-independent declaration work retained across dependency phases.
/// Inactive owned storage is retained for reuse; ordinary declarations borrow
/// the original event's native property.
#[derive(Default)]
pub(super) struct ResolvedDeclaration<'css> {
    pub(super) value: Option<Box<Property<'css>>>,
    pub(super) longhands: Vec<Property<'css>>,
    pub(super) native_values: Vec<Option<PreparedNativeValue>>,
    pub(super) renderer_value: Option<Box<PreparedRendererValue>>,
    keyword: Option<CssWideKeyword>,
    pub(super) requires_viewport_height: bool,
    state: u8,
}

impl<'css> ResolvedDeclaration<'css> {
    const OWNS_VALUE: u8 = 1;
    const EXPANDED: u8 = 2;
    const SUBSTITUTED: u8 = 4;
    const VALID: u8 = 8;

    pub(super) fn reset(&mut self) {
        self.state = 0;
        self.keyword = None;
        self.requires_viewport_height = false;
        self.longhands.clear();
        self.native_values.clear();
        self.renderer_value = None;
    }

    pub(super) fn keyword(&self) -> Option<CssWideKeyword> {
        self.keyword
    }

    pub(super) fn set_keyword(&mut self, keyword: Option<CssWideKeyword>) {
        self.keyword = keyword;
    }

    pub(super) fn owns_value(&self) -> bool {
        self.state & Self::OWNS_VALUE != 0
    }

    pub(super) fn expanded(&self) -> bool {
        self.state & Self::EXPANDED != 0
    }

    pub(super) fn set_expanded(&mut self, value: bool) {
        self.state = (self.state & !Self::EXPANDED) | (u8::from(value) * Self::EXPANDED);
    }

    pub(super) fn substituted(&self) -> bool {
        self.state & Self::SUBSTITUTED != 0
    }

    pub(super) fn set_substituted(&mut self, value: bool) {
        self.state = (self.state & !Self::SUBSTITUTED) | (u8::from(value) * Self::SUBSTITUTED);
    }

    pub(super) fn valid(&self) -> bool {
        self.state & Self::VALID != 0
    }

    pub(super) fn set_valid(&mut self, value: bool) {
        self.state = (self.state & !Self::VALID) | (u8::from(value) * Self::VALID);
    }

    pub(super) fn property<'a>(&'a self, original: &'a Property<'css>) -> &'a Property<'css> {
        if self.owns_value() {
            self.value.as_deref().expect("resolved owned value")
        } else {
            original
        }
    }

    pub(super) fn set_value(&mut self, value: Property<'css>) {
        if let Some(stored) = &mut self.value {
            **stored = value;
        } else {
            self.value = Some(Box::new(value));
        }
        self.state |= Self::OWNS_VALUE;
    }
}
