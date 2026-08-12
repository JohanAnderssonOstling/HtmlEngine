use lightningcss::media_query::{MediaCondition as ParsedCondition, MediaFeatureComparison, MediaFeatureId, MediaFeatureName, MediaFeatureValue, MediaList, MediaType as ParsedMediaType, Operator, Qualifier, QueryFeature};
use lightningcss::values::length::{Length, LengthValue};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MediaType {
    #[default]
    Screen,
    Print,
}

/// CSS media characteristics supplied by the renderer.
///
/// Width is the effective content column/page width. Height is optional because
/// fragment rendering and initial reader loading can occur before a visible
/// page height is known.
#[derive(Clone, Copy, Debug)]
pub struct MediaEnvironment {
    media_type: MediaType,
    viewport_width: f64,
    viewport_height: Option<f64>,
}

impl MediaEnvironment {
    pub fn screen(viewport_width: f64, viewport_height: Option<f64>) -> Option<Self> {
        Self::new(MediaType::Screen, viewport_width, viewport_height)
    }

    pub fn new(media_type: MediaType, viewport_width: f64, viewport_height: Option<f64>) -> Option<Self> {
        if !viewport_width.is_finite() || viewport_width <= 0.0 || viewport_height.is_some_and(|height| !height.is_finite() || height <= 0.0) {
            return None;
        }
        Some(Self { media_type, viewport_width, viewport_height })
    }

    pub fn media_type(self) -> MediaType {
        self.media_type
    }

    pub fn viewport_width(self) -> f64 {
        self.viewport_width
    }

    pub fn viewport_height(self) -> Option<f64> {
        self.viewport_height
    }
}

impl Default for MediaEnvironment {
    fn default() -> Self {
        Self { media_type: MediaType::Screen, viewport_width: 600.0, viewport_height: None }
    }
}

impl PartialEq for MediaEnvironment {
    fn eq(&self, other: &Self) -> bool {
        self.media_type == other.media_type && self.viewport_width.to_bits() == other.viewport_width.to_bits() && self.viewport_height.map(f64::to_bits) == other.viewport_height.map(f64::to_bits)
    }
}

impl Eq for MediaEnvironment {}

/// Exact activation state of conditional style-rule groups for one environment.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MediaMatchKey {
    words: Vec<u64>,
}

/// Owned media dependencies retained by the pipeline after Lightning CSS's
/// borrowed stylesheet AST has been dropped.
#[derive(Debug, Default)]
pub struct MediaQuerySet {
    queries: Vec<CompiledMediaList>,
    rule_conditions: Vec<Vec<u32>>,
    uses_viewport_units: AtomicBool,
}

impl Clone for MediaQuerySet {
    fn clone(&self) -> Self {
        Self { queries: self.queries.clone(), rule_conditions: self.rule_conditions.clone(), uses_viewport_units: AtomicBool::new(self.uses_viewport_units()) }
    }
}

impl MediaQuerySet {
    pub fn uses_viewport_units(&self) -> bool {
        self.uses_viewport_units.load(AtomicOrdering::Relaxed)
    }

    pub(crate) fn mark_viewport_unit_dependency(&self) {
        self.uses_viewport_units.store(true, AtomicOrdering::Relaxed);
    }
    pub fn match_key(&self, environment: MediaEnvironment, initial_font_size: f64) -> MediaMatchKey {
        let mut words = vec![0; self.rule_conditions.len().div_ceil(64)];
        for (index, path) in self.rule_conditions.iter().enumerate() {
            if path.iter().all(|query| self.queries[*query as usize].evaluate(environment, initial_font_size).is_true()) {
                words[index / 64] |= 1 << (index % 64);
            }
        }
        MediaMatchKey { words }
    }

    pub fn is_empty(&self) -> bool {
        self.rule_conditions.is_empty()
    }

