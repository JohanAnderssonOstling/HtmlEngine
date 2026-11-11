use super::columns::{ElementName, FilteredRange};
use crate as dom_values;
use super::MemoryUsageReport;
use super::{Document, DocumentMode, ImageId};
use std::collections::HashSet;
use std::fmt;
use std::num::NonZeroU32;
use std::ops::Range;
use unicode_bidi::{Direction, get_base_direction_full};

pub(super) const HTML_NAMESPACE: &str = "http://www.w3.org/1999/xhtml";

/// Opaque document-local node identity stored as a single `u32`.
///
/// IDs must only be used with their originating document. Equality and hashing
/// compare the local index and node kind, not document identity. Lookups dispatch on
/// node kind; array access performs ordinary bounds checks. Foreign document
/// IDs are a caller error.
///
/// Raw integers cannot be converted into node identities:
///
/// ```compile_fail
/// let _: html_dom::DomNodeId = 0u32.into();
/// ```
///
/// Its field is private, so callers obtain IDs through a document:
///
/// ```compile_fail
/// let _ = html_dom::DomNodeId { raw: 0 };
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(transparent)]
pub struct DomNodeId {
    raw: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DocumentBuildError {
    DuplicateChild { parent: u32, child: u32 },
    InvalidNodeId { raw: u32 },
    InvalidParent { parent: u32, child: u32 },
    InvalidStringIndex { raw: u32, index: u16 },
    InvalidClassRange { raw: u32, start: u16, end: u16 },
    InvalidAttributeIndex { raw: u32, index: u16 },
    InvalidImageIndex { raw: u32, index: u32 },
    MissingParentForTextNode { raw: u32 },
    MultipleRoots,
    NodeTypeMismatch { raw: u32, expected_text_parent: bool },
    RootHasParent { raw: u32 },
    MissingRoot,
    NonElementRoot { raw: u32 },
    RecursionCycle { raw: u32 },
    UnreachableNodes { raw: u32 },
    TooManyNodes,
    TooManyStrings,
    TooManyClasses,
    TooManyListEntries,
}

impl fmt::Display for DocumentBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateChild { parent, child } => write!(formatter, "duplicate child {child} in parent {parent}"),
            Self::InvalidNodeId { raw } => write!(formatter, "node id {raw} is out of bounds"),
            Self::InvalidParent { parent, child } => write!(formatter, "parent {parent} is invalid for child {child}"),
            Self::InvalidStringIndex { raw, index } => write!(formatter, "node {raw} contains invalid string index {index}"),
            Self::InvalidClassRange { raw, start, end } => write!(formatter, "node {raw} contains invalid class range {start}..{end}"),
            Self::InvalidAttributeIndex { raw, index } => write!(formatter, "node {raw} contains invalid attribute index {index}"),
            Self::InvalidImageIndex { raw, index } => write!(formatter, "node {raw} references invalid image index {index}"),
            Self::MissingParentForTextNode { raw } => write!(formatter, "text node {raw} needs a parent element"),
            Self::MultipleRoots => formatter.write_str("document has more than one root node"),
            Self::NodeTypeMismatch { raw, expected_text_parent } => {
                if *expected_text_parent {
                    write!(formatter, "node {raw} is not a text node")
                } else {
                    write!(formatter, "node {raw} is not an element node")
                }
            }
            Self::RootHasParent { raw } => write!(formatter, "root node {raw} has a parent"),
            Self::MissingRoot => formatter.write_str("document has nodes but no root"),
            Self::NonElementRoot { raw } => write!(formatter, "document root {raw} is not an element"),
            Self::RecursionCycle { raw } => write!(formatter, "recursion cycle detected near node {raw}"),
            Self::UnreachableNodes { raw } => write!(formatter, "node {raw} is not reachable from root"),
            Self::TooManyNodes => formatter.write_str("document has too many nodes"),
            Self::TooManyStrings => {
                formatter.write_str("document exceeds the 65536 distinct DOM string limit")
            }
            Self::TooManyListEntries => {
                formatter.write_str("document exceeds the u32 child or attribute entry limit")
            }
            Self::TooManyClasses => {
                formatter.write_str("document exceeds the 65535 class occurrence limit")
            }
        }
    }
}

impl std::error::Error for DocumentBuildError {}

/// Mutable DOM construction; `finish` validates the root and topology.
pub struct DocumentBuilder {
    document: Document,
}

impl Default for DocumentBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl DocumentBuilder {
    pub fn new() -> Self {
        Self { document: Document::new() }
    }
}

impl DocumentBuilder {
    pub fn document(&self) -> &Document {
        &self.document
    }

    #[cfg(test)]
    pub(crate) fn document_mut(&mut self) -> &mut Document {
        &mut self.document
    }

    pub fn set_mode(&mut self, mode: DocumentMode) {
        self.document.mode = mode;
    }

    pub fn set_title(&mut self, title: Option<String>) {
        self.document.title = title;
    }

    pub fn set_root_font_size(&mut self, root_font_size: crate::RootFontSize) {
        self.document.root_font_size = root_font_size;
    }

    pub fn intern_string(&mut self, value: &str) -> u16 {
        self.document.strings.intern(value)
    }

    pub fn try_intern_string(&mut self, value: &str) -> Result<u16, DocumentBuildError> {
        self.document.strings.try_intern(value)
    }

    pub fn images(&self) -> &[crate::ImageResource] {
        &self.document.images
    }

    pub fn images_mut(&mut self) -> &mut [crate::ImageResource] {
        &mut self.document.images
    }

    pub fn push_image(&mut self, image: crate::ImageResource) -> u32 {
        self.document.push_image(image)
    }

    pub fn intern_classes(&mut self, classes: Option<&str>) -> Range<u16> {
        self.document.intern_classes(classes)
    }

    pub fn try_intern_classes(&mut self, classes: Option<&str>) -> Result<Range<u16>, DocumentBuildError> {
        self.document.try_intern_classes(classes)
    }
}

impl DocumentBuilder {
    pub fn append_dom_element(&mut self, parent: Option<DomNodeId>, data: DomElementData) -> Result<DomNodeId, DocumentBuildError> {
        if let Some(parent) = parent {
            self.document.validate_element_id(parent)?;
        }
        let raw = DomNodeId::encode_index(self.document.element_count(), false)?;
        let image = data.image_idx.map(|index| {
            index.checked_add(1).and_then(NonZeroU32::new)
                .ok_or(DocumentBuildError::InvalidImageIndex { raw, index })
        }).transpose()?;
        let node_id = DomNodeId::new(raw);
        let elements = &mut self.document.elements;
        elements.names.push(ElementName {
            tag: data.tag,
            namespace: data.namespace,
        });
        elements
            .parents
            .push(parent.and_then(|id| NonZeroU32::new(id.raw() + 1)));
        elements.children.push(Vec::new());
        elements.attributes.push(data.attributes);
        elements.ids.push(data.attribute_cache.id);
        elements
            .classes
            .push(FilteredRange::new(data.attribute_cache.classes));
        elements.states.push(HtmlElementState::default());
        elements.images.push(image);
        if let Some(parent) = parent {
            self.document.append_child(parent, node_id)?;
        }
        Ok(node_id)
    }

