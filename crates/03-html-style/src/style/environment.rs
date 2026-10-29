use lightningcss::media_query::{
    MediaCondition, MediaFeature, MediaFeatureComparison, MediaFeatureId, MediaFeatureName,
    MediaFeatureValue, MediaList, MediaQuery, MediaType as ParsedMediaType, Operator, Qualifier,
    QueryFeature,
};
use lightningcss::values::length::{Length, LengthValue};
use static_self::IntoOwned;
use std::sync::Arc;

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

    pub fn new(
        media_type: MediaType,
        viewport_width: f64,
        viewport_height: Option<f64>,
    ) -> Option<Self> {
        if !viewport_width.is_finite()
            || viewport_width <= 0.0
            || viewport_height.is_some_and(|height| !height.is_finite() || height <= 0.0)
        {
            return None;
        }
        Some(Self {
            media_type,
            viewport_width,
            viewport_height,
        })
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
        Self {
            media_type: MediaType::Screen,
            viewport_width: 600.0,
            viewport_height: None,
        }
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

#[derive(Clone, Debug, Default)]
struct MediaQueryDefinitions {
    queries: Vec<MediaList<'static>>,
    rule_conditions: Vec<Vec<u32>>,
}

/// Media dependencies retained after the stylesheet AST has been dropped.
/// Query definitions are shared with the compiled program; viewport-unit use
/// belongs to the document that was resolved against those definitions.
#[derive(Clone, Debug, Default)]
pub struct MediaQuerySet {
    definitions: Option<Arc<MediaQueryDefinitions>>,
    uses_viewport_units: bool,
}

impl MediaQuerySet {
    pub fn uses_viewport_units(&self) -> bool {
        self.uses_viewport_units
    }

    pub(crate) fn mark_viewport_unit_dependency(&mut self) {
        self.uses_viewport_units = true;
    }

    pub fn match_key(
        &self,
        environment: MediaEnvironment,
        initial_font_size: f64,
    ) -> MediaMatchKey {
        let Some(definitions) = &self.definitions else {
            return MediaMatchKey::default();
        };
        let mut words = vec![0; definitions.rule_conditions.len().div_ceil(64)];
        for (index, path) in definitions.rule_conditions.iter().enumerate() {
            if path.iter().all(|query| {
                evaluate_media_list(
                    &definitions.queries[*query as usize],
                    environment,
                    initial_font_size,
                )
                .is_true()
            }) {
                words[index / 64] |= 1 << (index % 64);
            }
        }
        MediaMatchKey { words }
    }

    pub fn is_empty(&self) -> bool {
        self.definitions
            .as_ref()
            .is_none_or(|definitions| definitions.rule_conditions.is_empty())
    }

    pub(crate) fn register_query(&mut self, query: &MediaList<'_>) -> u32 {
        let definitions = self.definitions_mut();
        let id = u32::try_from(definitions.queries.len())
            .expect("a stylesheet cannot contain more than u32::MAX media rules");
        definitions.queries.push(query.clone().into_owned());
        id
    }

    pub(crate) fn record_dependency(&mut self, media_path: &[u32]) {
        if !media_path.is_empty()
            && self
                .definitions
                .as_ref()
                .and_then(|definitions| definitions.rule_conditions.last())
                .is_none_or(|last| last != media_path)
        {
            self.definitions_mut()
                .rule_conditions
                .push(media_path.to_vec());
        }
    }

    fn definitions_mut(&mut self) -> &mut MediaQueryDefinitions {
        Arc::make_mut(self.definitions.get_or_insert_with(Arc::default))
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

/// Evaluate native Lightning CSS nodes against the renderer's media environment.
pub(crate) fn evaluate_media_list(
    list: &MediaList<'_>,
    environment: MediaEnvironment,
    initial_font_size: f64,
) -> MatchResult {
    if list.media_queries.is_empty() {
        return MatchResult::True;
    }
    list.media_queries
        .iter()
        .fold(MatchResult::False, |result, query| {
            result.or(evaluate_query(query, environment, initial_font_size))
        })
}

fn evaluate_query(
    query: &MediaQuery<'_>,
    environment: MediaEnvironment,
    initial_font_size: f64,
) -> MatchResult {
    let media_type = match query.media_type {
        ParsedMediaType::All => MatchResult::True,
        ParsedMediaType::Screen => MatchResult::from(environment.media_type() == MediaType::Screen),
        ParsedMediaType::Print => MatchResult::from(environment.media_type() == MediaType::Print),
        ParsedMediaType::Custom(_) => MatchResult::False,
    };
    let result = query.condition.as_ref().map_or(media_type, |condition| {
        media_type.and(evaluate_condition(
            condition,
            environment,
            initial_font_size,
        ))
    });
    if query.qualifier == Some(Qualifier::Not) {
        result.not()
    } else {
        result
    }
}

fn evaluate_condition(
    condition: &MediaCondition<'_>,
    environment: MediaEnvironment,
    initial_font_size: f64,
) -> MatchResult {
    match condition {
        MediaCondition::Feature(feature) => {
            evaluate_feature(feature, environment, initial_font_size)
        }
        MediaCondition::Not(condition) => {
            evaluate_condition(condition, environment, initial_font_size).not()
        }
        MediaCondition::Operation {
            operator: Operator::And,
            conditions,
        } => conditions
            .iter()
            .fold(MatchResult::True, |result, condition| {
                result.and(evaluate_condition(
                    condition,
                    environment,
                    initial_font_size,
                ))
            }),
        MediaCondition::Operation {
            operator: Operator::Or,
            conditions,
        } => conditions
            .iter()
            .fold(MatchResult::False, |result, condition| {
                result.or(evaluate_condition(
                    condition,
                    environment,
                    initial_font_size,
                ))
            }),
        MediaCondition::Unknown(_) => MatchResult::Unknown,
    }
}

fn evaluate_feature(
    feature: &MediaFeature<'_>,
    environment: MediaEnvironment,
    initial_font_size: f64,
) -> MatchResult {
    let (name, operator, value) = match feature {
        QueryFeature::Boolean { name } => {
            return actual_feature_value(name, environment).map_or(MatchResult::Unknown, |value| {
                MatchResult::from(value.is_truthy())
            });
        }
        QueryFeature::Plain { name, value } => (name, MediaFeatureComparison::Equal, value),
        QueryFeature::Range {
            name,
            operator,
            value,
        } => (name, *operator, value),
        QueryFeature::Interval {
            name,
            start,
            start_operator,
            end,
            end_operator,
        } => {
            let Some(actual) = actual_feature_value(name, environment) else {
                return MatchResult::Unknown;
            };
            let Some(start) = resolve_feature_value(start, environment, initial_font_size) else {
                return MatchResult::Unknown;
            };
            let Some(end) = resolve_feature_value(end, environment, initial_font_size) else {
                return MatchResult::Unknown;
            };
            return MatchResult::from(
                compare_values(&start, *start_operator, &actual)
                    && compare_values(&actual, *end_operator, &end),
            );
        }
    };
    let Some(actual) = actual_feature_value(name, environment) else {
        return MatchResult::Unknown;
    };
    let Some(expected) = resolve_feature_value(value, environment, initial_font_size) else {
        return MatchResult::Unknown;
    };
    MatchResult::from(compare_values(&actual, operator, &expected))
}

impl From<bool> for MatchResult {
    fn from(value: bool) -> Self {
        if value { Self::True } else { Self::False }
    }
}

fn compare_values(
    actual: &ResolvedValue,
    operator: MediaFeatureComparison,
    expected: &ResolvedValue,
) -> bool {
    let ordering = match (actual, expected) {
        (ResolvedValue::Number(actual), ResolvedValue::Number(expected)) => {
            actual.partial_cmp(expected)
        }
        (ResolvedValue::Ident(actual), ResolvedValue::Ident(expected)) => {
            return operator == MediaFeatureComparison::Equal && actual == expected;
        }
        _ => None,
    };
    match (operator, ordering) {
        (MediaFeatureComparison::Equal, Some(std::cmp::Ordering::Equal)) => true,
        (MediaFeatureComparison::GreaterThan, Some(std::cmp::Ordering::Greater)) => true,
        (
            MediaFeatureComparison::GreaterThanEqual,
            Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal),
        ) => true,
        (MediaFeatureComparison::LessThan, Some(std::cmp::Ordering::Less)) => true,
        (
            MediaFeatureComparison::LessThanEqual,
            Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal),
        ) => true,
        _ => false,
    }
}

fn actual_feature_value(
    name: &MediaFeatureName<'_, MediaFeatureId>,
    environment: MediaEnvironment,
) -> Option<ResolvedValue> {
    let MediaFeatureName::Standard(name) = name else {
        return None;
    };
    match name {
        MediaFeatureId::Width | MediaFeatureId::DeviceWidth => {
            Some(ResolvedValue::Number(environment.viewport_width()))
        }
        MediaFeatureId::Height | MediaFeatureId::DeviceHeight => {
            environment.viewport_height().map(ResolvedValue::Number)
        }
        MediaFeatureId::AspectRatio | MediaFeatureId::DeviceAspectRatio => environment
            .viewport_height()
            .map(|height| ResolvedValue::Number(environment.viewport_width() / height)),
        MediaFeatureId::Orientation => environment.viewport_height().map(|height| {
            ResolvedValue::Ident(if environment.viewport_width() > height {
                MediaIdent::Landscape
            } else {
                MediaIdent::Portrait
            })
        }),
        MediaFeatureId::Color => Some(ResolvedValue::Number(8.0)),
        MediaFeatureId::ColorIndex | MediaFeatureId::Monochrome => Some(ResolvedValue::Number(0.0)),
        MediaFeatureId::ColorGamut => Some(ResolvedValue::Ident(MediaIdent::Srgb)),
        _ => None,
    }
}

fn resolve_feature_value(
    value: &MediaFeatureValue<'_>,
    environment: MediaEnvironment,
    initial_font_size: f64,
) -> Option<ResolvedValue> {
    Some(match value {
        MediaFeatureValue::Length(length) => {
            ResolvedValue::Number(resolve_length(length, environment, initial_font_size)?)
        }
        MediaFeatureValue::Integer(value) => ResolvedValue::Number(f64::from(*value)),
        MediaFeatureValue::Number(value) if value.is_finite() => {
            ResolvedValue::Number(f64::from(*value))
        }
        MediaFeatureValue::Ratio(ratio) if ratio.0.is_finite() && ratio.1.is_finite() => {
            // Keep the existing f32 ratio rounding and infinite zero denominator.
            ResolvedValue::Number(if ratio.1 == 0.0 {
                f64::INFINITY
            } else {
                f64::from(ratio.0 / ratio.1)
            })
        }
        MediaFeatureValue::Ident(ident) => {
            let (_, value) = [
                ("portrait", MediaIdent::Portrait),
                ("landscape", MediaIdent::Landscape),
                ("srgb", MediaIdent::Srgb),
                ("p3", MediaIdent::P3),
                ("rec2020", MediaIdent::Rec2020),
            ]
            .into_iter()
            .find(|(name, _)| ident.as_ref().eq_ignore_ascii_case(name))?;
            ResolvedValue::Ident(value)
        }
        _ => return None,
    })
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

fn resolve_length(
    length: &Length,
    environment: MediaEnvironment,
    initial_font_size: f64,
) -> Option<f64> {
    if let Some(px) = length.to_px() {
        return px.is_finite().then_some(f64::from(px));
    }
    let Length::Value(value) = length else {
        return None;
    };
    let (value, basis) = match value {
        LengthValue::Em(value) | LengthValue::Rem(value) => {
            return Some(f64::from(*value) * initial_font_size);
        }
        LengthValue::Vw(value)
        | LengthValue::Lvw(value)
        | LengthValue::Svw(value)
        | LengthValue::Dvw(value)
        | LengthValue::Vi(value)
        | LengthValue::Lvi(value)
        | LengthValue::Svi(value)
        | LengthValue::Dvi(value) => (value, environment.viewport_width()),
        LengthValue::Vh(value)
        | LengthValue::Lvh(value)
        | LengthValue::Svh(value)
        | LengthValue::Dvh(value)
        | LengthValue::Vb(value)
        | LengthValue::Lvb(value)
        | LengthValue::Svb(value)
        | LengthValue::Dvb(value) => (value, environment.viewport_height()?),
        LengthValue::Vmin(value)
        | LengthValue::Lvmin(value)
        | LengthValue::Svmin(value)
        | LengthValue::Dvmin(value) => (
            value,
            environment
                .viewport_width()
                .min(environment.viewport_height()?),
        ),
        LengthValue::Vmax(value)
        | LengthValue::Lvmax(value)
        | LengthValue::Svmax(value)
        | LengthValue::Dvmax(value) => (
            value,
            environment
                .viewport_width()
                .max(environment.viewport_height()?),
        ),
        _ => return None,
    };
    Some(f64::from(*value) / 100.0 * basis)
}

#[cfg(test)]
mod tests {
    use super::{MatchResult, MediaEnvironment, MediaQuerySet, evaluate_media_list};
    use lightningcss::rules::CssRule;
    use lightningcss::stylesheet::ParserOptions;
    use lightningcss::stylesheet::StyleSheet;
    use std::sync::Arc;

    fn evaluate(query: &str, width: f64, height: Option<f64>) -> MatchResult {
        let css = format!("@media {query} {{ p {{ color: red }} }}");
        let stylesheet =
            StyleSheet::parse(&css, ParserOptions::default()).expect("media query parses");
        let CssRule::Media(rule) = &stylesheet.rules.0[0] else {
            panic!("expected a media rule")
        };
        evaluate_media_list(
            &rule.query,
            MediaEnvironment::screen(width, height).unwrap(),
            16.0,
        )
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

    #[test]
    fn native_media_values_keep_font_viewport_and_physical_unit_bases() {
        for (query, width, height) in [
            ("(width: 40em)", 640.0, 800.0),
            ("(width: 40rem)", 640.0, 800.0),
            ("(width: 100svw)", 640.0, 800.0),
            ("(width: 80lvh)", 640.0, 800.0),
            ("(width: 100dvi)", 640.0, 800.0),
            ("(width: 100vmin)", 640.0, 800.0),
            ("(width: 100dvmax)", 800.0, 640.0),
            ("(width: 2in)", 192.0, 800.0),
            ("(width: 144pt)", 192.0, 800.0),
            ("(color-gamut: SRGB)", 640.0, 800.0),
        ] {
            assert_eq!(
                evaluate(query, width, Some(height)),
                MatchResult::True,
                "{query}"
            );
        }
        assert_eq!(
            evaluate("(width: 10ex)", 640.0, Some(800.0)),
            MatchResult::Unknown
        );
        assert_eq!(
            evaluate("(color-gamut: other)", 640.0, Some(800.0)),
            MatchResult::Unknown
        );
    }

    #[test]
    fn unavailable_media_values_keep_three_valued_boolean_logic() {
        assert_eq!(
            evaluate("(width > 0px), (height > 0px)", 600.0, None),
            MatchResult::True
        );
        assert_eq!(
            evaluate("(width < 0px) and (height > 0px)", 600.0, None),
            MatchResult::False
        );
        assert_eq!(
            evaluate("not ((width > 0px) and (height > 0px))", 600.0, None),
            MatchResult::Unknown
        );
    }

    #[test]
    fn media_dependencies_own_native_queries_after_source_is_dropped() {
        let mut dependencies = MediaQuerySet::default();
        {
            let css =
                String::from("@media (width >= 40em) and (color-gamut: srgb) { p { color: red } }");
            let stylesheet = StyleSheet::parse(&css, ParserOptions::default()).unwrap();
            let CssRule::Media(rule) = &stylesheet.rules.0[0] else {
                panic!("expected a media rule")
            };
            let id = dependencies.register_query(&rule.query);
            dependencies.record_dependency(&[id]);
        }
        let key =
            |width| dependencies.match_key(MediaEnvironment::screen(width, None).unwrap(), 16.0);
        assert_ne!(key(639.0), key(640.0));
        assert_eq!(key(640.0), key(800.0));
    }

    #[test]
    fn cloned_dependencies_share_queries_and_isolate_document_flags_and_builder_changes() {
        let stylesheet = StyleSheet::parse(
            "@media (min-width:500px) { p { color:red } } @media (max-width:700px) { p { color:blue } }",
            ParserOptions::default(),
        ).unwrap();
        let CssRule::Media(first) = &stylesheet.rules.0[0] else {
            panic!("media rule")
        };
        let CssRule::Media(second) = &stylesheet.rules.0[1] else {
            panic!("media rule")
        };
        let mut dependencies = MediaQuerySet::default();
        let first = dependencies.register_query(&first.query);
        dependencies.record_dependency(&[first]);
        let snapshot = dependencies.clone();
        let shared = snapshot.definitions.as_ref().unwrap();
        let environment = MediaEnvironment::screen(600.0, None).unwrap();
        let key = snapshot.match_key(environment, 16.0);
        assert!(Arc::ptr_eq(
            dependencies.definitions.as_ref().unwrap(),
            shared
        ));

        dependencies.record_dependency(&[first]);
        dependencies.mark_viewport_unit_dependency();
        assert!(Arc::ptr_eq(
            dependencies.definitions.as_ref().unwrap(),
            shared
        ));
        assert!(dependencies.uses_viewport_units());
        assert!(!snapshot.uses_viewport_units());
        assert!(dependencies.clone().uses_viewport_units());

        let second = dependencies.register_query(&second.query);
        dependencies.record_dependency(&[first, second]);
        drop(stylesheet);
        assert!(!Arc::ptr_eq(
            dependencies.definitions.as_ref().unwrap(),
            shared
        ));
        assert_eq!(snapshot.match_key(environment, 16.0), key);
        assert_ne!(dependencies.match_key(environment, 16.0), key);
    }

}
