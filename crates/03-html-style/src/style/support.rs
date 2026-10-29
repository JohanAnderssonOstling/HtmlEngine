//! Public declaration-support data shared by the API and native adapters.

/// Result of checking a declaration against the CSS parser owned by this
/// rendering stage. Unknown properties are kept distinct from invalid values
/// so conformance adapters cannot accidentally count them as passes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PropertyValueSyntax {
    Valid,
    Invalid,
    UnsupportedProperty,
    /// The property is implemented, but this value needs an unsupported feature.
    UnsupportedValue,
}

/// Syntax of an implemented declaration. Unsupported features are reported
/// explicitly without validating their complete grammar.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PropertySyntax {
    Valid,
    Invalid,
    /// This renderer does not implement the feature, so its syntax was not checked.
    Unsupported,
}

/// Whether the style stage implements the property's computed behavior.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PropertyCapability {
    Supported,
    Unsupported(UnsupportedStyleFeature),
}

/// Why syntactically recognizable CSS cannot participate in this renderer's
/// cascade. Keeping the reason in the type prevents parser acceptance from
/// being mistaken for rendered support.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnsupportedStyleFeature {
    Property,
    Value,
    Gradient,
    GeneratedContent,
    BackgroundImage,
    BorderImage,
    BoxShadow,
    FilterEffects,
    Transitions,
    Animations,
}

/// Renderer capability and, for implemented features, declaration syntax.
/// Feature queries and conformance adapters must reject unsupported features.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeclarationSupport {
    pub syntax: PropertySyntax,
    pub capability: PropertyCapability,
}