    pub fn append_dom_text(&mut self, parent: DomNodeId, text: impl Into<String>) -> Result<DomNodeId, DocumentBuildError> {
        self.document.validate_element_id(parent)?;
        let raw = DomNodeId::encode_index(self.document.text_count(), true)?;
        let node_id = DomNodeId::new(raw);
        self.document.texts.values.push(text.into());
        self.document
            .texts
            .parents
            .push(NonZeroU32::new(parent.raw() + 1));
        self.document.append_child(parent, node_id)?;
        Ok(node_id)
    }

    pub fn finish(mut self) -> Result<Document, DocumentBuildError> {
        self.document.dom_root = self.document.validate_for_build()?;
        self.document.elements.children.finish()?;
        self.document.elements.attributes.finish()?;
        self.document.rebuild_html_semantics();
        Ok(self.document)
    }

    fn validate_and_set_root(&mut self, node_id: DomNodeId) -> Result<(), DocumentBuildError> {
        self.document.validate_node_id(node_id)?;
        let raw = node_id.raw();
        if self.document.node_ref(node_id).parent().is_some() {
            return Err(DocumentBuildError::RootHasParent { raw });
        }
        if node_id.is_text() {
            return Err(DocumentBuildError::NonElementRoot { raw });
        }
        self.document.dom_root = Some(node_id);
        Ok(())
    }

    pub fn set_dom_id_attribute(&mut self, node_id: DomNodeId, id_idx: u16, id_name_idx: u16) -> Result<(), DocumentBuildError> {
        let index = self.document.validate_element_id(node_id)?;
        self.document.elements.ids.set(index, Some(id_idx));
        self.document
            .elements
            .attributes
            .get_mut_for_build(index)
            .push(DomAttribute {
                name: id_name_idx,
                local_name: id_name_idx,
                namespace: None,
                value: id_idx,
            });
        Ok(())
    }
}

impl DocumentBuilder {
    pub fn set_dom_root(&mut self, node_id: DomNodeId) -> Result<(), DocumentBuildError> {
        if let Some(existing_root) = self.document.dom_root {
            if existing_root != node_id {
                return Err(DocumentBuildError::MultipleRoots);
            }
            return Ok(());
        }
        self.validate_and_set_root(node_id)
    }
}

impl DomNodeId {
    fn new(raw: u32) -> Self {
        Self { raw }
    }

    const TEXT_BIT: u32 = 1 << 31;

    fn encode_index(index: usize, text: bool) -> Result<u32, DocumentBuildError> {
        let index = u32::try_from(index).map_err(|_| DocumentBuildError::TooManyNodes)?;
        if index >= Self::TEXT_BIT {
            return Err(DocumentBuildError::TooManyNodes);
        }
        Ok(index | if text { Self::TEXT_BIT } else { 0 })
    }

    /// Index within the element or text columns, not a combined node index.
    pub fn index(self) -> usize {
        (self.raw & !Self::TEXT_BIT) as usize
    }

    pub fn is_text(self) -> bool {
        self.raw & Self::TEXT_BIT != 0
    }

    /// Tagged document-local identity. Preserve the kind bit when storing keys;
    /// use `index()` only after selecting element or text storage.
    pub fn raw(self) -> u32 {
        self.raw
    }
}

#[derive(Clone, Copy)]
pub struct ElementRef<'a> {
    doc: &'a Document,
    node_id: DomNodeId,
}

/// A read-only semantic view of one DOM attribute.
///
/// The view keeps string-table indices and attribute storage private so test
/// adapters and other consumers do not couple themselves to DOM internals.
#[derive(Clone, Copy)]
pub struct AttributeRef<'a> {
    doc: &'a Document,
    attribute: &'a DomAttribute,
}

#[derive(Clone, Copy)]
pub struct TextRef<'a> {
    doc: &'a Document,
    node_id: DomNodeId,
}

