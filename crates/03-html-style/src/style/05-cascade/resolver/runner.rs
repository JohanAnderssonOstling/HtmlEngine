//! Winner selection and dependency-ordered computed-value conversion.

use super::context::{CascadePhase, ResolutionContext, ResolvedDeclaration};
use super::custom_properties::{
    CustomProperties, parse_property_tokens, resolve_single_var_property,
    substitute_property_tokens,
};
use super::properties;
use super::specified::{
    CascadeComputation, DeclarationEvent, ElementDeclarations, select_specified_values,
};
use super::state::{ParentStyle, WorkingStyle};
use super::traversal::root_font_size_for_resolution;
use super::values::rebind_line_height;
use super::wide_keywords::apply_css_wide_keyword_in_phase;
use crate::style::cascade::resolver::prepared_native::{PreparedNativeValue, prepare_declaration};
use crate::style::compiled::DeclarationId;
use crate::style::property_catalogue::PropertyKey;
use crate::style::syntax::capabilities::declaration_is_renderer_eligible;
use crate::style::syntax::prepared_renderer::PreparedRendererValue;
use crate::style::syntax::property_id::RendererPropertyId;
use crate::style::syntax::substitution::needs_substitution;
use crate::style::syntax::targets::expand_declaration;
use crate::style::syntax::wide_keywords::{CssWideKeyword, property_css_wide_keyword};
use html_dom::Document;
use html_style_model::{ComputedStylesBuilder, ReaderStyleOverrides};
use lightningcss::properties::Property;
use lightningcss::properties::custom::{TokenList, TokenOrValue};
use rustc_data_structures::fx::FxHashMap;

// Stop admitting new combinations at the limit; existing entries remain reusable.
// This bounds retention when every element has a different variable environment.
const MAX_SUBSTITUTION_CACHE_ENTRIES: usize = 256;

/// Owned by one style-resolution session, alongside its custom-property store.
/// A fragment/declaration ID identifies its property and complete expression;
/// the immutable map ID identifies the effective variable environment.
/// Inline/hint scratch and temporary pseudo-element maps are not admitted.
#[derive(Default)]
pub(super) struct SubstitutionCache<'css> {
    indices: FxHashMap<(DeclarationId, u32), usize>,
    entries: Vec<ResolvedDeclaration<'css>>,
}

impl<'css> SubstitutionCache<'css> {
    fn resolve(
        &mut self,
        id: DeclarationId,
        property: &Property<'css>,
        custom_properties: &CustomProperties<'_, 'css>,
        substitution_tokens: &mut Vec<TokenOrValue<'css>>,
    ) -> Option<&ResolvedDeclaration<'css>> {
        let tokens = match property {
            Property::Unparsed(value) => &value.value,
            Property::Custom(value) if !value.name.as_ref().starts_with("--") => &value.value,
            _ => return None,
        };
        if !needs_substitution(tokens) {
            return None;
        }
        let key = (id, custom_properties.map_id());
        if let Some(&index) = self.indices.get(&key) {
            return Some(&self.entries[index]);
        }
        if self.entries.len() >= MAX_SUBSTITUTION_CACHE_ENTRIES {
            return None;
        }
        let mut entry = ResolvedDeclaration::default();
        // Keep failures too: substituted-invalid declarations must still use
        // the existing unset behavior. Computed conversion stays per element.
        let valid = resolve_declaration(
            property,
            custom_properties,
            &mut entry,
            substitution_tokens,
            false,
        )
        .is_ok();
        entry.set_valid(valid);
        let index = self.entries.len();
        self.entries.push(entry);
        self.indices.insert(key, index);
        Some(&self.entries[index])
    }
}

pub(super) struct CascadeInputs<'a, 'inline, 'css> {
    pub(super) events: &'a [DeclarationEvent],
    pub(super) element: ElementDeclarations<'inline, 'css>,
    pub(super) custom_properties: &'a CustomProperties<'a, 'css>,
    pub(super) cache_substitutions: bool,
    pub(super) parent: &'a ParentStyle,
}

/// Borrows only computation inputs, leaving event and matching scratch independent.
pub(super) struct CascadeResolver<'a> {
    pub(super) doc: &'a Document,
    pub(super) styles: &'a mut ComputedStylesBuilder,
    pub(super) reader_overrides: &'a ReaderStyleOverrides,
    pub(super) resolution: &'a mut ResolutionContext,
}