    pub(crate) fn register_query(&mut self, query: CompiledMediaList) -> u32 {
        let id = u32::try_from(self.queries.len()).expect("a stylesheet cannot contain more than u32::MAX media rules");
        self.queries.push(query);
        id
    }

    pub(crate) fn record_dependency(&mut self, media_path: &[u32]) {
        if !media_path.is_empty() && self.rule_conditions.last().is_none_or(|last| last != media_path) {
            self.rule_conditions.push(media_path.to_vec());
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MatchResult {
    False,
    True,
    Unknown,
}

impl MatchResult {
    pub(crate) fn is_true(self) -> bool {
        self == Self::True
    }

    fn not(self) -> Self {
        match self {
            Self::False => Self::True,
            Self::True => Self::False,
            Self::Unknown => Self::Unknown,
        }
    }

    fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::False, _) | (_, Self::False) => Self::False,
            (Self::True, Self::True) => Self::True,
            _ => Self::Unknown,
        }
    }

    fn or(self, other: Self) -> Self {
        match (self, other) {
            (Self::True, _) | (_, Self::True) => Self::True,
            (Self::False, Self::False) => Self::False,
            _ => Self::Unknown,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CompiledMediaList {
    queries: Vec<CompiledMediaQuery>,
}

impl CompiledMediaList {
    pub(crate) fn compile(list: &MediaList<'_>) -> Self {
        Self { queries: list.media_queries.iter().map(CompiledMediaQuery::compile).collect() }
    }

    pub(crate) fn evaluate(&self, environment: MediaEnvironment, initial_font_size: f64) -> MatchResult {
        if self.queries.is_empty() {
            return MatchResult::True;
        }
        self.queries.iter().fold(MatchResult::False, |result, query| result.or(query.evaluate(environment, initial_font_size)))
    }
}

#[derive(Clone, Debug)]
struct CompiledMediaQuery {
    negated: bool,
    media_type: CompiledMediaType,
    condition: Option<CompiledCondition>,
}

impl CompiledMediaQuery {
    fn compile(query: &lightningcss::media_query::MediaQuery<'_>) -> Self {
        let media_type = match query.media_type {
            ParsedMediaType::All => CompiledMediaType::All,
            ParsedMediaType::Screen => CompiledMediaType::Screen,
            ParsedMediaType::Print => CompiledMediaType::Print,
            ParsedMediaType::Custom(_) => CompiledMediaType::Custom,
        };
        Self { negated: query.qualifier == Some(Qualifier::Not), media_type, condition: query.condition.as_ref().map(CompiledCondition::compile) }
    }

    fn evaluate(&self, environment: MediaEnvironment, initial_font_size: f64) -> MatchResult {
        let media_type = match self.media_type {
            CompiledMediaType::All => MatchResult::True,
            CompiledMediaType::Screen => MatchResult::from(environment.media_type() == MediaType::Screen),
            CompiledMediaType::Print => MatchResult::from(environment.media_type() == MediaType::Print),
            CompiledMediaType::Custom => MatchResult::False,
        };
        let result = self.condition.as_ref().map_or(media_type, |condition| media_type.and(condition.evaluate(environment, initial_font_size)));
        if self.negated { result.not() } else { result }
    }
}

impl From<bool> for MatchResult {
    fn from(value: bool) -> Self {
        if value { Self::True } else { Self::False }
    }
}

#[derive(Clone, Copy, Debug)]
enum CompiledMediaType {
    All,
    Screen,
    Print,
    Custom,
}

#[derive(Clone, Debug)]
enum CompiledCondition {
    Feature(CompiledFeature),
    Not(Box<Self>),
    And(Vec<Self>),
    Or(Vec<Self>),
    Unknown,
}

impl CompiledCondition {
    fn compile(condition: &ParsedCondition<'_>) -> Self {
        match condition {
            ParsedCondition::Feature(feature) => Self::Feature(CompiledFeature::compile(feature)),
            ParsedCondition::Not(condition) => Self::Not(Box::new(Self::compile(condition))),
            ParsedCondition::Operation { operator: Operator::And, conditions } => Self::And(conditions.iter().map(Self::compile).collect()),
            ParsedCondition::Operation { operator: Operator::Or, conditions } => Self::Or(conditions.iter().map(Self::compile).collect()),
            ParsedCondition::Unknown(_) => Self::Unknown,
        }
    }

    fn evaluate(&self, environment: MediaEnvironment, initial_font_size: f64) -> MatchResult {
        match self {
            Self::Feature(feature) => feature.evaluate(environment, initial_font_size),
            Self::Not(condition) => condition.evaluate(environment, initial_font_size).not(),
            Self::And(conditions) => conditions.iter().fold(MatchResult::True, |result, condition| result.and(condition.evaluate(environment, initial_font_size))),
            Self::Or(conditions) => conditions.iter().fold(MatchResult::False, |result, condition| result.or(condition.evaluate(environment, initial_font_size))),
            Self::Unknown => MatchResult::Unknown,
        }
    }
}

#[derive(Clone, Debug)]
enum CompiledFeature {
    Boolean(FeatureName),
    Comparison { name: FeatureName, operator: MediaFeatureComparison, value: FeatureValue },
    Interval { name: FeatureName, start: FeatureValue, start_operator: MediaFeatureComparison, end: FeatureValue, end_operator: MediaFeatureComparison },
    Unknown,
}

impl CompiledFeature {
    fn compile(feature: &lightningcss::media_query::MediaFeature<'_>) -> Self {
        match feature {
            QueryFeature::Boolean { name } => FeatureName::compile(name).map_or(Self::Unknown, Self::Boolean),
            QueryFeature::Plain { name, value } => match (FeatureName::compile(name), FeatureValue::compile(value)) {
                (Some(name), Some(value)) => Self::Comparison { name, operator: MediaFeatureComparison::Equal, value },
                _ => Self::Unknown,
            },
            QueryFeature::Range { name, operator, value } => match (FeatureName::compile(name), FeatureValue::compile(value)) {
                (Some(name), Some(value)) => Self::Comparison { name, operator: *operator, value },
                _ => Self::Unknown,
            },
            QueryFeature::Interval { name, start, start_operator, end, end_operator } => match (FeatureName::compile(name), FeatureValue::compile(start), FeatureValue::compile(end)) {
                (Some(name), Some(start), Some(end)) => Self::Interval { name, start, start_operator: *start_operator, end, end_operator: *end_operator },
                _ => Self::Unknown,
            },
        }
    }

    fn evaluate(&self, environment: MediaEnvironment, initial_font_size: f64) -> MatchResult {
        match self {
            Self::Boolean(name) => name.actual_value(environment).map_or(MatchResult::Unknown, |value| MatchResult::from(value.is_truthy())),
            Self::Comparison { name, operator, value } => compare_feature(*name, environment, initial_font_size, *operator, value),
            Self::Interval { name, start, start_operator, end, end_operator } => {
                let Some(actual) = name.actual_value(environment) else { return MatchResult::Unknown };
                let Some(start) = start.resolve(environment, initial_font_size) else { return MatchResult::Unknown };
                let Some(end) = end.resolve(environment, initial_font_size) else { return MatchResult::Unknown };
                MatchResult::from(compare_values(&start, *start_operator, &actual) && compare_values(&actual, *end_operator, &end))
            }
            Self::Unknown => MatchResult::Unknown,
        }
    }
}

fn compare_feature(name: FeatureName, environment: MediaEnvironment, initial_font_size: f64, operator: MediaFeatureComparison, expected: &FeatureValue) -> MatchResult {
    let Some(actual) = name.actual_value(environment) else { return MatchResult::Unknown };
    let Some(expected) = expected.resolve(environment, initial_font_size) else { return MatchResult::Unknown };
    MatchResult::from(compare_values(&actual, operator, &expected))
}

fn compare_values(actual: &ResolvedValue, operator: MediaFeatureComparison, expected: &ResolvedValue) -> bool {
    let ordering = match (actual, expected) {
        (ResolvedValue::Number(actual), ResolvedValue::Number(expected)) => actual.partial_cmp(expected),
        (ResolvedValue::Ident(actual), ResolvedValue::Ident(expected)) => return operator == MediaFeatureComparison::Equal && actual == expected,
        _ => None,
    };
    match (operator, ordering) {
        (MediaFeatureComparison::Equal, Some(std::cmp::Ordering::Equal)) => true,
        (MediaFeatureComparison::GreaterThan, Some(std::cmp::Ordering::Greater)) => true,
        (MediaFeatureComparison::GreaterThanEqual, Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)) => true,
        (MediaFeatureComparison::LessThan, Some(std::cmp::Ordering::Less)) => true,
        (MediaFeatureComparison::LessThanEqual, Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)) => true,
        _ => false,
    }
}

#[derive(Clone, Copy, Debug)]
enum FeatureName {
    Width,
    Height,
    AspectRatio,
    DeviceWidth,
    DeviceHeight,
    DeviceAspectRatio,
    Orientation,
    Color,
    ColorIndex,
    Monochrome,
    ColorGamut,
}

impl FeatureName {
    fn compile(name: &MediaFeatureName<'_, MediaFeatureId>) -> Option<Self> {
        match name {
            MediaFeatureName::Standard(MediaFeatureId::Width) => Some(Self::Width),
            MediaFeatureName::Standard(MediaFeatureId::Height) => Some(Self::Height),
            MediaFeatureName::Standard(MediaFeatureId::AspectRatio) => Some(Self::AspectRatio),
            MediaFeatureName::Standard(MediaFeatureId::DeviceWidth) => Some(Self::DeviceWidth),
            MediaFeatureName::Standard(MediaFeatureId::DeviceHeight) => Some(Self::DeviceHeight),
            MediaFeatureName::Standard(MediaFeatureId::DeviceAspectRatio) => Some(Self::DeviceAspectRatio),
            MediaFeatureName::Standard(MediaFeatureId::Orientation) => Some(Self::Orientation),
            MediaFeatureName::Standard(MediaFeatureId::Color) => Some(Self::Color),
            MediaFeatureName::Standard(MediaFeatureId::ColorIndex) => Some(Self::ColorIndex),
            MediaFeatureName::Standard(MediaFeatureId::Monochrome) => Some(Self::Monochrome),
            MediaFeatureName::Standard(MediaFeatureId::ColorGamut) => Some(Self::ColorGamut),
            _ => None,
        }
    }

