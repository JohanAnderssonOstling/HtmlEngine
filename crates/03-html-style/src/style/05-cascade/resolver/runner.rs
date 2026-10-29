//! Winner selection and dependency-ordered computed-value conversion.

use super::context::{CascadePhase, ResolvedDeclaration};
use super::custom_properties::{
    parse_property_tokens, resolve_single_var_property, substitute_property_tokens,
};
use super::properties;
use super::session::StyleResolverContext;
use super::specified::{
    CascadeComputation, DeclarationEvent, ElementDeclarations, select_specified_values,
};
use super::state::WorkingStyle;
use super::traversal::root_font_size_for_resolution;
use super::values::rebind_line_height;
use super::wide_keywords::{apply_css_wide_keyword_in_phase, copy_computed};
use crate::style::property_catalogue::PropertyKey;
use crate::style::syntax::capabilities::declaration_is_renderer_eligible;
use crate::style::syntax::property_id::RendererPropertyId;
use crate::style::syntax::substitution::needs_substitution;
use crate::style::syntax::wide_keywords::property_css_wide_keyword;
use lightningcss::properties::Property;
use lightningcss::properties::custom::TokenList;
use rustc_data_structures::fx::FxHashMap;
use std::collections::HashMap;

pub(super) struct CascadeInputs<'a, 'sheet, 'inline, 'css> {
    pub(super) events: &'a [DeclarationEvent<'sheet, 'css>],
    pub(super) element: ElementDeclarations<'inline, 'css>,
    pub(super) custom_properties: &'a FxHashMap<String, TokenList<'css>>,
    pub(super) parent: &'a WorkingStyle,
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
        rebind_line_height(
            style,
            || root_font_size_for_resolution(self.doc, self.styles),
            self.resolution,
        );
        if style.line_height_spec.is_none() && style.text.line_height_number != 0.0 {
            // Keep the reader override's finite cap for inherited unitless values.
            style.text.line_height = (f64::from(minimum) * f64::from(style.text.line_height_number))
                .min(f64::from(f32::MAX)) as f32;
        }
    }

    fn apply_selected_declaration(
        &mut self,
        style: &mut WorkingStyle,
        property: &Property<'css>,
        resolved: &ResolvedDeclaration<'css>,
        targets: &[PropertyKey],
        phase: CascadePhase,
        parent: &WorkingStyle,
    ) -> Result<(), ()> {
        let mut result = if resolved.valid { Ok(()) } else { Err(()) };
        if resolved.valid && resolved.keyword.is_none() {
            let computed = &mut *self.declaration_style;
            computed.clone_from(style);
            result = properties::compute_declaration(
                self.doc,
                self.styles,
                computed,
                property,
                resolved,
                parent,
                self.resolution,
            );
            if result.is_ok() {
                for &target in targets {
                    if target.phase() == phase {
                        copy_computed(style, computed, target, true);
                    }
                }
            }
        }
        // Failure after substitution has CSS unset semantics.
        if let Some(keyword) = resolved
            .keyword
            .or_else(|| (result.is_err() && resolved.substituted).then_some("unset"))
        {
            apply_css_wide_keyword_in_phase(
                style,
                targets,
                keyword,
                parent,
                self.doc.root_font_size(),
                phase,
            );
            return Ok(());
        }
        result
    }

    pub(super) fn apply_cascade<'event, 'inline>(
        &mut self,
        style: &mut WorkingStyle,
        inputs: CascadeInputs<'_, 'event, 'inline, 'css>,
        scratch: &mut CascadeComputation<'event, 'css>,
    ) {
        let CascadeInputs {
            events,
            element,
            custom_properties,
            parent,
        } = inputs;
        let CascadeComputation {
            valid_events,
            selection,
            resolved,
            slots,
        } = scratch;
        valid_events.clear();
        valid_events.reserve(events.len());
        for event in events.iter().copied() {
            if event.prepared_renderer_eligibility().unwrap_or_else(|| {
                crate::style::syntax::capabilities::declaration_is_renderer_eligible(
                    event.property(element),
                )
            }) {
                valid_events.push(event);
            }
        }

        slots.clear();
        slots.resize(valid_events.len(), usize::MAX);
        let mut resolved_count = 0;
        let mut var_map = None;
        let mut candidate = std::mem::take(self.declaration_style);
        // Convert a selected cascade into a scratch style. A failed winner
        // exposes the next declaration; a successful pass is the final style.
        loop {
            select_specified_values(valid_events, element, selection);
            candidate.clone_from(style);
            let mut invalid_sequence = None;
            'phases: for phase in [CascadePhase::Prerequisites, CascadePhase::Remaining] {
                for declaration in &selection.declarations {
                    let targets = selection.targets_for(declaration);
                    if !targets
                        .iter()
                        .any(|&target| target == PropertyKey::All || target.phase() == phase)
                    {
                        if phase == CascadePhase::Prerequisites {
                            self.enforce_reader_minimum(&mut candidate);
                        }
                        continue;
                    }
                    let property = valid_events[declaration.sequence].property(element);
                    let slot = &mut slots[declaration.sequence];
                    if *slot == usize::MAX {
                        *slot = resolved_count;
                        resolved_count += 1;
                        if resolved.len() <= *slot {
                            resolved.push(ResolvedDeclaration::default());
                        }
                        let entry = &mut resolved[*slot];
                        entry.valid =
                            resolve_declaration(property, custom_properties, &mut var_map, entry)
                                .is_ok();
                    }
                    let result = self.apply_selected_declaration(
                        &mut candidate,
                        property,
                        &resolved[*slot],
                        targets,
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
                if phase == CascadePhase::Prerequisites {
                    self.enforce_reader_minimum(&mut candidate);
                    properties::finish_prerequisites(
                        self.doc,
                        self.styles,
                        &mut candidate,
                        parent,
                        self.resolution,
                    );
                }
            }
            if let Some(sequence) = invalid_sequence {
                valid_events.remove(sequence);
                slots.remove(sequence);
            } else {
                // Retain the original accumulator as scratch for the next
                // element; clone_from can reuse its font-feature buffers.
                std::mem::swap(style, &mut candidate);
                break;
            }
        }
        *self.declaration_style = candidate;
    }
}