impl<'a> CascadeResolver<'a> {
    fn enforce_reader_minimum(&mut self, style: &mut WorkingStyle) {
        let Some(minimum) = self
            .reader_overrides
            .minimum_font_size
            .filter(|value| value.is_finite() && *value > 0.0)
        else {
            return;
        };
        if style.font(self.styles).font_size >= minimum {
            return;
        }
        style.font_mut(self.styles).font_size = minimum;
        style.font_mut(self.styles).font_size_x_height_px = 0.0;
        style.font_mut(self.styles).font_size_ch_advance_px = 0.0;
        style.font_mut(self.styles).font_size_cap_height_px = 0.0;
        style.font_mut(self.styles).font_size_root_ch = 0.0;
        style.font_mut(self.styles).font_size_root_cap_height = 0.0;
        style.font_mut(self.styles).font_size_root_line_height = 0.0;
        rebind_line_height(
            style,
            self.styles,
            || root_font_size_for_resolution(self.doc, self.styles),
            self.resolution,
        );
        if style.line_height_spec.is_none() && style.text(self.styles).line_height_number != 0.0 {
            // Keep the reader override's finite cap for inherited unitless values.
            style.text_mut(self.styles).line_height = (f64::from(minimum)
                * f64::from(style.text(self.styles).line_height_number))
            .min(f64::from(f32::MAX)) as f32;
        }
    }

    fn apply_selected_declaration<'css>(
        &mut self,
        style: &mut WorkingStyle,
        property: &Property<'css>,
        resolved: &ResolvedDeclaration<'css>,
        prepared_longhands: Option<&[Property<'css>]>,
        prepared_renderer: Option<&PreparedRendererValue>,
        prepared_native: &[Option<PreparedNativeValue>],
        targets: &[PropertyKey],
        phase: CascadePhase,
        parent: &ParentStyle,
    ) {
        let mut result = if resolved.valid() { Ok(()) } else { Err(()) };
        if resolved.requires_viewport_height
            && self.resolution.environment.viewport_height().is_none()
        {
            self.resolution.uses_viewport_units = true;
            result = Err(());
        }
        if result.is_ok() && resolved.keyword().is_none() {
            properties::compute_declaration(
                self.doc,
                self.styles,
                style,
                property,
                resolved,
                prepared_longhands,
                parent,
                self.resolution,
                resolved.renderer_value.as_deref().or(prepared_renderer),
                targets,
                phase,
                if resolved.owns_value() {
                    &resolved.native_values
                } else {
                    prepared_native
                },
            );
        }
        // Invalid substitution or unavailable environment inputs mean unset.
        // Prepared-value application itself has no failure result.
        if let Some(keyword) = resolved
            .keyword()
            .or_else(|| result.is_err().then_some(CssWideKeyword::Unset))
        {
            apply_css_wide_keyword_in_phase(
                style,
                targets,
                keyword,
                parent,
                self.styles,
                self.doc.root_font_size(),
                phase,
            );
        }
    }

    pub(super) fn apply_cascade<'inline, 'css>(
        &mut self,
        style: &mut WorkingStyle,
        inputs: CascadeInputs<'_, 'inline, 'css>,
        scratch: &mut CascadeComputation<'css>,
    ) {
        let CascadeInputs {
            events,
            element,
            custom_properties,
            cache_substitutions,
            parent,
        } = inputs;
        let CascadeComputation {
            valid_events,
            selection,
            resolved,
            slots,
            substitution_tokens,
            substitution_cache,
        } = scratch;
        valid_events.clear();
        valid_events.reserve(events.len());
        for event in events.iter().copied() {
            if event.prepared_renderer_eligibility(element) {
                if event.requires_viewport_height(element)
                    && self.resolution.environment.viewport_height().is_none()
                {
                    self.resolution.uses_viewport_units = true;
                    continue;
                }
                valid_events.push(event);
            }
        }

        slots.clear();
        slots.resize(valid_events.len(), usize::MAX);
        let mut resolved_count = 0;
        select_specified_values(valid_events, element, selection);
        for phase in [CascadePhase::Prerequisites, CascadePhase::Remaining] {
            for declaration in &selection.declarations {
                let targets = selection.targets_for(declaration);
                if !targets
                    .iter()
                    .any(|&target| target == PropertyKey::All || target.phase() == phase)
                {
                    if phase == CascadePhase::Prerequisites {
                        self.enforce_reader_minimum(style);
                    }
                    continue;
                }
                let event = valid_events[declaration.sequence];
                let property = event.property(element);
                let prepared_longhands = event.prepared_longhands(element);
                let cached = match (cache_substitutions, event.rule_property(element)) {
                    (true, Some((id, property))) => substitution_cache.resolve(
                        id,
                        property,
                        custom_properties,
                        substitution_tokens,
                    ),
                    _ => None,
                };
                let resolved_entry = if let Some(entry) = cached {
                    entry
                } else {
                    let slot = &mut slots[declaration.sequence];
                    if *slot == usize::MAX {
                        *slot = resolved_count;
                        resolved_count += 1;
                        if resolved.len() <= *slot {
                            resolved.push(ResolvedDeclaration::default());
                        }
                        let entry = &mut resolved[*slot];
                        let valid = resolve_declaration(
                            property,
                            custom_properties,
                            entry,
                            substitution_tokens,
                            prepared_longhands.is_some(),
                        )
                        .is_ok();
                        entry.set_valid(valid);
                    }
                    &resolved[*slot]
                };
                self.apply_selected_declaration(
                    style,
                    property,
                    resolved_entry,
                    prepared_longhands,
                    event.renderer_value(element),
                    event.native_values(element),
                    targets,
                    phase,
                    parent,
                );
                if phase == CascadePhase::Prerequisites {
                    self.enforce_reader_minimum(style);
                }
            }
            if phase == CascadePhase::Prerequisites {
                self.enforce_reader_minimum(style);
                properties::finish_prerequisites(
                    self.doc,
                    self.styles,
                    style,
                    parent,
                    self.resolution,
                );
            }
        }
    }
}