#[derive(Clone, Copy)]
pub enum NodeRef<'a> {
    Element(ElementRef<'a>),
    Text(TextRef<'a>),
}

/// Compact, immutable HTML state derived once when DOM construction finishes.
///
/// Dynamic browser state is deliberately absent: this document model has no
/// JavaScript or native form-control state. Consumers therefore cannot confuse
/// raw attribute presence with normalized static HTML semantics.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(transparent)]
pub struct HtmlElementState(u16);

impl HtmlElementState {
    const LINK: u16 = 1 << 0;
    const ENABLED: u16 = 1 << 1;
    const DISABLED: u16 = 1 << 2;
    const CHECKED: u16 = 1 << 3;
    const DEFAULT: u16 = 1 << 4;
    const OPEN: u16 = 1 << 5;
    const CLOSED: u16 = 1 << 6;
    const REQUIRED: u16 = 1 << 7;
    const OPTIONAL: u16 = 1 << 8;
    const READ_ONLY: u16 = 1 << 9;
    const READ_WRITE: u16 = 1 << 10;
    const PLACEHOLDER_SHOWN: u16 = 1 << 11;
    const INDETERMINATE: u16 = 1 << 12;
    const RTL: u16 = 1 << 13;
    const DEFINED: u16 = 1 << 14;

    fn set(&mut self, flag: u16, value: bool) {
        if value {
            self.0 |= flag;
        } else {
            self.0 &= !flag;
        }
    }

    pub fn is_link(self) -> bool {
        self.0 & Self::LINK != 0
    }
    pub fn is_enabled(self) -> bool {
        self.0 & Self::ENABLED != 0
    }
    pub fn is_disabled(self) -> bool {
        self.0 & Self::DISABLED != 0
    }
    pub fn is_checked(self) -> bool {
        self.0 & Self::CHECKED != 0
    }
    pub fn is_default(self) -> bool {
        self.0 & Self::DEFAULT != 0
    }
    pub fn is_open(self) -> bool {
        self.0 & Self::OPEN != 0
    }
    pub fn is_closed(self) -> bool {
        self.0 & Self::CLOSED != 0
    }
    pub fn is_required(self) -> bool {
        self.0 & Self::REQUIRED != 0
    }
    pub fn is_optional(self) -> bool {
        self.0 & Self::OPTIONAL != 0
    }
    pub fn is_read_only(self) -> bool {
        self.0 & Self::READ_ONLY != 0
    }
    pub fn is_read_write(self) -> bool {
        self.0 & Self::READ_WRITE != 0
    }
    pub fn is_placeholder_shown(self) -> bool {
        self.0 & Self::PLACEHOLDER_SHOWN != 0
    }
    pub fn is_indeterminate(self) -> bool {
        self.0 & Self::INDETERMINATE != 0
    }
    pub fn is_rtl(self) -> bool {
        self.0 & Self::RTL != 0
    }
    pub fn is_defined(self) -> bool {
        self.0 & Self::DEFINED != 0
    }
}

pub(crate) fn name_filter_mask(name: &str, ascii_insensitive: bool) -> u32 {
    let bytes = name.as_bytes();
    let Some(&first) = bytes.first() else { return 1 };
    let fold = |byte: u8| if ascii_insensitive { byte.to_ascii_lowercase() } else { byte };
    let hash = (bytes.len() as u32).wrapping_mul(0x9e37_79b9)
        ^ u32::from(fold(first)).wrapping_mul(0x85eb_ca6b)
        ^ u32::from(fold(bytes[bytes.len() / 2])).wrapping_mul(0xc2b2_ae35)
        ^ u32::from(fold(bytes[bytes.len() - 1]));
    let hash = hash ^ (hash >> 16);
    (1 << (hash & 31)) | (1 << ((hash >> 5) & 31))
}

#[inline]
fn name_filter_may_contain(filter: u32, name: &str, ascii_insensitive: bool) -> bool {
    if filter == 0 {
        return false;
    }
    let mask = name_filter_mask(name, ascii_insensitive);
    filter & mask == mask
}

#[derive(Clone, Debug)]
pub struct DomElementAttributeCache {
    id: Option<u16>,
    classes: Range<u16>,
}

impl DomElementAttributeCache {
    pub fn memory_usage_report(&self) -> MemoryUsageReport {
        MemoryUsageReport::new()
    }
}

pub struct DomElementData {
    pub tag: u16,
    pub namespace: Option<u16>,
    pub attributes: Vec<DomAttribute>,
    pub attribute_cache: DomElementAttributeCache,
    pub image_idx: Option<u32>,
}

impl DomElementAttributeCache {
    pub fn new(id: Option<u16>, classes: Range<u16>) -> Self {
        Self { id, classes }
    }
}

#[derive(Clone, Debug)]
pub struct DomAttribute {
    pub name: u16,
    pub local_name: u16,
    pub namespace: Option<u16>,
    pub value: u16,
}

impl DomAttribute {
    pub fn memory_usage_report(&self) -> MemoryUsageReport {
        MemoryUsageReport::new()
    }
}

impl Document {
    fn validate_node_id(&self, node_id: DomNodeId) -> Result<usize, DocumentBuildError> {
        let idx = node_id.index();
        if idx
            >= if node_id.is_text() {
                self.text_count()
            } else {
                self.element_count()
            }
        {
            Err(DocumentBuildError::InvalidNodeId { raw: node_id.raw() })
        } else {
            Ok(idx)
        }
    }

    fn validate_element_id(&self, node_id: DomNodeId) -> Result<usize, DocumentBuildError> {
        let index = self.validate_node_id(node_id)?;
        if node_id.is_text() {
            return Err(DocumentBuildError::NodeTypeMismatch {
                raw: node_id.raw(),
                expected_text_parent: false,
            });
        }
        Ok(index)
    }

    fn parent_id(&self, parent: Option<NonZeroU32>) -> Option<DomNodeId> {
        parent.map(|parent| DomNodeId::new(parent.get() - 1))
    }

    fn append_child(&mut self, parent: DomNodeId, child: DomNodeId) -> Result<(), DocumentBuildError> {
        let index = self.validate_element_id(parent)?;
        let children = self.elements.children.get_mut_for_build(index);
        if children.contains(&child) {
            return Err(DocumentBuildError::DuplicateChild {
                parent: parent.raw(),
                child: child.raw(),
            });
        }
        children.push(child);
        Ok(())
    }

    fn rebuild_html_semantics(&mut self) {
        let Some(root) = self.dom_root else { return };
        let mut stack = vec![(root, false, false, false, false)];
        while let Some((
            node,
            inherited_rtl,
            disabled_by_fieldset,
            inherited_editable,
            disabled_optgroup,
        )) = stack.pop()
        {
            let Some(element) = self.try_element_ref(node) else {
                continue;
            };
            let classes = &self.elements.classes[node.index()].range;
            let class_filter = self.class_list[classes.start as usize..classes.end as usize]
                .iter()
                .fold(0, |filter, &class| filter | name_filter_mask(self.strings.get(class), false));
            // Fold ASCII case for all attribute names so the filter remains
            // valid in HTML and XML, including after a document mode change.
            // Store qualified and local names for all namespace lookup forms.
            let attribute_filter =
                self.elements
                    .attributes
                    .get(node.index())
                    .iter()
                    .fold(0, |filter, attribute| {
                        let filter =
                            filter | name_filter_mask(self.strings.get(attribute.name), true);
                        if attribute.local_name == attribute.name {
                            filter
                        } else {
                            filter | name_filter_mask(self.strings.get(attribute.local_name), true)
                        }
                    });
            let tag = element.tag();
            let is_html = element.namespace() == Some(HTML_NAMESPACE);
            let attr = |name| {
                self.elements
                    .attributes
                    .get(node.index())
                    .iter()
                    .find(|attribute| {
                        attribute.namespace.is_none()
                            && self.strings.get(attribute.name).eq_ignore_ascii_case(name)
                    })
                    .map(|attribute| self.strings.get(attribute.value))
            };
            let has_attr = |name| attr(name).is_some();
            let tag_is = |expected: &str| is_html && tag.eq_ignore_ascii_case(expected);
            let input_type = tag_is("input").then(|| attr("type").unwrap_or("text"));

            let rtl = match is_html.then(|| attr("dir")).flatten() {
                Some(value) if value.eq_ignore_ascii_case("ltr") => false,
                Some(value) if value.eq_ignore_ascii_case("rtl") => true,
                Some(value) if value.eq_ignore_ascii_case("auto") => html_auto_direction(self, node).unwrap_or(false),
                _ if tag_is("bdi") => html_auto_direction(self, node).unwrap_or(false),
                _ => inherited_rtl,
            };
            let editable = match is_html.then(|| attr("contenteditable")).flatten() {
                Some(value) if value.is_empty() || value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("plaintext-only") => true,
                Some(value) if value.eq_ignore_ascii_case("false") => false,
                _ => inherited_editable,
            };

            let supports_disabled = tag_is("button") || tag_is("fieldset") || tag_is("input") || tag_is("optgroup") || tag_is("option") || tag_is("select") || tag_is("textarea");
            let fieldset_affected = tag_is("button") || tag_is("fieldset") || tag_is("input") || tag_is("select") || tag_is("textarea");
            let disabled = supports_disabled && (has_attr("disabled") || (fieldset_affected && disabled_by_fieldset) || (tag_is("option") && disabled_optgroup));

            let checked = (input_type.is_some_and(|kind| kind.eq_ignore_ascii_case("checkbox") || kind.eq_ignore_ascii_case("radio")) && has_attr("checked")) || (tag_is("option") && has_attr("selected"));
            let can_be_open = tag_is("details") || tag_is("dialog");
            let open = can_be_open && has_attr("open");
            let supports_required = tag_is("select") || tag_is("textarea") || input_type.is_some_and(input_type_supports_required);
            let required = supports_required && has_attr("required");
            let textual_input = input_type.is_some_and(input_type_supports_readonly);
            let read_write = editable || ((textual_input || tag_is("textarea")) && !disabled && !has_attr("readonly"));
            let placeholder_shown =
                (tag_is("textarea") || input_type.is_some_and(input_type_supports_placeholder)) && has_attr("placeholder") && attr("value").is_none_or(str::is_empty) && (tag_is("input") || element_text_is_empty(self, node));

            let mut state = HtmlElementState::default();
            state.set(HtmlElementState::LINK, (tag_is("a") || tag_is("area")) && has_attr("href"));
            state.set(HtmlElementState::ENABLED, supports_disabled && !disabled);
            state.set(HtmlElementState::DISABLED, disabled);
            state.set(HtmlElementState::CHECKED, checked);
            state.set(HtmlElementState::DEFAULT, checked);
            state.set(HtmlElementState::OPEN, open);
            state.set(HtmlElementState::CLOSED, can_be_open && !open);
            state.set(HtmlElementState::REQUIRED, required);
            state.set(HtmlElementState::OPTIONAL, supports_required && !required);
            state.set(HtmlElementState::READ_WRITE, read_write);
            state.set(HtmlElementState::READ_ONLY, !read_write);
            state.set(HtmlElementState::PLACEHOLDER_SHOWN, placeholder_shown);
            state.set(HtmlElementState::INDETERMINATE, tag_is("progress") && !has_attr("value"));
            state.set(HtmlElementState::RTL, rtl);
            state.set(HtmlElementState::DEFINED, !tag.contains('-'));

            let children = element.children();
            let fieldset_disables_children = tag_is("fieldset") && has_attr("disabled");
            let first_legend = fieldset_disables_children
                .then(|| {
                    children.clone().find(|child| {
                        self.try_element_ref(*child)
                            .is_some_and(|element| element.tag().eq_ignore_ascii_case("legend"))
                    })
                })
                .flatten();
            let optgroup_disables_children = tag_is("optgroup") && disabled;
            for child in children.rev() {
                if self.try_element_ref(child).is_none() {
                    continue;
                }
                let child_disabled_by_fieldset = disabled_by_fieldset || (fieldset_disables_children && Some(child) != first_legend);
                stack.push((child, rtl, child_disabled_by_fieldset, editable, optgroup_disables_children));
            }
            self.elements.classes[node.index()].filter = class_filter;
            self.elements
                .attributes
                .set_filter(node.index(), attribute_filter);
            self.elements.states[node.index()] = state;
        }
        self.rebuild_static_radio_indeterminate_state();
    }

    fn rebuild_static_radio_indeterminate_state(&mut self) {
        let radios = self
            .node_ids()
            .filter_map(|node| {
                let element = self.try_element_ref(node)?;
                (element.namespace() == Some(HTML_NAMESPACE)
                    && element.tag().eq_ignore_ascii_case("input")
                    && element
                        .attr("type")
                        .is_some_and(|kind| kind.eq_ignore_ascii_case("radio")))
                .then(|| {
                    (
                        node,
                        radio_group_owner(self, element),
                        element.attr("name").unwrap_or("").to_owned(),
                        element.html_state().is_checked(),
                    )
                })
            })
            .collect::<Vec<_>>();
        let checked_groups = radios.iter().filter(|(_, _, _, checked)| *checked).map(|(_, owner, name, _)| (*owner, name.as_str())).collect::<HashSet<_>>();
        for (node, owner, name, checked) in &radios {
            let indeterminate = !*checked && !checked_groups.contains(&(*owner, name.as_str()));
            self.elements.states[node.index()].set(HtmlElementState::INDETERMINATE, indeterminate);
        }
    }

    pub fn validate_for_build(&self) -> Result<Option<DomNodeId>, DocumentBuildError> {
        if self.node_count() == 0 {
            return Ok(None);
        }
        let root = self.dom_root.ok_or(DocumentBuildError::MissingRoot)?;
        self.validate_node_id(root)?;
        if root.is_text() {
            return Err(DocumentBuildError::NonElementRoot { raw: root.raw() });
        }
        let mut roots = 0;
        let mut elements = vec![0u8; self.element_count()];
        let mut texts = vec![0u8; self.text_count()];
        for node in self.element_ids() {
            let index = node.index();
            let raw = node.raw();
            let tag = self.elements.names[index].tag;
            for value in [
                Some(tag),
                self.elements.names[index].namespace,
                self.elements.ids.get(index),
            ]
            .into_iter()
            .flatten()
            {
                if value as usize >= self.strings.len() {
                    return Err(DocumentBuildError::InvalidStringIndex { raw, index: value });
                }
            }
            let classes = &self.elements.classes[index].range;
            if classes.start > classes.end || classes.end as usize > self.class_list.len() {
                return Err(DocumentBuildError::InvalidClassRange {
                    raw,
                    start: classes.start,
                    end: classes.end,
                });
            }
            for &class in &self.class_list[classes.start as usize..classes.end as usize] {
                if class as usize >= self.strings.len() {
                    return Err(DocumentBuildError::InvalidStringIndex { raw, index: class });
                }
            }
            for attribute in self.elements.attributes.get(index) {
                for value in [
                    Some(attribute.name),
                    Some(attribute.local_name),
                    attribute.namespace,
                    Some(attribute.value),
                ]
                .into_iter()
                .flatten()
                {
                    if value as usize >= self.strings.len() {
                        return Err(DocumentBuildError::InvalidAttributeIndex {
                            raw,
                            index: value,
                        });
                    }
                }
            }
            if let Some(image) = self.elements.images[index] {
                let image = image.get() - 1;
                if image as usize >= self.images.len() {
                    return Err(DocumentBuildError::InvalidImageIndex { raw, index: image });
                }
            }
            match self.parent_id(self.elements.parents[index]) {
                Some(parent) if self.validate_element_id(parent).is_err() => {
                    return Err(DocumentBuildError::InvalidParent {
                        parent: parent.raw(),
                        child: raw,
                    });
                }
                None => {
                    roots += 1;
                    if node != root {
                        return Err(DocumentBuildError::MultipleRoots);
                    }
                }
                _ => {}
            }
            for &child in self.elements.children.get(index) {
                let child_raw = child.raw();
                self.validate_node_id(child)?;
                let parent = self.node_ref(child).parent().ok_or_else(|| {
                    if child.is_text() {
                        DocumentBuildError::MissingParentForTextNode { raw: child_raw }
                    } else {
                        DocumentBuildError::InvalidParent {
                            parent: raw,
                            child: child_raw,
                        }
                    }
                })?;
                if parent != node {
                    return Err(DocumentBuildError::InvalidParent {
                        parent: raw,
                        child: child_raw,
                    });
                }
                let seen = if child.is_text() {
                    &mut texts[child.index()]
                } else {
                    &mut elements[child.index()]
                };
                if *seen != 0 {
                    return Err(DocumentBuildError::DuplicateChild {
                        parent: raw,
                        child: child_raw,
                    });
                }
                *seen = 1;
            }
        }
        if roots == 0 {
            return Err(DocumentBuildError::MissingRoot);
        }
        if roots != 1 {
            return Err(DocumentBuildError::MultipleRoots);
        }
        for index in 0..self.text_count() {
            let raw = DomNodeId::TEXT_BIT | index as u32;
            let parent = self
                .parent_id(self.texts.parents[index])
                .ok_or(DocumentBuildError::MissingParentForTextNode { raw })?;
            if self.validate_element_id(parent).is_err() {
                return Err(DocumentBuildError::InvalidParent {
                    parent: parent.raw(),
                    child: raw,
                });
            }
        }
        // Explicit traversal avoids recursion proportional to document depth.
        elements.fill(0);
        texts.fill(0);
        // Stack entries carry document-local tagged IDs. Child IDs were
        // checked in the pass above.
        let mut pending = vec![(root, false)];
        while let Some((node, exiting)) = pending.pop() {
            let state = if node.is_text() {
                &mut texts[node.index()]
            } else {
                &mut elements[node.index()]
            };
            if exiting {
                *state = 2;
                continue;
            }
            if *state == 1 {
                return Err(DocumentBuildError::RecursionCycle { raw: node.raw() });
            }
            if *state == 2 {
                continue;
            }
            *state = 1;
            pending.push((node, true));
            if !node.is_text() {
                pending.extend(self.elements.children.get(node.index()).iter().rev()
                    .map(|&child| (child, false)));
            }
        }
        for node in self.node_ids() {
            let state = if node.is_text() {
                texts[node.index()]
            } else {
                elements[node.index()]
            };
            if state == 0 {
                return Err(DocumentBuildError::UnreachableNodes { raw: node.raw() });
            }
        }
        Ok(Some(root))
    }

    pub fn print_dom_tree(&self) {
        if let Some(root) = self.dom_root {
            self.print_dom_node(root, 0);
        }
    }

    fn print_dom_node(&self, id: DomNodeId, depth: usize) {
        let indent = "  ".repeat(depth);
        match self.node_ref(id) {
            NodeRef::Element(element) => {
                println!("{}<{}>", indent, element.tag());
                for child in element.children() {
                    self.print_dom_node(child, depth + 1);
                }
            }
            NodeRef::Text(text) => {
                let truncated: String = text.text().chars().take(80).collect();
                println!("{}\"{}\"", indent, truncated);
            }
        }
    }

    /// Access a node already known to be an element in this document.
    /// The caller establishes its kind through traversal or prior dispatch.
    #[inline]
    pub fn element_ref(&self, node_id: DomNodeId) -> ElementRef<'_> {
        ElementRef { doc: self, node_id }
    }

    /// Access a node already known to be text in this document.
    /// The caller establishes its kind through traversal or prior dispatch.
    #[inline]
    pub fn text_ref(&self, node_id: DomNodeId) -> TextRef<'_> {
        TextRef { doc: self, node_id }
    }

    /// View an ID from this document as an element, or return `None` for text.
    /// ID validity is a construction invariant; access uses normal bounds checks.
    #[inline]
    pub fn try_element_ref(&self, node_id: DomNodeId) -> Option<ElementRef<'_>> {
        if node_id.is_text() {
            return None;
        }
        Some(self.element_ref(node_id))
    }

    /// Find an exact, case-sensitive ID, searching in [`Self::node_ids`] order.
    /// Duplicate IDs resolve to the first matching element.
    pub fn element_by_id(&self, id: &str) -> Option<ElementRef<'_>> {
        let id_idx = self.lookup_string(id)?;
        self.element_ids()
            .find(|node| self.elements.ids.get(node.index()) == Some(id_idx))
            .map(|node_id| ElementRef { doc: self, node_id })
    }

    /// View an ID from this document as text, or return `None` for an element.
    #[inline]
    pub fn try_text_ref(&self, node_id: DomNodeId) -> Option<TextRef<'_>> {
        node_id.is_text().then(|| self.text_ref(node_id))
    }

    /// Dispatch an ID from this document by its element/text bit.
    /// Invalid indices fail at subsequent array access.
    #[inline]
    pub fn node_ref(&self, node_id: DomNodeId) -> NodeRef<'_> {
        if node_id.is_text() {
            NodeRef::Text(TextRef { doc: self, node_id })
        } else {
            NodeRef::Element(ElementRef { doc: self, node_id })
        }
    }

    pub fn element_count(&self) -> usize {
        self.elements.names.len()
    }
    pub fn text_count(&self) -> usize {
        self.texts.values.len()
    }
    pub fn node_count(&self) -> usize {
        self.element_count() + self.text_count()
    }

    /// Elements in insertion order followed by text nodes in insertion order.
    /// Tree order is defined by child lists, never by a node's numeric identity.
    pub fn node_ids(&self) -> impl Iterator<Item = DomNodeId> + '_ {
        self.element_ids().chain(
            (0..self.text_count()).map(|index| {
                DomNodeId::new(DomNodeId::TEXT_BIT | index as u32)
            }),
        )
    }

    pub fn element_ids(&self) -> impl Iterator<Item = DomNodeId> + '_ {
        (0..self.element_count()).map(|index| DomNodeId::new(index as u32))
    }

    pub fn nodes(&self) -> impl Iterator<Item = (DomNodeId, NodeRef<'_>)> + '_ {
        self.node_ids().map(|node_id| (node_id, self.node_ref(node_id)))
    }

    pub fn dom_root(&self) -> Option<DomNodeId> {
        self.dom_root
    }

    pub fn dom_id_in_use(&self, id_idx: u16) -> bool {
        (0..self.element_count()).any(|index| self.elements.ids.get(index) == Some(id_idx))
    }

    pub fn is_text_node(&self, node_id: DomNodeId) -> bool {
        node_id.is_text()
    }

    pub fn is_element_node(&self, node_id: DomNodeId) -> bool {
        !node_id.is_text()
    }







    pub fn is_html_element_in_html_document(&self, node_id: DomNodeId) -> bool {
        self.try_element_ref(node_id)
            .is_some_and(|element| element.is_html_element_in_html_document())
    }

    pub fn dom_ancestors(&self, node_id: DomNodeId) -> DomAncestorIter<'_> {
        let current = self.node_ref(node_id).parent();
        DomAncestorIter { doc: self, current }
    }

    /// Whether `node` is `root` itself or a descendant in the source DOM.
    pub fn dom_contains(&self, root: DomNodeId, node: DomNodeId) -> bool {
        node == root || self.dom_ancestors(node).any(|ancestor| ancestor == root)
    }








    pub fn intern_classes(&mut self, classes: Option<&str>) -> Range<u16> {
        self.try_intern_classes(classes).expect("DOM class capacity exhausted")
    }

    pub fn try_intern_classes(&mut self, classes: Option<&str>) -> Result<Range<u16>, DocumentBuildError> {
        let start = self.class_list.len();
        if let Some(classes) = classes {
            for class in classes.split_whitespace() {
                if self.class_list.len() == usize::from(u16::MAX) {
                    self.class_list.truncate(start);
                    return Err(DocumentBuildError::TooManyClasses);
                }
                let idx = match self.strings.try_intern(class) {
                    Ok(idx) => idx,
                    Err(error) => {
                        self.class_list.truncate(start);
                        return Err(error);
                    }
                };
                self.class_list.push(idx);
            }
        }
        let end = self.class_list.len() as u16;
        Ok(start as u16..end)
    }
}

