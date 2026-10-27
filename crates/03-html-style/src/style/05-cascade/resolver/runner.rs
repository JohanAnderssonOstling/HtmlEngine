//! Winner selection and dependency-ordered computed-value conversion.

use super::*;
use crate::style::syntax::{flex, font};
use crate::style::syntax::value::parsed_property;
use static_self::IntoOwned;
use std::borrow::Cow;
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
    pub(super) parent_custom: &'a FxHashMap<String, TokenList<'css>>,
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
            html_presentational_hints::apply(self.doc, node, style, phase, self.resolution.font_size_scale);
            return;
        }
        let before = style.clone();
        html_presentational_hints::apply(self.doc, node, style, phase, self.resolution.font_size_scale);
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
        if let Some(keyword) = property_css_wide_keyword(property) {
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
            if let Some(value) = property.longhand(property_id)
                .or_else(|| flex::longhand(property, property_id))
                .or_else(|| font::longhand(property, property_id))
            {
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
        parent_custom: &FxHashMap<String, TokenList<'css>>,
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
        let substituted = matches!(property, Property::Unparsed(value) if needs_substitution(&value.value) || contains_random_math(&value.value))
            || matches!(property, Property::Custom(value) if needs_substitution(&value.value) || contains_random_math(&value.value));
        let resolved = match property {
            Property::Unparsed(unparsed) if needs_substitution(&unparsed.value) || contains_random_math(&unparsed.value) => {
                resolved_unparsed = resolve_single_var_property(unparsed, custom_properties)
                    .or_else(|| {
                        let mut substitutable = unparsed.clone();
                        if !substitute_property_tokens(&mut substitutable.value, var_map, parent_custom, &self.random, property.property_id().name()) {
                            return None;
                        }
                        parse_substituted_property(&substitutable.property_id, &substitutable.value)
                    });
                let Some(property) = resolved_unparsed.as_ref() else {
                    self.unset_targets(style, targets, phase, parent);
                    return Ok(());
                };
                property
            }
            Property::Custom(custom)
                if !custom.name.as_ref().starts_with("--")
                    && (needs_substitution(&custom.value) || contains_random_math(&custom.value)) =>
            {
                let mut value = custom.clone();
                if !substitute_property_tokens(&mut value.value, var_map, parent_custom, &self.random, property.property_id().name()) {
                    self.unset_targets(style, targets, phase, parent);
                    return Ok(());
                }
                resolved_custom = Property::Custom(value);
                &resolved_custom
            }
            property => property,
        };

        let numeric_resolved;
        let resolved = match self.resolve_numeric_property(resolved) {
            Ok(Some(property)) => {
                numeric_resolved = property;
                &numeric_resolved
            }
            Ok(None) => resolved,
            Err(()) if substituted => {
                self.unset_targets(style, targets, phase, parent);
                return Ok(());
            }
            Err(()) => return Err(()),
        };

        // Resolve a contextual flex shorthand once before validating or
        // extracting its selected longhands. Replaying it for one target
        // would also overwrite independent longhand winners.
        let computed_flex = if matches!(resolved, Property::Unparsed(value)
            if matches!(value.property_id, PropertyId::Flex(_)))
        {
            match properties::resolve_sign_property(
                resolved, style, parent_font_size,
                root_font_size_for_resolution(self.doc, self.styles), self.resolution,
            ) {
                Ok(property) => property,
                Err(()) if substituted => {
                    self.unset_targets(style, targets, phase, parent);
                    return Ok(());
                }
                Err(()) => return Err(()),
            }
        } else {
            None
        };
        let resolved = computed_flex.as_ref().unwrap_or(resolved);

        // Dependency recovery values need an explicit renderer conversion
        // after substitution. Otherwise the winning declaration becomes unset
        // rather than exposing a losing declaration.
        if substituted
            && property_css_wide_keyword(resolved).is_none()
            && (matches!(resolved, Property::Unparsed(value)
                if !properties::supports_unparsed_property(&value.property_id)
                    || needs_substitution(&value.value))
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
            .is_some_and(|longhands| {
                let reset_only = if matches!(resolved.property_id(), PropertyId::Font) {
                    font::RESET_LONGHANDS.len()
                } else {
                    0
                };
                targets.len() < longhands.len() + reset_only
            })
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

    fn resolve_numeric_property<'property>(
        &mut self,
        property: &Property<'_>,
    ) -> Result<Option<Property<'property>>, ()> {
        if !matches!(property, Property::Unparsed(_) | Property::Custom(_))
            || property.property_id().name().starts_with("--")
        {
            return Ok(None);
        }
        let css = property.value_to_css_string(PrinterOptions::default()).map_err(|_| ())?;
        let tree = self.tree_counting.lower(self.doc, &css)?;
        let lowered = crate::style::syntax::random_math::for_computation(&tree, |key|self.random.base(key,property.property_id().name(),1))?;
        if matches!(tree, Cow::Borrowed(_)) && matches!(lowered, Cow::Borrowed(_)) {
            return Ok(None);
        }
        parsed_property(property.property_id(), &lowered)
            .map(IntoOwned::into_owned).map(Some).ok_or(())
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
            parent_custom,
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
                            needs_substitution(&value.value)
                                && !single_var_property_is_resolvable(value, custom_properties)
                        }
                        Property::Custom(value) => needs_substitution(&value.value),
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
                        parent_custom,
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
                // Retain the original accumulator as scratch for the next
                // element; clone_from can reuse its font-feature buffers.
                std::mem::swap(style, &mut candidate);
                *self.validation_style = candidate;
                break;
            }
        }
    }
}