fn resolve_declaration<'vars, 'css>(
    property: &Property<'css>,
    custom_properties: &'vars CustomProperties<'vars, 'css>,
    resolved: &mut ResolvedDeclaration<'css>,
    substitution_tokens: &mut Vec<TokenOrValue<'css>>,
    expansion_prepared: bool,
) -> Result<(), ()> {
    resolved.reset();
    let substitute =
        |tokens: &mut TokenList<'css>| substitute_property_tokens(tokens, custom_properties);
    let owned = match property {
        Property::Unparsed(value) if needs_substitution(&value.value) => {
            resolved.set_substituted(true);
            Some(
                resolve_single_var_property(value, custom_properties)
                    .or_else(|| {
                        let mut tokens = TokenList(std::mem::take(substitution_tokens));
                        tokens.0.clone_from(&value.value.0);
                        let result = substitute(&mut tokens)
                            .then(|| parse_property_tokens(&value.property_id, &tokens))
                            .flatten();
                        *substitution_tokens = tokens.0;
                        result
                    })
                    .ok_or(())?,
            )
        }
        Property::Custom(value)
            if !value.name.as_ref().starts_with("--") && needs_substitution(&value.value) =>
        {
            resolved.set_substituted(true);
            let mut value = value.clone();
            if !substitute(&mut value.value) {
                return Err(());
            }
            Some(Property::Custom(value))
        }
        _ => None,
    };
    if let Some(value) = owned {
        resolved.set_value(value);
    }
    resolved.set_keyword(property_css_wide_keyword(resolved.property(property)));
    let value = resolved.property(property);
    if resolved.substituted()
        && resolved.keyword().is_none()
        && (matches!(value, Property::Unparsed(_)) || !declaration_is_renderer_eligible(value))
    {
        return Err(());
    }
    if resolved.keyword().is_some() {
        return Ok(());
    }
    if let Some((id, tokens)) = RendererPropertyId::raw(value)
        && let Some(canonical) = id.native_property()
    {
        let parsed = parse_property_tokens(&canonical, tokens).ok_or(())?;
        resolved.set_value(parsed);
    }
    let value = if resolved.owns_value() {
        resolved.value.as_deref().expect("resolved owned value")
    } else {
        property
    };
    if resolved.owns_value()
        && let Some((id, tokens)) = RendererPropertyId::raw(value)
    {
        resolved.renderer_value = Some(Box::new(
            PreparedRendererValue::parse(id, tokens).ok_or(())?,
        ));
    }
    if !expansion_prepared {
        let expanded = expand_declaration(value, &mut resolved.longhands)?;
        resolved.set_expanded(expanded);
    }
    if resolved.owns_value() {
        let native = prepare_declaration(
            resolved
                .expanded()
                .then_some(resolved.longhands.as_slice())
                .unwrap_or(std::slice::from_ref(resolved.property(property))),
        )?;
        resolved.native_values = native;
    }
    if resolved.owns_value() {
        resolved.requires_viewport_height = resolved
            .native_values
            .iter()
            .flatten()
            .any(PreparedNativeValue::requires_viewport_height)
            || resolved
                .renderer_value
                .as_deref()
                .is_some_and(PreparedRendererValue::requires_viewport_height);
    }
    Ok(())
}
