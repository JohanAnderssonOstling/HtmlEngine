//! Winner selection and dependency-ordered computed-value conversion.

use super::*;
use specified::{DeclarationEvent, PropertyTarget, SpecifiedSelection, select_specified_values};

/// The cascade computes dependency roots before values that consume them.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum CascadePhase {
    Prerequisites,
    Remaining,
}

pub(super) struct CascadeInputs<'a, 'sheet, 'inline, 'css> {
    pub(super) events: &'a [DeclarationEvent<'sheet, 'css>],
    pub(super) inline_style: Option<&'inline StyleAttribute<'css>>,
    pub(super) hints_sequence: usize,
    pub(super) parent_font_size: f32,
    pub(super) custom_properties: &'a FxHashMap<String, TokenList<'css>>,
    pub(super) parent: &'a ParentStyle,
    pub(super) presentational_hints_node: Option<DomNodeId>,
}

fn target_phase(target: &PropertyTarget) -> CascadePhase {
    if matches!(
        &*target.name,
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

impl<'a, 'css> StyleResolverContext<'a, 'css> {
    fn enforce_reader_minimum(&self, style: &mut WorkingStyle) {
        let Some(minimum) = self
            .reader_overrides
            .minimum_font_size
            .filter(|value| value.is_finite() && *value > 0.0)
        else {
            return;
        };
        if style.font.font_size >= minimum {
            return;
        }
        style.font.font_size = minimum;
        style.font.font_size_x_height_px = 0.0;
        style.font.font_size_ch_advance_px = 0.0;
        style.font.font_size_cap_height_px = 0.0;
        style.font.font_size_root_ch = 0.0;
        style.font.font_size_root_cap_height = 0.0;
        style.font.font_size_root_line_height = 0.0;
        if let Some(spec) = style.line_height_spec.as_ref() {
            if let Some((line_height, x_height_px)) = checked_line_height_components(
                spec,
                minimum,
                root_font_size_for_resolution(self.doc, self.styles),
                self.resolution,
            ) {
                style.text.line_height = line_height;
                style.text.line_height_x_height_px = x_height_px;
            }
        } else if style.text.line_height_number != 0.0 {
            style.text.line_height = (f64::from(minimum) * f64::from(style.text.line_height_number))
                .min(f64::from(f32::MAX)) as f32;
            style.text.line_height_x_height_px = 0.0;
        }
    }

    fn apply_presentational_hints(
        &mut self,
        style: &mut WorkingStyle,
        node: DomNodeId,
        phase: CascadePhase,
        selection: &SpecifiedSelection,
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

    fn apply_computed_property(
        &mut self,
        style: &mut WorkingStyle,
        property: &Property<'_>,
        target: &PropertyTarget,
        parent_font_size: f32,
        phase: CascadePhase,
        parent: &ParentStyle,
    ) -> Result<(), ()> {
        if &*target.name != "all" && target_phase(target) != phase {
            return Ok(());
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
            return Ok(());
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
            phase,
            parent,
            self.resolution,
        )
    }

    fn apply_selected_declaration(
        &mut self,
        style: &mut WorkingStyle,
        property: &Property<'css>,
        targets: &[PropertyTarget],
        parent_font_size: f32,
        custom_properties: &FxHashMap<String, TokenList<'css>>,
        var_map: &HashMap<&str, TokenList<'css>>,
        phase: CascadePhase,
        parent: &ParentStyle,
    ) -> Result<(), ()> {
        if !targets
            .iter()
            .any(|target| &*target.name == "all" || target_phase(target) == phase)
        {
            return Ok(());
        }
        let resolved_unparsed;
        let resolved_custom;
        let substituted = matches!(property, Property::Unparsed(value) if token_list_contains_var(&value.value))
            || matches!(property, Property::Custom(value) if token_list_contains_var(&value.value));
        let resolved = match property {
            Property::Unparsed(unparsed) if token_list_contains_var(&unparsed.value) => {
                resolved_unparsed = resolve_single_var_property(unparsed, custom_properties)
                    .or_else(|| {
                        let mut substitutable = unparsed.clone();
                        mark_var_substitution_boundaries(&mut substitutable.value);
                        substitutable.substitute_variables(var_map).ok()
                    });
                let Some(property) = resolved_unparsed.as_ref() else {
                    self.unset_targets(style, targets, phase, parent);
                    return Ok(());
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
                    self.unset_targets(style, targets, phase, parent);
                    return Ok(());
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
                || !crate::style::syntax::capabilities::declaration_is_renderer_eligible(resolved))
        {
            self.unset_targets(style, targets, phase, parent);
            return Ok(());
        }
        // A shorthand can win only some of its longhands. Check the whole
        // shorthand so an invalid hidden component cannot make those winners
        // appear valid. Fully selected shorthands are checked by application.
        if resolved
            .property_id()
            .longhands()
            .is_some_and(|longhands| targets.len() < longhands.len())
            && !properties::partially_selected_shorthand_is_computable(
                self.doc,
                self.styles,
                self.validation_style,
                style,
                resolved,
                parent_font_size,
                parent,
                self.resolution,
            )
        {
            if substituted {
                self.unset_targets(style, targets, phase, parent);
                return Ok(());
            }
            return Err(());
        }
        let before = substituted.then(|| style.clone());
        for target in targets {
            if self
                .apply_computed_property(style, resolved, target, parent_font_size, phase, parent)
                .is_err()
            {
                if let Some(before) = before {
                    *style = before;
                    self.unset_targets(style, targets, phase, parent);
                    return Ok(());
                }
                return Err(());
            }
        }
        Ok(())
    }

    fn unset_targets(
        &self,
        style: &mut WorkingStyle,
        targets: &[PropertyTarget],
        phase: CascadePhase,
        parent: &ParentStyle,
    ) {
        for target in targets {
            if &*target.name == "all" || target_phase(target) == phase {
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
    }

    pub(super) fn apply_cascade<'event, 'inline>(
        &mut self,
        style: &mut WorkingStyle,
        inputs: CascadeInputs<'_, 'event, 'inline, 'css>,
        valid_events: &mut Vec<DeclarationEvent<'event, 'css>>,
        selection: &mut SpecifiedSelection,
    ) {
        let CascadeInputs {
            events,
            inline_style,
            hints_sequence,
            parent_font_size,
            custom_properties,
            parent,
            presentational_hints_node,
        } = inputs;
        valid_events.clear();
        valid_events.reserve(events.len());
        let mut valid_hints_sequence = 0;
        for (sequence, event) in events.iter().copied().enumerate() {
            if event.prepared_renderer_eligibility().unwrap_or_else(|| {
                crate::style::syntax::capabilities::declaration_is_renderer_eligible(
                    event.property(inline_style),
                )
            }) {
                if sequence < hints_sequence {
                    valid_hints_sequence += 1;
                }
                valid_events.push(event);
            }
        }

        // Convert a selected cascade into a scratch style. A failed winner
        // exposes the next declaration; a successful pass is the final style.
        loop {
            select_specified_values(
                valid_events,
                self.prepared,
                inline_style,
                self.property_targets,
                selection,
            );
            let needs_var_map =
                selection.declarations.iter().any(|declaration| {
                    match valid_events[declaration.sequence].property(inline_style) {
                        Property::Unparsed(value) => {
                            token_list_contains_var(&value.value)
                                && !single_var_property_is_resolvable(value, custom_properties)
                        }
                        Property::Custom(value) => token_list_contains_var(&value.value),
                        _ => false,
                    }
                });
            let var_map: HashMap<_, _> = if needs_var_map {
                custom_properties
                    .iter()
                    .map(|(name, value)| (name.as_str(), value.clone()))
                    .collect()
            } else {
                HashMap::new()
            };
            let mut candidate = std::mem::take(self.validation_style);
            candidate.clone_from(style);
            let mut invalid_sequence = None;
            'phases: for phase in [CascadePhase::Prerequisites, CascadePhase::Remaining] {
                let mut hints_applied = false;
                for declaration in &selection.declarations {
                    if !hints_applied && declaration.sequence >= valid_hints_sequence {
                        if let Some(node) = presentational_hints_node {
                            self.apply_presentational_hints(
                                &mut candidate,
                                node,
                                phase,
                                &selection,
                                parent,
                            );
                            if phase == CascadePhase::Prerequisites {
                                self.enforce_reader_minimum(&mut candidate);
                            }
                        }
                        hints_applied = true;
                    }
                    let result = self.apply_selected_declaration(
                        &mut candidate,
                        valid_events[declaration.sequence].property(inline_style),
                        selection.targets_for(declaration),
                        parent_font_size,
                        custom_properties,
                        &var_map,
                        phase,
                        parent,
                    );
                    if result.is_err() {
                        invalid_sequence = Some(declaration.sequence);
                        break 'phases;
                    }
                    if phase == CascadePhase::Prerequisites {
                        self.enforce_reader_minimum(&mut candidate);
                    }
                }
                if !hints_applied {
                    if let Some(node) = presentational_hints_node {
                        self.apply_presentational_hints(
                            &mut candidate,
                            node,
                            phase,
                            &selection,
                            parent,
                        );
                        if phase == CascadePhase::Prerequisites {
                            self.enforce_reader_minimum(&mut candidate);
                        }
                    }
                }
                if phase == CascadePhase::Prerequisites {
                    self.enforce_reader_minimum(&mut candidate);
                }
            }
            if let Some(sequence) = invalid_sequence {
                *self.validation_style = candidate;
                if sequence < valid_hints_sequence {
                    valid_hints_sequence -= 1;
                }
                valid_events.remove(sequence);
            } else {
                *style = candidate;
                break;
            }
        }
    }
}
