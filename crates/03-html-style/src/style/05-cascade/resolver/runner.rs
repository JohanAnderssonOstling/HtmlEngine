//! Winner selection and dependency-ordered computed-value conversion.

use super::*;
use specified::{
    PropertyTarget, SelectedDeclaration, SpecifiedSelection, build_cascade_events,
    select_specified_values,
};

/// The cascade computes dependency roots before values that consume them.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum CascadePhase {
    Prerequisites,
    Remaining,
}

pub(super) struct CascadeInputs<'a, 'css> {
    pub(super) normal_rules: &'a [MatchedRule],
    pub(super) important_rules: &'a [MatchedRule],
    pub(super) parent_font_size: f32,
    pub(super) custom_properties: &'a FxHashMap<String, TokenList<'css>>,
    pub(super) parent: &'a ParentStyle,
    pub(super) inline_style: Option<&'a StyleAttribute<'css>>,
    pub(super) presentational_hints_node: Option<DomNodeId>,
}

fn target_phase(target: &PropertyTarget) -> CascadePhase {
    if matches!(
        target.name.as_str(),
        "direction" | "font-size" | "line-height" | "color"
    ) {
        CascadePhase::Prerequisites
    } else {
        CascadePhase::Remaining
    }
}

fn wide_keyword<'property, 'css>(property: &'property Property<'css>) -> Option<&'property str> {
    let keyword = match property {
        Property::Unparsed(value) => single_ident_keyword(&value.value),
        Property::Custom(value) if !value.name.as_ref().starts_with("--") => {
            single_ident_keyword(&value.value)
        }
        _ => None,
    }?;
    ["inherit", "initial", "unset", "revert", "revert-layer"]
        .iter()
        .any(|candidate| keyword.eq_ignore_ascii_case(candidate))
        .then_some(keyword)
}

impl<'a, 'sheet, 'css> StyleResolverContext<'a, 'sheet, 'css> {
    fn apply_presentational_hints(
        &mut self,
        style: &mut WorkingStyle,
        node: DomNodeId,
        phase: CascadePhase,
        selection: &SpecifiedSelection<'_, '_>,
        parent: &ParentStyle,
    ) {
        if selection.reverted_hint_targets.is_empty() && !selection.reverted_all_hints {
            html_presentational_hints::apply(self.doc, node, style, phase);
            return;
        }
        let before = style.clone();
        html_presentational_hints::apply(self.doc, node, style, phase);
        if selection.reverted_all_hints {
            // `all` excludes direction. Preserve that hint while rolling back
            // every other presentation-hint property.
            let hinted_direction = style.text.direction;
            *style = before;
            style.text.direction = hinted_direction;
            resolve_logical_text_alignments(&mut style.text);
            return;
        }
        for name in &selection.reverted_hint_targets {
            let _ = apply_css_wide_keyword_in_phase(
                style,
                name,
                "revert",
                parent,
                self.doc.root_font_size(),
                phase,
                Some(&before),
                Some(&before),
            );
        }
    }

    fn declaration_is_eligible(&self, property: &Property<'_>) -> bool {
        if matches!(property, Property::All(_)) || wide_keyword(property).is_some() {
            return true;
        }
        if matches!(property, Property::Unparsed(value) if token_list_contains_var(&value.value))
            || matches!(property, Property::Custom(value) if token_list_contains_var(&value.value))
        {
            return true;
        }
        if crate::style::syntax::capabilities::property_uses_unsupported_text_decoration_style(
            property,
        ) || crate::style::syntax::capabilities::property_uses_unsupported_outline_style(
            property,
        ) {
            return false;
        }
        if matches!(property, Property::Unparsed(_) | Property::Custom(_)) {
            let Ok(value) = property.value_to_css_string(PrinterOptions::default()) else {
                return false;
            };
            let support = crate::declaration_support(property.property_id().name(), &value);
            if support.syntax == crate::PropertySyntax::Invalid
                || matches!(
                    support.capability,
                    crate::PropertyCapability::Unsupported(
                        crate::UnsupportedStyleFeature::Gradient
                    )
                )
            {
                return false;
            }
        }
        true
    }

    fn declaration_is_computable(
        &mut self,
        style: &WorkingStyle,
        property: &Property<'_>,
        parent_font_size: f32,
        parent: &ParentStyle,
    ) -> bool {
        if matches!(property, Property::All(_))
            || wide_keyword(property).is_some()
            || matches!(property, Property::Unparsed(value) if token_list_contains_var(&value.value))
            || matches!(property, Property::Custom(value) if token_list_contains_var(&value.value))
        {
            return true;
        }
        property_is_computable(
            self.doc,
            self.styles,
            style,
            property,
            parent_font_size,
            parent,
            self.prepared.environment(),
        )
    }

    fn apply_computed_property(
        &mut self,
        style: &mut WorkingStyle,
        property: &Property<'_>,
        target: &PropertyTarget,
        parent_font_size: f32,
        phase: CascadePhase,
        parent: &ParentStyle,
    ) {
        if target.name != "all" && target_phase(target) != phase {
            return;
        }
        if let Some(keyword) = wide_keyword(property) {
            let _ = apply_css_wide_keyword_in_phase(
                style,
                &target.name,
                keyword,
                parent,
                self.doc.root_font_size(),
                phase,
                None,
                None,
            );
            return;
        }

        let longhand;
        let property = if let Some(property_id) = &target.longhand {
            if let Some(value) = property.longhand(property_id) {
                longhand = value;
                &longhand
            } else {
                // A few Lightning CSS shorthands advertise a longhand that
                // their extractor cannot materialize. Replaying the shorthand
                // is safe because higher winning longhands execute later.
                property
            }
        } else {
            property
        };
        apply_property_in_phase(
            self.doc,
            self.styles,
            style,
            property,
            parent_font_size,
            self.prepared.environment(),
            phase,
            parent,
        );
    }