fn resolve_declaration<'vars, 'css>(
    property: &Property<'css>,
    custom_properties: &'vars FxHashMap<String, TokenList<'css>>,
    var_map: &mut Option<HashMap<&'vars str, TokenList<'css>>>,
    resolved: &mut ResolvedDeclaration<'css>,
) -> Result<(), ()> {
    resolved.value = None;
    resolved.longhands.clear();
    resolved.expanded = false;
    resolved.keyword = None;
    resolved.substituted = false;
    let mut substitute = |tokens: &mut TokenList<'css>| {
        substitute_property_tokens(
            tokens,
            var_map.get_or_insert_with(|| {
                custom_properties
                    .iter()
                    .map(|(name, value)| (name.as_str(), value.clone()))
                    .collect()
            }),
        )
    };
    resolved.value = (match property {
        Property::Unparsed(value) if needs_substitution(&value.value) => {
            resolved.substituted = true;
            Some(
                resolve_single_var_property(value, custom_properties)
                    .or_else(|| {
                        let mut value = value.clone();
                        substitute(&mut value.value)
                            .then(|| parse_property_tokens(&value.property_id, &value.value))
                            .flatten()
                    })
                    .ok_or(())?,
            )
        }
        Property::Custom(value)
            if !value.name.as_ref().starts_with("--") && needs_substitution(&value.value) =>
        {
            resolved.substituted = true;
            let mut value = value.clone();
            if !substitute(&mut value.value) {
                return Err(());
            }
            Some(Property::Custom(value))
        }
        _ => None,
    })
    .map(Box::new);
    let value = resolved.value.as_deref().unwrap_or(property);
    resolved.keyword = property_css_wide_keyword(value);
    if resolved.substituted
        && resolved.keyword.is_none()
        && (matches!(value, Property::Unparsed(_)) || !declaration_is_renderer_eligible(value))
    {
        return Err(());
    }
    if resolved.keyword.is_some() {
        return Ok(());
    }
    if let Some((id, tokens)) = RendererPropertyId::raw(value)
        && let Some(canonical) = id.gap_property()
    {
        resolved.value = Some(Box::new(
            parse_property_tokens(&canonical, tokens).ok_or(())?,
        ));
    }
    resolved.expanded = properties::expand_declaration(
        resolved.value.as_deref().unwrap_or(property),
        &mut resolved.longhands,
    )?;
    Ok(())
}