fn html_auto_direction(doc: &Document, root: DomNodeId) -> Option<bool> {
    let element = doc.try_element_ref(root)?;
    if matches!(element.tag(), "input" | "textarea") {
        if let Some(value) = element.attr("value") {
            if let Some(direction) = first_strong_direction(value) {
                return Some(direction);
            }
        }
    }
    let mut pending: Vec<_> = element.children().rev().collect();
    let mut content = String::new();
    while let Some(node) = pending.pop() {
        if let Some(text) = doc.try_text_ref(node) {
            content.push_str(text.text());
        } else if let Some(element) = doc.try_element_ref(node) {
            if matches!(element.tag(), "bdi" | "script" | "style" | "textarea")
                || element.attr("dir").is_some_and(|value| ["ltr", "rtl", "auto"].iter().any(|direction| value.eq_ignore_ascii_case(direction)))
            {
                continue;
            }
            pending.extend(element.children().rev());
        }
    }
    first_strong_direction(&content)
}

fn first_strong_direction(text: &str) -> Option<bool> {
    match get_base_direction_full(text) {
        Direction::Ltr => Some(false),
        Direction::Rtl => Some(true),
        Direction::Mixed => None,
    }
}

fn input_type_supports_required(kind: &str) -> bool {
    input_type_supports_readonly(kind)
        || ["checkbox", "radio", "file"]
            .iter()
            .any(|candidate| kind.eq_ignore_ascii_case(candidate))
}

