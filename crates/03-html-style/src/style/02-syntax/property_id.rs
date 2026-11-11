//! Property identities for CSS that Lightning CSS preserves as raw tokens.

use crate::style::property_catalogue::PropertyKey;
use cssparser::match_ignore_ascii_case;
use lightningcss::properties::custom::TokenList;
use lightningcss::properties::{Property, PropertyId};

/// Send aliases through their existing native property grammar.
pub(crate) fn canonical_property_id(id: PropertyId<'_>) -> PropertyId<'_> {
    RendererPropertyId::from_lightning(&id)
        .and_then(RendererPropertyId::native_property)
        .unwrap_or(id)
}

macro_rules! native_property_value {
    () => {
        None
    };
    ($native:ident) => {
        Some(PropertyId::$native)
    };
}

macro_rules! renderer_property_ids {
    ($($key:ident => $name:literal, $inherited:literal, $phase:ident, [$($field:ident $(.$member:ident)*),* $(,)?], $copy:ident $(($($arg:tt)*))?, [$($renderer:ident => $renderer_name:literal $(=> $native:ident)?),*];)+) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
        pub(crate) enum RendererPropertyId { $($($renderer,)*)+ }
        impl RendererPropertyId {
            pub(crate) fn from_name(name: &str) -> Option<Self> {
                match_ignore_ascii_case! { name,
                    $($($renderer_name => Some(Self::$renderer),)*)+
                    _ => None,
                }
            }
            pub(crate) fn name(self) -> &'static str {
                match self { $($(Self::$renderer => $renderer_name,)*)+ }
            }
            pub(crate) fn key(self) -> PropertyKey {
                match self { $($(Self::$renderer => PropertyKey::$key,)*)+ }
            }
            pub(crate) fn native_property(self) -> Option<PropertyId<'static>> {
                match self { $($(Self::$renderer => native_property_value!($($native)?),)*)+ }
            }
        }
    };
}
crate::style::property_catalogue::property_catalogue!(renderer_property_ids);

impl RendererPropertyId {
    pub(crate) fn from_lightning(id: &PropertyId<'_>) -> Option<Self> {
        match id {
            PropertyId::LetterSpacing => Some(Self::LetterSpacing),
            PropertyId::WordSpacing => Some(Self::WordSpacing),
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
}
