//! Stylesheet syntax delegated to Lightning CSS.
use super::selectors::selector_list_is_web_valid_for_scope;
use lightningcss::rules::CssRule;
use lightningcss::stylesheet::{ParserOptions, StyleSheet};
use lightningcss::visitor::{Visit, VisitTypes, Visitor};

pub(crate) fn stylesheet_is_valid(css: &str) -> bool {
    StyleSheet::parse(css, ParserOptions::default())
        .is_ok_and(|mut sheet| sheet.visit(&mut ScopeValidation).is_ok())
}

struct ScopeValidation;

impl<'i> Visitor<'i> for ScopeValidation {
    type Error = ();

    fn visit_types(&self) -> VisitTypes {
        VisitTypes::RULES
    }

    fn visit_rule(&mut self, rule: &mut CssRule<'i>) -> Result<(), Self::Error> {
        if let CssRule::Scope(scope) = rule
            && (!scope
                .scope_start
                .as_ref()
                .is_none_or(selector_list_is_web_valid_for_scope)
                || !scope
                    .scope_end
                    .as_ref()
                    .is_none_or(selector_list_is_web_valid_for_scope))
        {
            return Err(());
        }
        rule.visit_children(self)
    }
}
