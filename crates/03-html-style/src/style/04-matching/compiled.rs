//! Execute compact selector instructions against document nodes.
use crate::style::compiled::{
    CompiledSelectors, EffectiveRuleId, FastTest, PreparedSelector, Relation,
};
use html_dom::{Document, DomNodeId};

impl CompiledSelectors {
    pub(crate) fn rule_count(&self) -> usize {
        self.rule_starts.len().saturating_sub(1)
    }

    pub(crate) fn for_rule(&self, id: EffectiveRuleId) -> &[PreparedSelector] {
        let start = self.rule_starts[id.index()] as usize;
        let end = self.rule_starts[id.index() + 1] as usize;
        &self.selectors[start..end]
    }

    #[inline]
    pub(crate) fn matches(
        &self,
        selector: PreparedSelector,
        doc: &Document,
        node: DomNodeId,
    ) -> Option<bool> {
        match selector.fast_len {
            0 => None,
            1 => Some(self.matches_compound(selector.fast_start as usize, doc, node)),
            _ => Some(self.matches_from(selector.fast_start as usize, doc, node)),
        }
    }

    #[inline]
    fn matches_compound(&self, compound_index: usize, doc: &Document, node: DomNodeId) -> bool {
        let Some(element) = doc.element_ref(node) else {
            return false;
        };
        let compound = self.compounds[compound_index];
        for test in &self.tests[compound.test_start as usize
            ..compound.test_start as usize + compound.test_len as usize]
        {
            let matches = match test {
                FastTest::LocalName { name, lower_name } => {
                    let expected = if element.is_html_element_in_html_document() {
                        lower_name.as_ref()
                    } else {
                        name.as_ref()
                    };
                    element.tag() == expected
                }
                FastTest::Id(id) => element.id().is_some_and(|value| value == id.as_ref()),
                FastTest::Class(class) => element.has_class(class.as_ref()),
            };
            if !matches {
                return false;
            }
        }
        true
    }

    fn matches_from(&self, compound_index: usize, doc: &Document, node: DomNodeId) -> bool {
        if !self.matches_compound(compound_index, doc, node) {
            return false;
        }
        let element = doc
            .element_ref(node)
            .expect("a matching compact compound belongs to an element");
        match self.compounds[compound_index].relation {
            Relation::End => true,
            Relation::Parent => element
                .parent()
                .is_some_and(|parent| self.matches_from(compound_index + 1, doc, parent)),
            Relation::Ancestor => doc
                .dom_ancestors(node)
                .any(|ancestor| self.matches_from(compound_index + 1, doc, ancestor)),
        }
    }
}