    fn actual_value(self, environment: MediaEnvironment) -> Option<ResolvedValue> {
        match self {
            Self::Width | Self::DeviceWidth => Some(ResolvedValue::Number(environment.viewport_width())),
            Self::Height | Self::DeviceHeight => environment.viewport_height().map(ResolvedValue::Number),
            Self::AspectRatio | Self::DeviceAspectRatio => environment.viewport_height().map(|height| ResolvedValue::Number(environment.viewport_width() / height)),
            Self::Orientation => environment.viewport_height().map(|height| ResolvedValue::Ident(if environment.viewport_width() > height { MediaIdent::Landscape } else { MediaIdent::Portrait })),
            Self::Color => Some(ResolvedValue::Number(8.0)),
            Self::ColorIndex | Self::Monochrome => Some(ResolvedValue::Number(0.0)),
            Self::ColorGamut => Some(ResolvedValue::Ident(MediaIdent::Srgb)),
        }
    }
}

#[derive(Clone, Debug)]
enum FeatureValue {
    Length(CompiledLength),
    Ratio(f64),
    Number(f64),
    Ident(MediaIdent),
}

impl FeatureValue {
    fn compile(value: &MediaFeatureValue<'_>) -> Option<Self> {
        match value {
            MediaFeatureValue::Length(length) => CompiledLength::compile(length).map(Self::Length),
            MediaFeatureValue::Integer(value) => Some(Self::Number(f64::from(*value))),
            MediaFeatureValue::Number(value) if value.is_finite() => Some(Self::Number(f64::from(*value))),
            MediaFeatureValue::Ratio(ratio) if ratio.0.is_finite() && ratio.1.is_finite() => {
                // Media Queries normalizes a zero denominator to an infinite
                // ratio, including the legacy `0/0` spelling.
                Some(Self::Ratio(if ratio.1 == 0.0 { f64::INFINITY } else { f64::from(ratio.0 / ratio.1) }))
            }
            MediaFeatureValue::Ident(ident) if ident.as_ref().eq_ignore_ascii_case("portrait") => Some(Self::Ident(MediaIdent::Portrait)),
            MediaFeatureValue::Ident(ident) if ident.as_ref().eq_ignore_ascii_case("landscape") => Some(Self::Ident(MediaIdent::Landscape)),
            MediaFeatureValue::Ident(ident) if ident.as_ref().eq_ignore_ascii_case("srgb") => Some(Self::Ident(MediaIdent::Srgb)),
            MediaFeatureValue::Ident(ident) if ident.as_ref().eq_ignore_ascii_case("p3") => Some(Self::Ident(MediaIdent::P3)),
            MediaFeatureValue::Ident(ident) if ident.as_ref().eq_ignore_ascii_case("rec2020") => Some(Self::Ident(MediaIdent::Rec2020)),
            _ => None,
        }
    }