fn input_type_supports_readonly(kind: &str) -> bool {
    input_type_supports_placeholder(kind)
        || ["date", "month", "week", "time", "datetime-local"]
            .iter()
            .any(|candidate| kind.eq_ignore_ascii_case(candidate))
}

fn input_type_supports_placeholder(kind: &str) -> bool {
    [
        "text", "search", "url", "tel", "email", "password", "number",
    ]
    .iter()
    .any(|candidate| kind.eq_ignore_ascii_case(candidate))
}

fn element_text_is_empty(document: &Document, root: DomNodeId) -> bool {
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        match document.node_ref(node) {
            NodeRef::Text(text) if !text.text().is_empty() => return false,
            NodeRef::Element(element) => stack.extend(element.children()),
            _ => {}
        }
    }
    true
}

fn radio_group_owner(document: &Document, element: ElementRef<'_>) -> Option<DomNodeId> {
    if let Some(form_id) = element.attr("form")
        && let Some(form) = document.node_ids().find(|node| document.try_element_ref(*node).is_some_and(|candidate| candidate.tag().eq_ignore_ascii_case("form") && candidate.id() == Some(form_id)))
    {
        return Some(form);
    }
    document.dom_ancestors(element.node_id()).find(|ancestor| document.try_element_ref(*ancestor).is_some_and(|candidate| candidate.namespace() == Some(HTML_NAMESPACE) && candidate.tag().eq_ignore_ascii_case("form")))
}

