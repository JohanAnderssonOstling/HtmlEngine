//! Small selector summaries used only to reject impossible matches.
use crate::style::syntax::selectors::ancestor_hash;
use html_dom::ElementRef;
use lightningcss::selector::Component;

#[derive(Clone, Copy, Default)]
pub struct SelectorFilter {
    bits: [u64; 4],
}

impl SelectorFilter {
    #[inline]
    pub fn insert(&mut self, name: &str) {
        let hash = ancestor_hash(name);
        for shift in [0, 16] {
            let bit = ((hash >> shift) & 255) as usize;
            self.bits[bit / 64] |= 1 << (bit % 64);
        }
    }

    #[inline]
    pub(crate) fn contains(&self, requirements: Self) -> bool {
        self.bits
            .iter()
            .zip(requirements.bits)
            .all(|(actual, required)| actual & required == required)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.bits == [0; 4]
    }

    pub(super) fn insert_element(&mut self, element: ElementRef<'_>) {
        self.insert(element.tag());
        if let Some(id) = element.id() {
            self.insert(id);
        }
        for class in element.classes() {
            self.insert(class);
        }
        for attribute in element.attributes() {
            self.insert(attribute.local_name());
        }
    }

    /// Only unconditional positive tests in the rightmost compound are safe.
    /// Logical arguments may refer outside the subtree; negations add no facts.
    pub(super) fn for_subject(components: &[Component<'_>]) -> Self {
        let mut filter = Self::default();
        for component in components
            .iter()
            .take_while(|component| !component.is_combinator())
        {
            match component {
                Component::LocalName(name) => filter.insert(name.lower_name.as_ref()),
                Component::ID(id) | Component::Class(id) => filter.insert(id.as_ref()),
                Component::AttributeInNoNamespaceExists { local_name, .. }
                | Component::AttributeInNoNamespace { local_name, .. } => {
                    filter.insert(local_name.as_ref())
                }
                Component::AttributeOther(attribute) => {
                    filter.insert(attribute.local_name.as_ref())
                }
                _ => {}
            }
        }
        filter
    }
}