    fn resolve(&self, environment: MediaEnvironment, initial_font_size: f64) -> Option<ResolvedValue> {
        match self {
            Self::Length(length) => length.resolve(environment, initial_font_size).map(ResolvedValue::Number),
            Self::Ratio(ratio) => Some(ResolvedValue::Number(*ratio)),
            Self::Number(number) => Some(ResolvedValue::Number(*number)),
            Self::Ident(ident) => Some(ResolvedValue::Ident(*ident)),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MediaIdent {
    Portrait,
    Landscape,
    Srgb,
    P3,
    Rec2020,
}

#[derive(Clone, Copy, Debug)]
enum ResolvedValue {
    Number(f64),
    Ident(MediaIdent),
}

impl ResolvedValue {
    fn is_truthy(self) -> bool {
        match self {
            Self::Number(value) => value != 0.0,
            Self::Ident(_) => true,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum CompiledLength {
    Pixels(f64),
    InitialFont(f64),
    ViewportWidth(f64),
    ViewportHeight(f64),
    ViewportMin(f64),
    ViewportMax(f64),
}

impl CompiledLength {
    fn compile(length: &Length) -> Option<Self> {
        if let Some(px) = length.to_px() {
            return px.is_finite().then_some(Self::Pixels(f64::from(px)));
        }
        let Length::Value(value) = length else { return None };
        let compiled = match value {
            LengthValue::Em(value) | LengthValue::Rem(value) => Self::InitialFont(f64::from(*value)),
            LengthValue::Vw(value) | LengthValue::Lvw(value) | LengthValue::Svw(value) | LengthValue::Dvw(value) | LengthValue::Vi(value) | LengthValue::Lvi(value) | LengthValue::Svi(value) | LengthValue::Dvi(value) => {
                Self::ViewportWidth(f64::from(*value) / 100.0)
            }
            LengthValue::Vh(value) | LengthValue::Lvh(value) | LengthValue::Svh(value) | LengthValue::Dvh(value) | LengthValue::Vb(value) | LengthValue::Lvb(value) | LengthValue::Svb(value) | LengthValue::Dvb(value) => {
                Self::ViewportHeight(f64::from(*value) / 100.0)
            }
            LengthValue::Vmin(value) | LengthValue::Lvmin(value) | LengthValue::Svmin(value) | LengthValue::Dvmin(value) => Self::ViewportMin(f64::from(*value) / 100.0),
            LengthValue::Vmax(value) | LengthValue::Lvmax(value) | LengthValue::Svmax(value) | LengthValue::Dvmax(value) => Self::ViewportMax(f64::from(*value) / 100.0),
            _ => return None,
        };
        Some(compiled)
    }

    fn resolve(self, environment: MediaEnvironment, initial_font_size: f64) -> Option<f64> {
        match self {
            Self::Pixels(value) => Some(value),
            Self::InitialFont(value) => Some(value * initial_font_size),
            Self::ViewportWidth(value) => Some(value * environment.viewport_width()),
            Self::ViewportHeight(value) => environment.viewport_height().map(|height| value * height),
            Self::ViewportMin(value) => environment.viewport_height().map(|height| value * environment.viewport_width().min(height)),
            Self::ViewportMax(value) => environment.viewport_height().map(|height| value * environment.viewport_width().max(height)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CompiledMediaList, MatchResult, MediaEnvironment};
    use lightningcss::rules::CssRule;
    use lightningcss::stylesheet::ParserOptions;
    use lightningcss::stylesheet::StyleSheet;

    fn evaluate(query: &str, width: f64, height: Option<f64>) -> MatchResult {
        let css = format!("@media {query} {{ p {{ color: red }} }}");
        let stylesheet = StyleSheet::parse(&css, ParserOptions::default()).expect("media query parses");
        let CssRule::Media(rule) = &stylesheet.rules.0[0] else { panic!("expected a media rule") };
        CompiledMediaList::compile(&rule.query).evaluate(MediaEnvironment::screen(width, height).unwrap(), 16.0)
    }

    #[test]
    fn evaluates_width_ranges_and_boolean_operations() {
        assert!(evaluate("screen and (min-width: 40em)", 640.0, Some(800.0)).is_true());
        assert_eq!(evaluate("screen and (width > 640px)", 640.0, Some(800.0)), MatchResult::False);
        assert!(evaluate("(400px < width <= 640px)", 640.0, Some(800.0)).is_true());
        assert!(evaluate("(max-width: 500px), (orientation: landscape)", 700.0, Some(600.0)).is_true());
    }

    #[test]
    fn evaluates_infinite_and_device_aspect_ratios() {
        assert!(evaluate("screen and (max-aspect-ratio: 0/0)", 800.0, Some(600.0)).is_true());
        assert!(evaluate("screen and (max-device-aspect-ratio: 0/0)", 800.0, Some(600.0)).is_true());
        assert!(evaluate("screen and (min-device-aspect-ratio: 1279/1024)", 800.0, Some(600.0)).is_true());
        assert!(evaluate("screen and (device-width: 800px) and (device-height: 600px)", 800.0, Some(600.0)).is_true());
    }

    #[test]
    fn evaluates_static_color_media_characteristics() {
        assert!(evaluate("(color)", 800.0, Some(600.0)).is_true());
        assert!(evaluate("not (monochrome)", 800.0, Some(600.0)).is_true());
        assert!(evaluate("(min-color: -10) and (min-color-index: -10) and (min-monochrome: -10)", 800.0, Some(600.0)).is_true());
        assert!(evaluate("(color-gamut: srgb)", 800.0, Some(600.0)).is_true());
        assert_eq!(evaluate("(color-gamut: p3)", 800.0, Some(600.0)), MatchResult::False);
        assert_eq!(evaluate("(color-gamut: rec2020)", 800.0, Some(600.0)), MatchResult::False);
    }

    #[test]
    fn unknown_height_and_unknown_features_do_not_match_even_when_negated() {
        assert_eq!(evaluate("(min-height: 500px)", 600.0, None), MatchResult::Unknown);
        assert_eq!(evaluate("not (unknown-renderer-feature: yes)", 600.0, Some(800.0)), MatchResult::Unknown);
    }
}