impl<'a> ElementRef<'a> {
    pub fn node_id(self) -> DomNodeId {
        self.node_id
    }

    #[inline]
    pub fn tag(self) -> &'a str {
        self.doc
            .strings
            .get(self.doc.elements.names[self.node_id.index()].tag)
    }

    pub fn id(self) -> Option<&'a str> {
        self.doc
            .elements
            .ids
            .get(self.node_id.index())
            .map(|idx| self.doc.strings.get(idx))
    }

    #[inline]
    pub fn classes(self) -> impl Iterator<Item = &'a str> + 'a {
        let range = self.doc.elements.classes[self.node_id.index()].range.clone();
        self.doc.class_list[range.start as usize..range.end as usize].iter().map(|&idx| self.doc.strings.get(idx))
    }

    pub fn attributes(self) -> impl Iterator<Item = AttributeRef<'a>> + 'a {
        self.doc
            .elements
            .attributes
            .get(self.node_id.index())
            .iter()
            .map(|attribute| AttributeRef {
                doc: self.doc,
                attribute,
            })
    }

    #[inline]
    pub fn has_class(self, class: &str) -> bool {
        name_filter_may_contain(
            self.doc.elements.classes[self.node_id.index()].filter,
            class,
            false,
        ) && self.classes().any(|candidate| candidate == class)
    }

    pub fn parent(self) -> Option<DomNodeId> {
        self.doc
            .parent_id(self.doc.elements.parents[self.node_id.index()])
    }

    pub fn children(self) -> impl DoubleEndedIterator<Item = DomNodeId> + Clone + 'a {
        self.doc
            .elements
            .children
            .get(self.node_id.index())
            .iter()
            .copied()
    }

    /// Direct element children in source order, skipping text nodes.
    pub fn child_elements(self) -> impl Iterator<Item = ElementRef<'a>> + Clone + 'a {
        self.children().filter_map(move |node| self.doc.try_element_ref(node))
    }

    pub fn namespace(self) -> Option<&'a str> {
        self.doc.elements.names[self.node_id.index()]
            .namespace
            .map(|idx| self.doc.strings.get(idx))
    }

    pub fn href(self) -> Option<&'a str> {
        self.attr("href")
    }

    pub fn attr(self, attr: &str) -> Option<&'a str> {
        if !name_filter_may_contain(
            self.doc.elements.attributes.filter(self.node_id.index()),
            attr,
            true,
        ) {
            return None;
        }
        let html_name_matching = self.is_html_element_in_html_document();
        self.doc
            .elements
            .attributes
            .get(self.node_id.index())
            .iter()
            .find(|entry| entry.namespace.is_none() && if html_name_matching { self.doc.strings.get(entry.name).eq_ignore_ascii_case(attr) } else { self.doc.strings.get(entry.name) == attr })
            .map(|entry| self.doc.strings.get(entry.value))
    }

    pub fn has_attr(self, attr: &str) -> bool {
        self.attr(attr).is_some()
    }

    pub fn attr_expanded(self, namespace: Option<&str>, local_name: &str) -> Option<&'a str> {
        if !name_filter_may_contain(
            self.doc.elements.attributes.filter(self.node_id.index()),
            local_name,
            true,
        ) {
            return None;
        }
        self.doc
            .elements
            .attributes
            .get(self.node_id.index())
            .iter()
            .find(|entry| {
                self.doc.strings.get(entry.local_name) == local_name
                    && match (namespace, entry.namespace) {
                        (None, None) => true,
                        (Some(expected), Some(actual)) => self.doc.strings.get(actual) == expected,
                        _ => false,
                    }
            })
            .map(|entry| self.doc.strings.get(entry.value))
    }

    pub fn has_attr_expanded(self, namespace: Option<&str>, local_name: &str) -> bool {
        self.attr_expanded(namespace, local_name).is_some()
    }

    pub fn attr_in_any_namespace(self, local_name: &str) -> Option<&'a str> {
        if !name_filter_may_contain(
            self.doc.elements.attributes.filter(self.node_id.index()),
            local_name,
            true,
        ) {
            return None;
        }
        self.doc
            .elements
            .attributes
            .get(self.node_id.index())
            .iter()
            .find(|entry| self.doc.strings.get(entry.local_name) == local_name)
            .map(|entry| self.doc.strings.get(entry.value))
    }

    pub fn has_attr_in_any_namespace(self, local_name: &str) -> bool {
        self.attr_in_any_namespace(local_name).is_some()
    }

    /// Image identity in this element's source document.
    pub fn image(self) -> Option<ImageId> {
        self.doc.elements.images[self.node_id.index()]
            .map(|index| index.get() - 1)
            .map(dom_values::image_id_new)
    }

    pub fn previous_element_sibling(self) -> Option<ElementRef<'a>> {
        let parent = self.doc.element_ref(self.parent()?);
        let mut previous = None;
        for child_idx in parent.children() {
            if child_idx == self.node_id {
                return previous;
            }
            if let Some(child) = self.doc.try_element_ref(child_idx) {
                previous = Some(child);
            }
        }
        None
    }
}

impl<'a> ElementRef<'a> {
    pub fn tag_idx(self) -> u16 {
        self.doc.elements.names[self.node_id.index()].tag
    }

    pub fn has_class_idx(self, class: u16, mask: u32) -> bool {
        let index = self.node_id.index();
        let classes = &self.doc.elements.classes[index].range;
        self.doc.elements.classes[index].filter & mask == mask
            && self.doc.class_list[classes.start as usize..classes.end as usize].contains(&class)
    }

