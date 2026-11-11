//! Lazily bind general-matcher name predicates to one immutable document.
use super::dom::{attribute_value_matches, parsed_case_sensitivity};
use html_dom::{Document, ElementRef, storage};
use lightningcss::selector::Component;
use rustc_data_structures::fx::FxHashMap;

#[derive(Clone, Copy)]
struct BoundName {
    exact: Option<u16>,
    lower: Option<u16>,
    mask: u32,
}

#[derive(Default)]
pub(super) struct GeneralNames([FxHashMap<String, BoundName>; 2]);

impl GeneralNames {
    pub(super) fn matches(
        &mut self,
        component: &Component<'_>,
        document: &Document,
        element: ElementRef<'_>,
    ) -> Option<bool> {
        if !matches!(
            component,
            Component::LocalName(_)
                | Component::ID(_)
                | Component::Class(_)
                | Component::AttributeInNoNamespaceExists { .. }
                | Component::AttributeInNoNamespace { .. }
        ) {
            return None;
        }
        let (name, lower, attribute) = match component {
            Component::LocalName(name) => {
                (name.name.as_ref(), Some(name.lower_name.as_ref()), false)
            }
            Component::ID(name) | Component::Class(name) => (name.as_ref(), None, false),
            Component::AttributeInNoNamespaceExists {
                local_name,
                local_name_lower,
            } => (local_name.as_ref(), Some(local_name_lower.as_ref()), true),
            Component::AttributeInNoNamespace { local_name, .. } => {
                (local_name.as_ref(), None, true)
            }
            _ => unreachable!("name predicate checked above"),
        };
        let cache = &mut self.0[usize::from(attribute)];
        let cached = cache.get(name).copied();
        let bound = cached.unwrap_or_else(|| {
            let exact = document.lookup_string(name);
            let lower = if let Some(lower) = lower {
                document.lookup_string(lower)
            } else if name.bytes().any(|byte| byte.is_ascii_uppercase()) {
                document.lookup_string(&name.to_ascii_lowercase())
            } else {
                exact
            };
            let bound = BoundName {
                exact,
                lower,
                mask: storage::selector_name_mask(name, attribute),
            };
            cache.insert(name.to_owned(), bound);
            bound
        });
        let html = element.is_html_element_in_html_document();
        let name_idx = if html { bound.lower } else { bound.exact };
        Some(match component {
            Component::LocalName(_) => name_idx == Some(element.tag_idx()),
            Component::ID(_) => bound.exact.is_some() && bound.exact == element.id_idx(),
            Component::Class(_) => bound
                .exact
                .is_some_and(|index| element.has_class_idx(index, bound.mask)),
            Component::AttributeInNoNamespaceExists {
                local_name,
                local_name_lower,
            } => {
                let name = if html {
                    local_name_lower.as_ref()
                } else {
                    local_name.as_ref()
                };
                element
                    .attr_with_name_idx(name, name_idx, bound.mask)
                    .is_some()
            }
            Component::AttributeInNoNamespace {
                local_name,
                operator,
                value,
                case_sensitivity,
                ..
            } => {
                let name = local_name.as_ref();
                element
                    .attr_with_name_idx(name, name_idx, bound.mask)
                    .is_some_and(|actual| {
                        attribute_value_matches(
                            *operator,
                            actual,
                            value.as_ref(),
                            parsed_case_sensitivity(*case_sensitivity, html, Some(name)),
                        )
                    })
            }
            _ => unreachable!("name predicate checked above"),
        })
    }
}
