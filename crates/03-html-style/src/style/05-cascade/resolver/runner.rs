//! Shared two-phase cascade execution for elements and pseudo-elements.

use super::*;

/// The cascade runs in dependency-ordered passes. `direction` is finalized
/// before logical properties are mapped, and `font-size`/`color` are finalized
/// before dependent lengths and `currentColor` are resolved.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum CascadePhase {
    Prerequisites,
    Remaining,
}

pub(super) struct CascadeInputs<'a, 'css> {
    pub(super) normal_rules: &'a [MatchedRule],
    pub(super) important_rules: &'a [MatchedRule],
    pub(super) rollback_layers: &'a [RulePriority],
    pub(super) rollback_origins: &'a [CascadeOrigin],
    pub(super) parent_font_size: f32,
    pub(super) custom_properties: &'a FxHashMap<String, TokenList<'css>>,
    pub(super) parent: &'a ParentStyle,
    pub(super) inline_style: Option<&'a StyleAttribute<'css>>,
    pub(super) presentational_hints_node: Option<DomNodeId>,
}

/// Apply CSS declarations to a computed style
impl<'a, 'sheet, 'css> StyleResolverContext<'a, 'sheet, 'css> {
    fn apply_declarations<'declaration>(
        &mut self,
        style: &mut WorkingStyle,
        declarations: &[Property<'declaration>],
        parent_font_size: f32,
        custom_properties: Option<&FxHashMap<String, TokenList<'declaration>>>,
        phase: CascadePhase,
        parent: &ParentStyle,
        revert_basis: &WorkingStyle,
        revert_layer_basis: &WorkingStyle,
    ) {
        let doc = self.doc;
        let styles = &mut *self.styles;
        let mut var_map: HashMap<&str, TokenList<'declaration>> = HashMap::new();
        if let Some(map) = custom_properties {
            for (name, value) in map {
                var_map.insert(name.as_str(), value.clone());
            }
        }

        for property in declarations {
            match property {
                Property::Custom(custom) => {
                    let raw_name = custom.name.as_ref();
                    if raw_name.starts_with("--") {
                        // Custom properties were already cascaded, inherited,
                        // and resolved for CSS-wide keywords into `var_map`.
                        // Re-inserting this raw declaration would undo such
                        // resolution (for example, restoring literal `initial`).
                        continue;
                    }
                }
                Property::Unparsed(unparsed) if unparsed.property_id.name().starts_with("--") => {
                    continue;
                }
                _ => {}
            }

            if let Property::Unparsed(unparsed) = property
                && token_list_contains_var(&unparsed.value)
            {
                let mut substitutable = unparsed.clone();
                mark_var_substitution_boundaries(&mut substitutable.value);
                let resolved = substitutable
                    .substitute_variables(&var_map)
                    .ok()
                    .or_else(|| resolve_single_var_property(unparsed, &var_map));
                if let Some(resolved) = resolved {
                    if let Property::Unparsed(value) = &resolved
                        && let Some(keyword) = single_ident_keyword(&value.value)
                        && apply_css_wide_keyword_in_phase(
                            style,
                            value.property_id.name(),
                            keyword,
                            parent,
                            doc.root_font_size(),
                            phase,
                            None,
                            None,
                        )
                    {
                        continue;
                    }
                    // A supported typed property only remains `Unparsed` when
                    // the substituted component values do not match its
                    // grammar. The declaration is invalid at computed-value
                    // time even though LightningCSS preserves it for output.
                    let invalid_after_substitution = matches!(&resolved, Property::Unparsed(_));
                    set_line_height_resolution_bases(doc, &*styles, &resolved, style, parent);
                    if invalid_after_substitution
                        || property_has_faulty_numeric_value(
                            doc,
                            style,
                            &resolved,
                            parent_font_size,
                            self.prepared.environment(),
                        )
                    {
                        // A declaration containing var() has already won the
                        // cascade. If substitution produces an invalid computed
                        // value, CSS requires `unset` rather than exposing the
                        // declaration that lost earlier in the cascade.
                        let _ = apply_css_wide_keyword_in_phase(
                            style,
                            unparsed.property_id.name(),
                            "unset",
                            parent,
                            doc.root_font_size(),
                            phase,
                            None,
                            None,
                        );
                    } else {
                        apply_property_in_phase(
                            doc,
                            styles,
                            style,
                            &resolved,
                            parent_font_size,
                            self.prepared.environment(),
                            phase,
                            parent,
                            revert_basis,
                            revert_layer_basis,
                        );
                    }
                } else {
                    // Missing variables without a usable fallback are likewise
                    // invalid at computed-value time.
                    let _ = apply_css_wide_keyword_in_phase(
                        style,
                        unparsed.property_id.name(),
                        "unset",
                        parent,
                        doc.root_font_size(),
                        phase,
                        None,
                        None,
                    );
                }
                continue;
            }

            if let Property::Custom(custom) = property
                && !custom.name.as_ref().starts_with("--")
                && token_list_contains_var(&custom.value)
            {
                let mut resolved = custom.clone();
                mark_var_substitution_boundaries(&mut resolved.value);
                resolved.value.substitute_variables(&var_map);
                if token_list_contains_var(&resolved.value) {
                    let _ = apply_css_wide_keyword_in_phase(
                        style,
                        custom.name.as_ref(),
                        "unset",
                        parent,
                        doc.root_font_size(),
                        phase,
                        None,
                        None,
                    );
                } else {
                    apply_property_in_phase(
                        doc,
                        styles,
                        style,
                        &Property::Custom(resolved),
                        parent_font_size,
                        self.prepared.environment(),
                        phase,
                        parent,
                        revert_basis,
                        revert_layer_basis,
                    );
                }
                continue;
            }

            apply_property_in_phase(
                doc,
                styles,
                style,
                property,
                parent_font_size,
                self.prepared.environment(),
                phase,
                parent,
                revert_basis,
                revert_layer_basis,
            );
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
            rollback_layers,
            rollback_origins,
            parent_font_size,
            custom_properties,
            parent,
            inline_style,
            presentational_hints_node,
        } = inputs;
        let prepared = self.prepared;
        let unused_rollback_basis = WorkingStyle::default();
        for phase in [CascadePhase::Prerequisites, CascadePhase::Remaining] {
            let mut hints_applied = false;
            let mut normal_origin_baselines: Vec<(CascadeOrigin, WorkingStyle)> = Vec::new();
            let mut normal_layer_baselines: Vec<(RulePriority, WorkingStyle)> = Vec::new();

            for matched in normal_rules {
                let rule = prepared.get(matched.id);
                let priority = rule.priority();
                if origin_needs_rollback(rollback_origins, priority.origin())
                    && origin_baseline(&normal_origin_baselines, priority.origin()).is_none()
                {
                    normal_origin_baselines.push((priority.origin(), style.clone()));
                }
                if let Some(node_idx) = presentational_hints_node
                    && !hints_applied
                    && priority.origin() == CascadeOrigin::Author
                {
                    html_presentational_hints::apply(self.doc, node_idx, style, phase);
                    hints_applied = true;
                }
                if layer_needs_rollback(rollback_layers, priority)
                    && layer_baseline(&normal_layer_baselines, priority).is_none()
                {
                    normal_layer_baselines.push((priority, style.clone()));
                }
                let origin_basis = origin_baseline(&normal_origin_baselines, priority.origin())
                    .unwrap_or(&unused_rollback_basis);
                let layer_basis = layer_baseline(&normal_layer_baselines, priority)
                    .unwrap_or(&unused_rollback_basis);
                self.apply_declarations(
                    style,
                    &rule.style_rule().declarations.declarations,
                    parent_font_size,
                    Some(custom_properties),
                    phase,
                    parent,
                    origin_basis,
                    layer_basis,
                );
            }

            if let Some(node_idx) = presentational_hints_node
                && !hints_applied
            {
                if rollback_origins.contains(&CascadeOrigin::Author)
                    && origin_baseline(&normal_origin_baselines, CascadeOrigin::Author).is_none()
                {
                    normal_origin_baselines.push((CascadeOrigin::Author, style.clone()));
                }
                html_presentational_hints::apply(self.doc, node_idx, style, phase);
            }

            if let Some(inline_style) = inline_style {
                let origin_basis = origin_baseline(&normal_origin_baselines, CascadeOrigin::Author)
                    .unwrap_or(&unused_rollback_basis);
                let layer_basis =
                    declarations_use_revert_layer(&inline_style.declarations.declarations)
                        .then(|| style.clone());
                self.apply_declarations(
                    style,
                    &inline_style.declarations.declarations,
                    parent_font_size,
                    Some(custom_properties),
                    phase,
                    parent,
                    origin_basis,
                    layer_basis.as_ref().unwrap_or(&unused_rollback_basis),
                );
            }

            for matched in important_rules.iter().filter(|matched| {
                prepared.get(matched.id).priority().origin() == CascadeOrigin::Author
            }) {
                let rule = prepared.get(matched.id);
                let origin_basis =
                    origin_baseline(&normal_origin_baselines, rule.priority().origin())
                        .unwrap_or(&unused_rollback_basis);
                let layer_basis = layer_baseline(&normal_layer_baselines, rule.priority())
                    .unwrap_or(&unused_rollback_basis);
                self.apply_declarations(
                    style,
                    &rule.style_rule().declarations.important_declarations,
                    parent_font_size,
                    Some(custom_properties),
                    phase,
                    parent,
                    origin_basis,
                    layer_basis,
                );
            }

            if let Some(inline_style) = inline_style {
                let origin_basis = origin_baseline(&normal_origin_baselines, CascadeOrigin::Author)
                    .unwrap_or(&unused_rollback_basis);
                let layer_basis = declarations_use_revert_layer(
                    &inline_style.declarations.important_declarations,
                )
                .then(|| style.clone());
                self.apply_declarations(
                    style,
                    &inline_style.declarations.important_declarations,
                    parent_font_size,
                    Some(custom_properties),
                    phase,
                    parent,
                    origin_basis,
                    layer_basis.as_ref().unwrap_or(&unused_rollback_basis),
                );
            }

            for matched in important_rules.iter().filter(|matched| {
                prepared.get(matched.id).priority().origin() == CascadeOrigin::UserAgent
            }) {
                let rule = prepared.get(matched.id);
                let origin_basis =
                    origin_baseline(&normal_origin_baselines, rule.priority().origin())
                        .unwrap_or(&unused_rollback_basis);
                let layer_basis = layer_baseline(&normal_layer_baselines, rule.priority())
                    .unwrap_or(&unused_rollback_basis);
                self.apply_declarations(
                    style,
                    &rule.style_rule().declarations.important_declarations,
                    parent_font_size,
                    Some(custom_properties),
                    phase,
                    parent,
                    origin_basis,
                    layer_basis,
                );
            }
        }
    }
}