    /// Use a document-bound name and fingerprint while retaining HTML's ASCII
    /// case-insensitive lookup, including manually constructed mixed-case names.
    pub fn attr_with_name_idx(
        self,
        name: &str,
        name_idx: Option<u16>,
        mask: u32,
    ) -> Option<&'a str> {
        let index = self.node_id.index();
        if self.doc.elements.attributes.filter(index) & mask != mask {
            return None;
        }
        let html = self.is_html_element_in_html_document();
        if !html && name_idx.is_none() {
            return None;
        }
        self
            .doc
            .elements
            .attributes
            .get(index)
            .iter()
            .find(|attribute| {
                attribute.namespace.is_none()
                    && (Some(attribute.name) == name_idx
                        || (html
                            && self
                                .doc
                                .strings
                                .get(attribute.name)
                                .eq_ignore_ascii_case(name)))
            })
            .map(|attribute| self.doc.strings.get(attribute.value))
    }

    pub fn id_idx(self) -> Option<u16> {
        self.doc.elements.ids.get(self.node_id.index())
    }

    pub fn attr_value_idx_no_namespace(self, name_idx: u16) -> Option<u16> {
        self
            .doc
            .elements
            .attributes
            .get(self.node_id.index())
            .iter()
            .find(|entry| entry.namespace.is_none() && entry.name == name_idx)
            .map(|entry| entry.value)
    }

    pub fn html_state(self) -> HtmlElementState {
        self.doc.elements.states[self.node_id.index()]
    }

    pub fn image_idx(self) -> Option<u32> {
        self.doc.elements.images[self.node_id.index()].map(|index| index.get() - 1)
    }

    #[inline]
    pub fn is_html_element_in_html_document(self) -> bool {
        self.doc.mode == DocumentMode::Html
            && self.doc.strings.html_namespace.is_some_and(|namespace| {
                self.doc.elements.names[self.node_id.index()].namespace == Some(namespace)
            })
    }

    /// Normalized HTML ordered-list start value. XML elements with the same
    /// spelling deliberately have no HTML list semantics.
    pub fn html_list_start(self) -> Option<i64> {
        (self.namespace() == Some(HTML_NAMESPACE) && self.tag().eq_ignore_ascii_case("ol"))
            .then(|| self.attr("start").and_then(parse_html_integer))
            .flatten()
    }

    pub fn html_list_reversed(self) -> bool {
        self.namespace() == Some(HTML_NAMESPACE)
            && self.tag().eq_ignore_ascii_case("ol")
            && self.has_attr("reversed")
    }

    pub fn html_list_item_value(self) -> Option<i64> {
        (self.namespace() == Some(HTML_NAMESPACE) && self.tag().eq_ignore_ascii_case("li"))
            .then(|| self.attr("value").and_then(parse_html_integer))
            .flatten()
    }
}

fn parse_html_integer(value: &str) -> Option<i64> {
    value.trim().parse().ok()
}

impl<'a> AttributeRef<'a> {
    pub fn name(self) -> &'a str {
        self.doc.strings.get(self.attribute.name)
    }

    pub fn local_name(self) -> &'a str {
        self.doc.strings.get(self.attribute.local_name)
    }

    pub fn namespace(self) -> Option<&'a str> {
        self.attribute.namespace.map(|namespace| self.doc.strings.get(namespace))
    }

    pub fn value(self) -> &'a str {
        self.doc.strings.get(self.attribute.value)
    }
}

impl<'a> NodeRef<'a> {
    pub fn node_id(self) -> DomNodeId {
        match self {
            NodeRef::Element(element) => element.node_id(),
            NodeRef::Text(text) => text.node_id(),
        }
    }

    pub fn parent(self) -> Option<DomNodeId> {
        match self {
            NodeRef::Element(element) => element.parent(),
            NodeRef::Text(text) => text.parent(),
        }
    }
}

impl<'a> TextRef<'a> {
    pub fn node_id(self) -> DomNodeId {
        self.node_id
    }

    pub fn text(self) -> &'a str {
        &self.doc.texts.values[self.node_id.index()]
    }

    pub fn parent(self) -> Option<DomNodeId> {
        self.doc
            .parent_id(self.doc.texts.parents[self.node_id.index()])
    }
}

pub struct DomAncestorIter<'a> {
    doc: &'a Document,
    current: Option<DomNodeId>,
}

impl<'a> Iterator for DomAncestorIter<'a> {
    type Item = DomNodeId;

