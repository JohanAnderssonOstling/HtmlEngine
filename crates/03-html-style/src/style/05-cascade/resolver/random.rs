//! Document-stable random base values, shared by CSS random functions.

use super::custom_properties::token_list_to_css_string;
use crate::style::syntax::random_key::{Key, PropertyScope, parse_key};
use crate::style::syntax::value::NumberValue;
use html_dom::{Document, DomNodeId};
use lightningcss::traits::Parse;
use lightningcss::values::calc::Calc;
use lightningcss::values::length::Length;
use std::cell::Cell;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hash, Hasher};
use std::sync::LazyLock;

static RANDOM_SEED: LazyLock<RandomState> = LazyLock::new(RandomState::new);

pub(super) struct RandomContext {
    document: u64,
    element: Cell<Option<DomNodeId>>,
    pseudo: Cell<u8>,
    used_element: Cell<bool>,
}

impl RandomContext {
    pub(super) fn new(doc: &Document) -> Self {
        let mut hasher = RANDOM_SEED.build_hasher();
        doc.lineage().hash(&mut hasher);
        Self {
            document: hasher.finish(),
            element: Cell::new(None),
            pseudo: Cell::new(0),
            used_element: Cell::new(false),
        }
    }

    pub(super) fn begin_element(&self, node: DomNodeId) {
        self.element.set(Some(node));
        self.pseudo.set(0);
        self.used_element.set(false);
    }

    pub(super) fn set_pseudo(&self, mask: u8) {
        self.pseudo.set(mask);
    }

    pub(super) fn used_element(&self) -> bool {
        self.used_element.get()
    }

    pub(super) fn base(&self, source: &str, property_name: &str, index: usize) -> Option<f64> {
        match parse_key(source)? {
            Key::Fixed(value) => {
                let value = match value {
                    NumberValue::Number(number) => number,
                    NumberValue::Calculation(tokens) => {
                        let css = token_list_to_css_string(&tokens)?;
                        let Calc::Number(number) = Calc::<Length>::parse_string(&css).ok()? else {
                            return None;
                        };
                        number.max(0.0).min(1.0)
                    }
                };
                Some(f64::from(value).min(f64::from_bits(1.0f64.to_bits() - 1)))
            }
            Key::Cache {
                name,
                element,
                property,
            } => {
                let property = match property {
                    PropertyScope::None => None,
                    PropertyScope::Property => Some(format!("ua-{property_name}")),
                    PropertyScope::Index => Some(format!("ua-{property_name}-{index}")),
                    PropertyScope::User(value) => Some(value),
                };
                let element = if element {
                    self.used_element.set(true);
                    Some((
                        self.element
                            .get()
                            .expect("random values require an originating element"),
                        self.pseudo.get(),
                    ))
                } else {
                    None
                };
                let mut hasher = RANDOM_SEED.build_hasher();
                (self.document, name, property, element).hash(&mut hasher);
                // Use exactly representable f32 fractions, matching native
                // CSS calculation precision and random-item's shared base.
                Some((hasher.finish() >> 40) as f64 / (1u64 << 24) as f64)
            }
        }
    }
}