    fn apply_selected_declaration<'event>(
        &mut self,
        style: &mut WorkingStyle,
        declaration: &SelectedDeclaration<'event, 'css>,
        parent_font_size: f32,
        var_map: &HashMap<&str, TokenList<'css>>,
        phase: CascadePhase,
        parent: &ParentStyle,
    ) {
        let resolved_unparsed;
        let resolved_custom;
        let substituted = matches!(declaration.property, Property::Unparsed(value) if token_list_contains_var(&value.value))
            || matches!(declaration.property, Property::Custom(value) if token_list_contains_var(&value.value));
        let resolved = match declaration.property {
            Property::Unparsed(unparsed) if token_list_contains_var(&unparsed.value) => {
                let mut substitutable = unparsed.clone();
                mark_var_substitution_boundaries(&mut substitutable.value);
                resolved_unparsed = substitutable
                    .substitute_variables(var_map)
                    .ok()
                    .or_else(|| resolve_single_var_property(unparsed, var_map));
                let Some(property) = resolved_unparsed.as_ref() else {
                    for target in &declaration.targets {
                        if target.name == "all" || target_phase(target) == phase {
                            let _ = apply_css_wide_keyword_in_phase(
                                style,
                                &target.name,
                                "unset",
                                parent,
                                self.doc.root_font_size(),
                                phase,
                                None,
                                None,
                            );
                        }
                    }
                    return;
                };
                property
            }
            Property::Custom(custom)
                if !custom.name.as_ref().starts_with("--")
                    && token_list_contains_var(&custom.value) =>
            {
                let mut value = custom.clone();
                mark_var_substitution_boundaries(&mut value.value);
                value.value.substitute_variables(var_map);
                if token_list_contains_var(&value.value) {
                    for target in &declaration.targets {
                        if target.name == "all" || target_phase(target) == phase {
                            let _ = apply_css_wide_keyword_in_phase(
                                style,
                                &target.name,
                                "unset",
                                parent,
                                self.doc.root_font_size(),
                                phase,
                                None,
                                None,
                            );
                        }
                    }
                    return;
                }
                resolved_custom = Property::Custom(value);
                &resolved_custom
            }
            property => property,
        };

        // An unparsed value remaining after substitution is invalid at
        // computed-value time. It still won the cascade, so each affected
        // longhand becomes `unset` rather than exposing a losing declaration.
        if substituted
            && wide_keyword(resolved).is_none()
            && (matches!(resolved, Property::Unparsed(_))
                || !self.declaration_is_eligible(resolved)
                || !self.declaration_is_computable(style, resolved, parent_font_size, parent))
        {
            for target in &declaration.targets {
                if target.name == "all" || target_phase(target) == phase {
                    let _ = apply_css_wide_keyword_in_phase(
                        style,
                        &target.name,
                        "unset",
                        parent,
                        self.doc.root_font_size(),
                        phase,
                        None,
                        None,
                    );
                }
            }
            return;
        }
        for target in &declaration.targets {
            self.apply_computed_property(style, resolved, target, parent_font_size, phase, parent);
        }
    }

    pub(super) fn apply_cascade(
        &mut self,
        style: &mut WorkingStyle,
        inputs: CascadeInputs<'_, 'css>,
    ) {
        let CascadeInputs {
            normal_rules,
            important_rules,
            parent_font_size,
            custom_properties,
            parent,
            inline_style,
            presentational_hints_node,
        } = inputs;
        let (events, hints_sequence) =
            build_cascade_events(self.prepared, normal_rules, important_rules, inline_style);

        let mut valid_events = Vec::with_capacity(events.len());
        let mut valid_hints_sequence = 0;
        for (sequence, event) in events.into_iter().enumerate() {
            if self.declaration_is_eligible(event.property) {
                if sequence < hints_sequence {
                    valid_hints_sequence += 1;
                }
                valid_events.push(event);
            }
        }

        // Validate only declarations that can actually win. If a renderer
        // conversion rejects one, remove it and expose the next candidate.
        // Ordinary cascades therefore avoid dry-running every overridden
        // declaration.
        let selection = loop {
            let selection = select_specified_values(&valid_events);
            let mut invalid: Vec<_> = selection
                .declarations
                .iter()
                .filter(|declaration| {
                    !self.declaration_is_computable(
                        style,
                        declaration.property,
                        parent_font_size,
                        parent,
                    )
                })
                .map(|declaration| declaration.sequence)
                .collect();
            if invalid.is_empty() {
                break selection;
            }
            invalid.sort_unstable();
            invalid.dedup();
            for sequence in invalid.into_iter().rev() {
                if sequence < valid_hints_sequence {
                    valid_hints_sequence -= 1;
                }
                valid_events.remove(sequence);
            }
        };
        let var_map = custom_properties
            .iter()
            .map(|(name, value)| (name.as_str(), value.clone()))
            .collect();
        for phase in [CascadePhase::Prerequisites, CascadePhase::Remaining] {
            let mut hints_applied = false;
            for declaration in &selection.declarations {
                if !hints_applied && declaration.sequence >= valid_hints_sequence {
                    if let Some(node) = presentational_hints_node {
                        self.apply_presentational_hints(style, node, phase, &selection, parent);
                    }
                    hints_applied = true;
                }
                self.apply_selected_declaration(
                    style,
                    declaration,
                    parent_font_size,
                    &var_map,
                    phase,
                    parent,
                );
            }
            if !hints_applied {
                if let Some(node) = presentational_hints_node {
                    self.apply_presentational_hints(style, node, phase, &selection, parent);
                }
            }
        }
    }
}