    fn next(&mut self) -> Option<Self::Item> {
        let id = self.current?;
        // Construction guarantees that every ancestor is an element from the
        // same immutable document; traversal does not revalidate these IDs.
        self.current = self.doc.parent_id(self.doc.elements.parents[id.index()]);
        Some(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_element_state_remains_two_bytes() {
        assert_eq!(std::mem::size_of::<HtmlElementState>(), std::mem::size_of::<u16>());
    }

    #[test]
    fn finish_allows_empty_document() {
        let mut builder = DocumentBuilder::new();
        let document = builder.finish().expect("empty document");
        assert!(document.dom_root().is_none());
        assert_eq!(document.node_count(), 0);
    }

    #[test]
    fn finish_with_explicit_root_populates_root() {
        let mut builder = DocumentBuilder::new();
        let html_tag = builder.intern_string("html");
        let body_tag = builder.intern_string("body");
        let html = builder.append_dom_element(None, DomElementData { tag: html_tag, namespace: None, attributes: Vec::new(), attribute_cache: DomElementAttributeCache::new(None, 0..0), image_idx: None }).expect("root append");
        builder.set_dom_root(html).expect("root set");
        let body = builder.append_dom_element(Some(html), DomElementData { tag: body_tag, namespace: None, attributes: Vec::new(), attribute_cache: DomElementAttributeCache::new(None, 0..0), image_idx: None }).expect("body append");
        let html = builder.document().dom_root().expect("rooted builder stores html root");
        let document = builder.finish().expect("document build");
        assert_eq!(document.dom_root(), Some(html));
        let body_ref = document.try_element_ref(body).expect("body element");
        assert_eq!(body_ref.parent(), Some(html));
    }

    #[test]
    fn element_lookup_preserves_first_duplicate_and_exact_id_matching() {
        let mut builder = DocumentBuilder::new();
        let tag = builder.intern_string("span");
        let id_name = builder.intern_string("id");
        let element = || DomElementData {
            tag,
            namespace: None,
            attributes: Vec::new(),
            attribute_cache: DomElementAttributeCache::new(None, 0..0),
            image_idx: None,
        };
        let root = builder.append_dom_element(None, element()).unwrap();
        builder.set_dom_root(root).unwrap();
        builder.append_dom_text(root, "dup").unwrap();
        let mut nodes = Vec::new();
        for id in ["dup", "Dup", "", "dup"] {
            let node = builder.append_dom_element(Some(root), element()).unwrap();
            let id_idx = builder.intern_string(id);
            builder.set_dom_id_attribute(node, id_idx, id_name).unwrap();
            nodes.push(node);
        }
        let document = builder.finish().unwrap();
        for (id, expected) in [("dup", nodes[0]), ("Dup", nodes[1]), ("", nodes[2])] {
            assert_eq!(
                document.element_by_id(id).map(ElementRef::node_id),
                Some(expected)
            );
        }
        assert!(document.element_by_id("span").is_none());
        assert!(document.element_by_id("not-interned").is_none());
    }

    #[test]
    fn html_attribute_names_are_ascii_case_insensitive() {
        let mut builder = DocumentBuilder::new();
        builder.set_mode(DocumentMode::Html);
        let html_tag = builder.intern_string("html");
        let html_namespace = builder.intern_string(HTML_NAMESPACE);
        let title_name = builder.intern_string("title");
        let title_value = builder.intern_string("PASS");
        let attributes = vec![DomAttribute { name: title_name, local_name: title_name, namespace: None, value: title_value }];
        let html = builder.append_dom_element(None, DomElementData { tag: html_tag, namespace: Some(html_namespace), attributes, attribute_cache: DomElementAttributeCache::new(None, 0..0), image_idx: None }).expect("html root append");
        builder.set_dom_root(html).expect("set html root");
        let document = builder.finish().expect("document build");
        let element = document.try_element_ref(html).expect("html element");

        assert_eq!(element.attr("Title"), Some("PASS"));
        assert_eq!(element.attr("TITLE"), Some("PASS"));
    }

    #[test]
    fn xml_attribute_names_remain_case_sensitive() {
        let mut builder = DocumentBuilder::new();
        builder.set_mode(DocumentMode::Xml);
        let root_tag = builder.intern_string("root");
        let title_name = builder.intern_string("title");
        let title_value = builder.intern_string("PASS");
        let attributes = vec![DomAttribute { name: title_name, local_name: title_name, namespace: None, value: title_value }];
        let root = builder.append_dom_element(None, DomElementData { tag: root_tag, namespace: None, attributes, attribute_cache: DomElementAttributeCache::new(None, 0..0), image_idx: None }).expect("xml root append");
        builder.set_dom_root(root).expect("set xml root");
        let document = builder.finish().expect("document build");
        let element = document.try_element_ref(root).expect("xml element");

        assert_eq!(element.attr("title"), Some("PASS"));
        assert_eq!(element.attr("Title"), None);
    }

    #[test]
    fn set_dom_root_rejects_non_element_node() {
        let mut builder = DocumentBuilder::new();
        let html_tag = builder.intern_string("html");
        let html = builder.append_dom_element(None, DomElementData { tag: html_tag, namespace: None, attributes: Vec::new(), attribute_cache: DomElementAttributeCache::new(None, 0..0), image_idx: None }).expect("append html");
        let body = builder.append_dom_text(html, "text").expect("append body text");
        builder.document_mut().texts.parents[body.index()] = None;
        assert!(matches!(
            builder.set_dom_root(body),
            Err(DocumentBuildError::NonElementRoot { raw }) if raw == body.raw()
        ));
    }

    #[test]
    fn invalid_element_targets_are_rejected_before_builder_mutation() {
        let mut builder = DocumentBuilder::new();
        let tag = builder.intern_string("div");
        let id_name = builder.intern_string("id");
        let id_value = builder.intern_string("target");
        let element = || DomElementData {
            tag,
            namespace: None,
            attributes: Vec::new(),
            attribute_cache: DomElementAttributeCache::new(None, 0..0),
            image_idx: None,
        };
        let root = builder.append_dom_element(None, element()).unwrap();
        let text = builder.append_dom_text(root, "text").unwrap();
        let missing = DomNodeId::new(u32::MAX);
        for (node, expected) in [
            (
                missing,
                DocumentBuildError::InvalidNodeId { raw: missing.raw() },
            ),
            (
                text,
                DocumentBuildError::NodeTypeMismatch {
                    raw: text.raw(),
                    expected_text_parent: false,
                },
            ),
        ] {
            assert_eq!(
                builder.append_dom_element(Some(node), element()),
                Err(expected)
            );
            assert_eq!(builder.append_dom_text(node, "rejected"), Err(expected));
            assert_eq!(
                builder.set_dom_id_attribute(node, id_value, id_name),
                Err(expected)
            );
            assert_eq!(builder.document().node_count(), 2);
        }
        builder.set_dom_root(root).unwrap();
        let document = builder.finish().unwrap();
        let root = document.try_element_ref(root).unwrap();
        assert_eq!(root.children().collect::<Vec<_>>(), [text]);
        assert_eq!(root.attr("id"), None);
    }

    #[test]
    fn finish_rejects_invalid_string_and_image_indices() {
        let mut builder = DocumentBuilder::new();
        let root = builder
            .append_dom_element(None, DomElementData { tag: u16::MAX, namespace: None, attributes: Vec::new(), attribute_cache: DomElementAttributeCache::new(None, 0..0), image_idx: None })
            .expect("invalid indices are accepted only in the in-progress builder");
        builder.set_dom_root(root).expect("root identity is valid");
        assert!(matches!(builder.finish(), Err(DocumentBuildError::InvalidStringIndex { .. })));

        let mut builder = DocumentBuilder::new();
        let tag = builder.intern_string("html");
        let root = builder
            .append_dom_element(None, DomElementData { tag, namespace: None, attributes: Vec::new(), attribute_cache: DomElementAttributeCache::new(None, 0..0), image_idx: Some(0) })
            .expect("invalid image references are checked at finalization");
        builder.set_dom_root(root).expect("root identity is valid");
        assert!(matches!(builder.finish(), Err(DocumentBuildError::InvalidImageIndex { index: 0, .. })));
    }

    #[test]
    fn finish_rejects_missing_text_parent() {
        let mut builder = DocumentBuilder::new();
        let tag = builder.intern_string("html");
        let root = builder.append_dom_element(None, DomElementData { tag, namespace: None, attributes: Vec::new(), attribute_cache: DomElementAttributeCache::new(None, 0..0), image_idx: None }).expect("root append");
        let text = builder.append_dom_text(root, "text").expect("text append");
        builder.set_dom_root(root).expect("root set");
        builder.document_mut().texts.parents[text.index()] = None;
        assert!(matches!(builder.finish(), Err(DocumentBuildError::MissingParentForTextNode { .. })));
    }

    #[test]
    fn finish_rejects_multiple_toplevel_roots() {
        let mut builder = DocumentBuilder::new();
        let html_tag = builder.intern_string("html");
        let section_tag = builder.intern_string("section");
        let html = builder.append_dom_element(None, DomElementData { tag: html_tag, namespace: None, attributes: Vec::new(), attribute_cache: DomElementAttributeCache::new(None, 0..0), image_idx: None }).expect("append root");
        builder.set_dom_root(html).expect("rooted");
        let _second_root =
            builder.append_dom_element(None, DomElementData { tag: section_tag, namespace: None, attributes: Vec::new(), attribute_cache: DomElementAttributeCache::new(None, 0..0), image_idx: None }).expect("append second root");

        assert!(matches!(builder.finish(), Err(DocumentBuildError::MultipleRoots)));
    }

    #[test]
    fn finish_rejects_recursion_cycle() {
        let mut builder = DocumentBuilder::new();
        let html_tag = builder.intern_string("html");
        let section_tag = builder.intern_string("section");
        let html = builder.append_dom_element(None, DomElementData { tag: html_tag, namespace: None, attributes: Vec::new(), attribute_cache: DomElementAttributeCache::new(None, 0..0), image_idx: None }).expect("append root");
        let section = builder.append_dom_element(Some(html), DomElementData { tag: section_tag, namespace: None, attributes: Vec::new(), attribute_cache: DomElementAttributeCache::new(None, 0..0), image_idx: None }).expect("append child");
        builder.set_dom_root(html).expect("rooted");

        builder
            .document_mut()
            .elements
            .children
            .get_mut_for_build(section.index())
            .push(html);
        assert!(matches!(
            builder.finish(),
            Err(DocumentBuildError::RecursionCycle { raw: _ }
                | DocumentBuildError::InvalidParent { .. })
        ));
    }
}
