//! Property identities for CSS that Lightning CSS preserves as raw tokens.

use crate::style::property_catalogue::PropertyKey;
use cssparser::match_ignore_ascii_case;
use lightningcss::properties::custom::TokenList;
use lightningcss::properties::{Property, PropertyId};

macro_rules! renderer_property_ids {
    ($($variant:ident => $name:literal),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
        pub(crate) enum RendererPropertyId {
            $($variant),+
        }

        impl RendererPropertyId {
            /// Resolve a CSS name at the raw declaration boundary. Custom
            /// properties retain their own case-sensitive identity.
            pub(crate) fn from_name(name: &str) -> Option<Self> {
                match_ignore_ascii_case! { name,
                    $($name => Some(Self::$variant),)+
                    _ => None,
                }
            }

            pub(crate) fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => $name),+
                }
            }
        }
    };
}

renderer_property_ids! {
    Float => "float",
    Clear => "clear",
    ObjectFit => "object-fit",
    ObjectPosition => "object-position",
    OutlineOffset => "outline-offset",
    TableLayout => "table-layout",
    BorderCollapse => "border-collapse",
    CaptionSide => "caption-side",
    EmptyCells => "empty-cells",
    FontKerning => "font-kerning",
    FontFeatureSettings => "font-feature-settings",
    FontVariantNumeric => "font-variant-numeric",
    FontVariantLigatures => "font-variant-ligatures",
    TextBox => "text-box",
    TextBoxTrim => "text-box-trim",
    TextBoxEdge => "text-box-edge",
    Contain => "contain",
    BreakBefore => "break-before",
    BreakAfter => "break-after",
    BreakInside => "break-inside",
    PageBreakBefore => "page-break-before",
    PageBreakAfter => "page-break-after",
    PageBreakInside => "page-break-inside",
    Widows => "widows",
    Orphans => "orphans",
    Content => "content",
    CounterReset => "counter-reset",
    CounterIncrement => "counter-increment",
    Quotes => "quotes",
    GridGap => "grid-gap",
    GridRowGap => "grid-row-gap",
    GridColumnGap => "grid-column-gap",
}

impl RendererPropertyId {
    pub(crate) fn from_lightning(id: &PropertyId<'_>) -> Option<Self> {
        match id {
            PropertyId::Custom(name) => Self::from_name(name.as_ref()),
            _ => None,
        }
    }

    pub(crate) fn raw<'p, 'css>(
        property: &'p Property<'css>,
    ) -> Option<(Self, &'p TokenList<'css>)> {
        match property {
            Property::Custom(value) => Some((Self::from_name(value.name.as_ref())?, &value.value)),
            Property::Unparsed(value) => {
                Some((Self::from_lightning(&value.property_id)?, &value.value))
            }
            _ => None,
        }
    }

    pub(crate) fn key(self) -> PropertyKey {
        PropertyKey::from_name(self.name()).expect("renderer property is catalogued")
    }

    pub(crate) fn gap_property(self) -> Option<PropertyId<'static>> {
        match self {
            Self::GridGap => Some(PropertyId::Gap),
            Self::GridRowGap => Some(PropertyId::RowGap),
            Self::GridColumnGap => Some(PropertyId::ColumnGap),
            _ => None,
        }
    }
}
