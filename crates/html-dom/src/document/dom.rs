use super::MemoryUsageReport;
use super::{Document, DocumentMode};
use std::collections::HashSet;
use std::fmt;
use std::marker::PhantomData;
use std::ops::Range;

const HTML_NAMESPACE: &str = "http://www.w3.org/1999/xhtml";

/// Opaque node identity branded by its source document.
///
/// Raw integers cannot be converted into node identities:
///
/// ```compile_fail
/// let _: html_dom::DomNodeId = 0u32.into();
/// ```
///
/// Its fields are private, so a branded identity cannot be forged either:
///
/// ```compile_fail
/// let _ = html_dom::DomNodeId { raw: 0, document_token: 1 };
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct DomNodeId {
    raw: u32,
    document_token: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DocumentBuildError {
    DuplicateChild { parent: u32, child: u32 },
    ForeignNodeId { raw: u32 },
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
    ToplevelTableEntries { kind: &'static str, raw: u32, index: u16 },
    TooManyNodes,
}

impl fmt::Display for DocumentBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateChild { parent, child } => write!(formatter, "duplicate child {child} in parent {parent}"),
            Self::ForeignNodeId { raw } => write!(formatter, "node id {raw} does not belong to this document"),
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
            Self::ToplevelTableEntries { kind, raw, index } => write!(formatter, "{kind} entry {raw} references invalid string index {index}"),
            Self::TooManyNodes => formatter.write_str("document has too many nodes"),
        }
    }
}

impl std::error::Error for DocumentBuildError {}

/// Marker for an un-rooted in-progress DOM builder.
#[derive(Debug)]
pub struct NeedsRoot;

/// Marker for a rooted in-progress DOM builder.
#[derive(Debug)]
pub struct Rooted;

/// Marker for a builder with nodes but no confirmed root.
#[derive(Debug)]
pub struct Rootless;

pub struct DocumentBuilder<State = NeedsRoot> {
    document: Document,
    _state: PhantomData<State>,
}

impl Default for DocumentBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl DocumentBuilder {
    pub fn new() -> Self {
        Self { document: Document::new(), _state: PhantomData }
    }
}

impl<State> DocumentBuilder<State> {
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

    pub fn push_toc_entry(&mut self, entry: crate::DocumentTocEntry) {
        self.document.toc_entries.push(entry);
    }
}

impl<State> DocumentBuilder<State> {
    fn append_dom_element_raw(&mut self, parent: Option<DomNodeId>, data: DomElementData) -> Result<DomNodeId, DocumentBuildError> {
        let parent = if let Some(parent) = parent {
            let parent_idx = self.document.validate_node_id(parent)?;
            match self.document.dom_nodes.get(parent_idx) {
                Some(DomNode::Element(_)) => Some(parent),
                Some(DomNode::Text(_)) => return Err(DocumentBuildError::NodeTypeMismatch { raw: parent.raw, expected_text_parent: false }),
                None => return Err(DocumentBuildError::InvalidNodeId { raw: parent.raw }),
            }
        } else {
            None
        };

        let raw = self.document.dom_nodes.len().try_into().map_err(|_| DocumentBuildError::TooManyNodes)?;
        let node_id = DomNodeId { raw, document_token: self.document.lineage_token() };
        self.document.dom_nodes.push(DomNode::Element(DomElement {
            tag: data.tag,
            namespace: data.namespace,
            parent,
            children: Vec::new(),
            attributes: data.attributes,
            attribute_cache: data.attribute_cache,
            html_state: HtmlElementState::default(),
            image_idx: data.image_idx,
        }));
        if let Some(parent) = parent {
            self.document.append_child(parent, node_id)?;
        }
        Ok(node_id)
    }

    fn append_dom_text_raw(&mut self, parent: DomNodeId, text: impl Into<String>) -> Result<DomNodeId, DocumentBuildError> {
        let parent_idx = self.document.validate_node_id(parent)?;
        match self.document.dom_nodes.get(parent_idx) {
            Some(DomNode::Element(_)) => {}
            Some(DomNode::Text(_)) => return Err(DocumentBuildError::NodeTypeMismatch { raw: parent.raw, expected_text_parent: false }),
            None => return Err(DocumentBuildError::InvalidNodeId { raw: parent.raw }),
        }

        let raw = self.document.dom_nodes.len().try_into().map_err(|_| DocumentBuildError::TooManyNodes)?;
        let node_id = DomNodeId { raw, document_token: self.document.lineage_token() };
        self.document.dom_nodes.push(DomNode::Text(DomText { text: text.into(), parent: Some(parent) }));
        self.document.append_child(parent, node_id)?;
        Ok(node_id)
    }

    fn validate_and_set_root(&mut self, node_id: DomNodeId) -> Result<(), DocumentBuildError> {
        let idx = self.document.validate_node_id(node_id)?;
        let raw = node_id.raw;
        if let Some(node) = self.document.dom_nodes.get(idx) {
            if node.parent().is_some() {
                return Err(DocumentBuildError::RootHasParent { raw });
            }
            if !matches!(node, DomNode::Element(_)) {
                return Err(DocumentBuildError::NonElementRoot { raw });
            }
        }
        self.document.dom_root = Some(node_id);
        Ok(())
    }

    pub fn set_dom_id_attribute(&mut self, node_id: DomNodeId, id_idx: u16, id_name_idx: u16) -> Result<(), DocumentBuildError> {
        let idx = self.document.validate_node_id(node_id)?;
        let Some(DomNode::Element(element)) = self.document.dom_nodes.get_mut(idx) else {
            return Err(DocumentBuildError::NodeTypeMismatch { raw: node_id.raw, expected_text_parent: false });
        };
        element.attribute_cache.id = Some(id_idx);
        element.attributes.push(DomAttribute { name: id_name_idx, local_name: id_name_idx, namespace: None, value: id_idx });
        Ok(())
    }
}

impl DocumentBuilder<NeedsRoot> {
    pub fn append_dom_element(mut self, parent: Option<DomNodeId>, data: DomElementData) -> Result<(DomNodeId, DocumentBuilder<Rootless>), DocumentBuildError> {
        let node_id = self.append_dom_element_raw(parent, data)?;
        Ok((node_id, DocumentBuilder { document: self.document, _state: PhantomData }))
    }

    pub fn append_dom_text(mut self, parent: DomNodeId, text: impl Into<String>) -> Result<(DomNodeId, DocumentBuilder<Rootless>), DocumentBuildError> {
        let node_id = self.append_dom_text_raw(parent, text)?;
        Ok((node_id, DocumentBuilder { document: self.document, _state: PhantomData }))
    }

    pub fn set_dom_root(mut self, node_id: DomNodeId) -> Result<DocumentBuilder<Rooted>, DocumentBuildError> {
        if self.document.dom_root.is_some() {
            return Err(DocumentBuildError::MultipleRoots);
        }
        self.validate_and_set_root(node_id)?;
        Ok(DocumentBuilder { document: self.document, _state: PhantomData })
    }

    /// Finalize an explicitly rootless document.
    ///
    /// This is only valid when no DOM nodes were added. The call site must use
    /// this path explicitly so non-empty DOMs must be rooted before finalization.
    pub fn finish_without_root(self) -> Result<Document, DocumentBuildError> {
        let root = self.document.validate_for_build()?;
        let mut document = self.document;
        document.dom_root = root;
        document.rebuild_html_semantics();
        document.rebuild_document_toc_entries();
        Ok(document)
    }
}

impl DocumentBuilder<Rootless> {
    pub fn append_dom_element(&mut self, parent: Option<DomNodeId>, data: DomElementData) -> Result<DomNodeId, DocumentBuildError> {
        self.append_dom_element_raw(parent, data)
    }

    pub fn append_dom_text(&mut self, parent: DomNodeId, text: impl Into<String>) -> Result<DomNodeId, DocumentBuildError> {
        self.append_dom_text_raw(parent, text)
    }

    pub fn set_dom_root(mut self, node_id: DomNodeId) -> Result<DocumentBuilder<Rooted>, DocumentBuildError> {
        self.validate_and_set_root(node_id)?;
        Ok(DocumentBuilder { document: self.document, _state: PhantomData })
    }
}

impl DocumentBuilder<Rooted> {
    pub fn set_dom_root(&mut self, node_id: DomNodeId) -> Result<(), DocumentBuildError> {
        if let Some(existing_root) = self.document.dom_root {
            if existing_root != node_id {
                return Err(DocumentBuildError::MultipleRoots);
            }
            return Ok(());
        }
        self.validate_and_set_root(node_id)
    }

    pub fn append_dom_element(&mut self, parent: Option<DomNodeId>, data: DomElementData) -> Result<DomNodeId, DocumentBuildError> {
        self.append_dom_element_raw(parent, data)
    }

    pub fn append_dom_text(&mut self, parent: DomNodeId, text: impl Into<String>) -> Result<DomNodeId, DocumentBuildError> {
        self.append_dom_text_raw(parent, text)
    }

    pub fn rebuild_document_toc_entries(&mut self) {
        self.document.rebuild_document_toc_entries();
    }

    pub fn finish(self) -> Result<Document, DocumentBuildError> {
        let root = self.document.validate_for_build()?;
        let mut document = self.document;
        document.dom_root = root;
        document.rebuild_html_semantics();
        document.rebuild_document_toc_entries();
        Ok(document)
    }
}

impl DomNodeId {
    fn new(raw: u32, document_token: u64) -> Self {
        Self { raw, document_token }
    }

    pub fn index(self) -> usize {
        self.raw as usize
    }

    pub fn raw(self) -> u32 {
        self.raw
    }

    fn token(self) -> u64 {
        self.document_token
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

#[derive(Clone, Debug)]
pub enum DomNode {
    Element(DomElement),
    Text(DomText),
}

impl DomNode {
    pub fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        match self {
            DomNode::Element(element) => report.extend_prefixed("DomNode::Element", element.memory_usage_report()),
            DomNode::Text(text) => report.extend_prefixed("DomNode::Text", text.memory_usage_report()),
        }
        report
    }
}

#[derive(Clone, Debug)]
pub struct DomElement {
    pub tag: u16,
    pub namespace: Option<u16>,

    pub parent: Option<DomNodeId>,
    pub children: Vec<DomNodeId>,
    pub attributes: Vec<DomAttribute>,
    attribute_cache: DomElementAttributeCache,
    html_state: HtmlElementState,
    pub image_idx: Option<u32>,
}

/// Compact, immutable HTML state derived once when DOM construction finishes.
///
/// Dynamic browser state is deliberately absent: this document model has no
/// JavaScript or native form-control state. Consumers therefore cannot confuse
/// raw attribute presence with normalized static HTML semantics.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(transparent)]
pub struct HtmlElementState(u32);

impl HtmlElementState {
    const LINK: u32 = 1 << 0;
    const ENABLED: u32 = 1 << 1;
    const DISABLED: u32 = 1 << 2;
    const CHECKED: u32 = 1 << 3;
    const DEFAULT: u32 = 1 << 4;
    const OPEN: u32 = 1 << 5;
    const CLOSED: u32 = 1 << 6;
    const REQUIRED: u32 = 1 << 7;
    const OPTIONAL: u32 = 1 << 8;
    const READ_ONLY: u32 = 1 << 9;
    const READ_WRITE: u32 = 1 << 10;
    const PLACEHOLDER_SHOWN: u32 = 1 << 11;
    const INDETERMINATE: u32 = 1 << 12;
    const RTL: u32 = 1 << 13;
    const DEFINED: u32 = 1 << 14;

    fn set(&mut self, flag: u32, value: bool) {
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

impl DomElement {
    pub fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        report.add_slice_storage::<DomNodeId>("DomElement.children.storage", self.children.capacity(), self.children.len());
        report.add_slice_storage::<DomAttribute>("DomElement.attributes.storage", self.attributes.capacity(), self.attributes.len());
        report
    }
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

#[derive(Clone, Debug)]
pub struct DomText {
    pub text: String,
    pub parent: Option<DomNodeId>,
}

impl DomText {
    pub fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        report.add_slice_storage::<u8>("DomText.text.storage", self.text.capacity(), self.text.len());
        report
    }
}

impl DomNode {
    pub fn parent(&self) -> Option<DomNodeId> {
        match self {
            DomNode::Element(e) => e.parent,
            DomNode::Text(t) => t.parent,
        }
    }

    pub fn as_element(&self) -> Option<&DomElement> {
        match self {
            DomNode::Element(e) => Some(e),
            DomNode::Text(_) => None,
        }
    }

    pub fn as_text(&self) -> Option<&DomText> {
        match self {
            DomNode::Element(_) => None,
            DomNode::Text(t) => Some(t),
        }
    }
}

impl Document {
    fn validate_node_id(&self, node_id: DomNodeId) -> Result<usize, DocumentBuildError> {
        if node_id.token() != self.lineage_token() {
            return Err(DocumentBuildError::ForeignNodeId { raw: node_id.raw() });
        }
        let idx = node_id.index();
        if idx >= self.dom_nodes.len() { Err(DocumentBuildError::InvalidNodeId { raw: node_id.raw() }) } else { Ok(idx) }
    }

    fn lineage_token(&self) -> u64 {
        self.lineage.0.get()
    }

    fn has_valid_token(&self, node_id: DomNodeId) -> bool {
        node_id.token() == self.lineage_token()
    }

    fn node_or_err(&self, node_id: DomNodeId) -> Option<usize> {
        if !self.has_valid_token(node_id) {
            return None;
        }
        let idx = node_id.index();
        (idx < self.dom_nodes.len()).then_some(idx)
    }

    fn append_child(&mut self, parent: DomNodeId, child: DomNodeId) -> Result<(), DocumentBuildError> {
        let parent_idx = self.validate_node_id(parent)?;
        let parent_id = parent;
        let Some(DomNode::Element(parent)) = self.dom_nodes.get_mut(parent_idx) else {
            return Err(DocumentBuildError::NodeTypeMismatch { raw: parent.raw(), expected_text_parent: false });
        };

        if parent.children.iter().any(|candidate| candidate.raw() == child.raw() && candidate.token() == child.token()) {
            return Err(DocumentBuildError::DuplicateChild { parent: parent_id.raw(), child: child.raw() });
        }

        parent.children.push(child);
        Ok(())
    }

    fn rebuild_html_semantics(&mut self) {
        let Some(root) = self.dom_root else { return };
        let mut stack = vec![(root, false, false, false, false)];
        while let Some((node, inherited_rtl, disabled_by_fieldset, inherited_editable, disabled_optgroup)) = stack.pop() {
            let Some(element) = self.dom_nodes.get(node.index()).and_then(DomNode::as_element) else { continue };
            let tag = self.strings.get(element.tag);
            let is_html = element.namespace.is_some_and(|namespace| self.strings.get(namespace) == HTML_NAMESPACE);
            let attr = |name| element.attributes.iter().find(|attribute| attribute.namespace.is_none() && self.strings.get(attribute.name).eq_ignore_ascii_case(name)).map(|attribute| self.strings.get(attribute.value));
            let has_attr = |name| attr(name).is_some();
            let tag_is = |expected: &str| is_html && tag.eq_ignore_ascii_case(expected);
            let input_type = tag_is("input").then(|| attr("type").unwrap_or("text"));

            let rtl = match is_html.then(|| attr("dir")).flatten() {
                Some(value) if value.eq_ignore_ascii_case("ltr") => false,
                Some(value) if value.eq_ignore_ascii_case("rtl") => true,
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

            let children = element.children.clone();
            let fieldset_disables_children = tag_is("fieldset") && has_attr("disabled");
            let first_legend = fieldset_disables_children.then(|| children.iter().copied().find(|child| self.element_ref(*child).is_some_and(|element| element.tag().eq_ignore_ascii_case("legend")))).flatten();
            let optgroup_disables_children = tag_is("optgroup") && disabled;
            if let Some(DomNode::Element(element)) = self.dom_nodes.get_mut(node.index()) {
                element.html_state = state;
            }
            for child in children.into_iter().rev() {
                if self.element_ref(child).is_none() {
                    continue;
                }
                let child_disabled_by_fieldset = disabled_by_fieldset || (fieldset_disables_children && Some(child) != first_legend);
                stack.push((child, rtl, child_disabled_by_fieldset, editable, optgroup_disables_children));
            }
        }
        self.rebuild_static_radio_indeterminate_state();
    }

    fn rebuild_static_radio_indeterminate_state(&mut self) {
        let radios = self
            .node_ids()
            .filter_map(|node| {
                let element = self.element_ref(node)?;
                (element.namespace() == Some(HTML_NAMESPACE) && element.tag().eq_ignore_ascii_case("input") && element.attr("type").is_some_and(|kind| kind.eq_ignore_ascii_case("radio")))
                    .then(|| (node, radio_group_owner(self, element), element.attr("name").unwrap_or("").to_owned(), element.html_state().is_checked()))
            })
            .collect::<Vec<_>>();
        let checked_groups = radios.iter().filter(|(_, _, _, checked)| *checked).map(|(_, owner, name, _)| (*owner, name.clone())).collect::<HashSet<_>>();
        for (node, owner, name, checked) in radios {
            let indeterminate = !checked && !checked_groups.contains(&(owner, name));
            if let Some(DomNode::Element(element)) = self.dom_nodes.get_mut(node.index()) {
                element.html_state.set(HtmlElementState::INDETERMINATE, indeterminate);
            }
        }
    }

    pub fn node_id_from_raw(&self, raw: u32) -> Option<DomNodeId> {
        ((raw as usize) < self.dom_nodes.len()).then_some(DomNodeId::new(raw, self.lineage_token()))
    }

    pub fn validate_for_build(&self) -> Result<Option<DomNodeId>, DocumentBuildError> {
        if self.dom_nodes.is_empty() {
            return Ok(None);
        }

        if self.dom_root.is_none() {
            return Err(DocumentBuildError::MissingRoot);
        }

        let mut roots = Vec::new();

        for (raw, node) in self.dom_nodes.iter().enumerate() {
            let raw = raw as u32;
            let node_id = DomNodeId::new(raw, self.lineage_token());

            match node {
                DomNode::Element(element) => {
                    if element.tag as usize >= self.strings.len() {
                        return Err(DocumentBuildError::InvalidStringIndex { raw, index: element.tag });
                    }
                    if let Some(namespace) = element.namespace
                        && namespace as usize >= self.strings.len()
                    {
                        return Err(DocumentBuildError::InvalidStringIndex { raw, index: namespace });
                    }
                    if let Some(id) = element.attribute_cache.id
                        && id as usize >= self.strings.len()
                    {
                        return Err(DocumentBuildError::InvalidStringIndex { raw, index: id });
                    }
                    if element.attribute_cache.classes.start > element.attribute_cache.classes.end || element.attribute_cache.classes.end as usize > self.class_list.len() {
                        return Err(DocumentBuildError::InvalidClassRange { raw, start: element.attribute_cache.classes.start, end: element.attribute_cache.classes.end });
                    }
                    for &class in &self.class_list[element.attribute_cache.classes.start as usize..element.attribute_cache.classes.end as usize] {
                        if class as usize >= self.strings.len() {
                            return Err(DocumentBuildError::InvalidStringIndex { raw, index: class });
                        }
                    }
                    for attribute in &element.attributes {
                        if attribute.name as usize >= self.strings.len() {
                            return Err(DocumentBuildError::InvalidAttributeIndex { raw, index: attribute.name });
                        }
                        if attribute.local_name as usize >= self.strings.len() {
                            return Err(DocumentBuildError::InvalidAttributeIndex { raw, index: attribute.local_name });
                        }
                        if let Some(namespace) = attribute.namespace
                            && namespace as usize >= self.strings.len()
                        {
                            return Err(DocumentBuildError::InvalidAttributeIndex { raw, index: namespace });
                        }
                        if attribute.value as usize >= self.strings.len() {
                            return Err(DocumentBuildError::InvalidAttributeIndex { raw, index: attribute.value });
                        }
                    }

                    if let Some(image_idx) = element.image_idx
                        && image_idx as usize >= self.images.len()
                    {
                        return Err(DocumentBuildError::InvalidImageIndex { raw, index: image_idx });
                    }

                    if element.parent.is_none() {
                        roots.push(node_id);
                    }
                    if let Some(parent) = element.parent {
                        let parent_idx = self.node_or_err(parent).ok_or(DocumentBuildError::InvalidParent { parent: parent.raw(), child: raw })?;
                        let parent_node = self.dom_nodes.get(parent_idx).expect("validated parent index in bounds");
                        if !matches!(parent_node, DomNode::Element(_)) {
                            return Err(DocumentBuildError::InvalidParent { parent: parent.raw(), child: raw });
                        }
                    }

                    let mut seen_children = HashSet::new();
                    for &child in &element.children {
                        if self.node_or_err(child).is_none() {
                            return Err(DocumentBuildError::InvalidNodeId { raw: child.raw() });
                        }
                        if !seen_children.insert(child.raw()) {
                            return Err(DocumentBuildError::DuplicateChild { parent: raw, child: child.raw() });
                        }
                        let child_node = self.dom_nodes.get(child.index()).ok_or(DocumentBuildError::InvalidNodeId { raw: child.raw() })?;
                        let child_parent = child_node
                            .parent()
                            .ok_or_else(|| if matches!(child_node, DomNode::Text(_)) { DocumentBuildError::MissingParentForTextNode { raw: child.raw() } } else { DocumentBuildError::InvalidParent { parent: raw, child: child.raw() } })?;
                        if child_parent.raw() != raw {
                            return Err(DocumentBuildError::InvalidParent { parent: raw, child: child.raw() });
                        }
                    }
                }
                DomNode::Text(text) => {
                    let parent = text.parent.ok_or(DocumentBuildError::MissingParentForTextNode { raw })?;
                    let parent_idx = self.node_or_err(parent).ok_or(DocumentBuildError::InvalidParent { parent: parent.raw(), child: raw })?;
                    match self.dom_nodes.get(parent_idx) {
                        Some(DomNode::Element(_)) => {}
                        _ => return Err(DocumentBuildError::InvalidParent { parent: parent.raw(), child: raw }),
                    }
                }
            }
        }

        if roots.is_empty() {
            return Err(DocumentBuildError::MissingRoot);
        }
        if roots.len() != 1 {
            return Err(DocumentBuildError::MultipleRoots);
        }

        let root = roots[0];
        let Some(root_idx) = self.node_or_err(root) else {
            return Err(DocumentBuildError::InvalidNodeId { raw: root.raw() });
        };
        if !matches!(self.dom_nodes.get(root_idx), Some(DomNode::Element(_))) {
            return Err(DocumentBuildError::NonElementRoot { raw: root.raw() });
        }
        if self.dom_root.is_some() && self.dom_root != Some(root) {
            return Err(DocumentBuildError::MultipleRoots);
        }

        let mut visit_state = vec![0u8; self.dom_nodes.len()];
        fn visit(node_idx: usize, token: u64, nodes: &[DomNode], state: &mut [u8]) -> Result<(), DocumentBuildError> {
            if state[node_idx] == 1 {
                return Err(DocumentBuildError::RecursionCycle { raw: node_idx as u32 });
            }
            if state[node_idx] == 2 {
                return Ok(());
            }
            state[node_idx] = 1;

            match &nodes[node_idx] {
                DomNode::Element(element) => {
                    for &child in &element.children {
                        if child.token() != token {
                            return Err(DocumentBuildError::ForeignNodeId { raw: child.raw() });
                        }
                        let child_idx = child.index();
                        visit(child_idx, token, nodes, state)?;
                    }
                }
                DomNode::Text(_) => {}
            }
            state[node_idx] = 2;
            Ok(())
        }
        visit(root_idx, self.lineage_token(), &self.dom_nodes, &mut visit_state)?;

        for (idx, state) in visit_state.iter().copied().enumerate() {
            if state == 0 {
                return Err(DocumentBuildError::UnreachableNodes { raw: idx as u32 });
            }
        }

        for (index, entry) in self.toc_entries.iter().enumerate() {
            if entry.title as usize >= self.strings.len() {
                return Err(DocumentBuildError::ToplevelTableEntries { kind: "toc title", raw: index as u32, index: entry.title });
            }
            if entry.href as usize >= self.strings.len() {
                return Err(DocumentBuildError::ToplevelTableEntries { kind: "toc href", raw: index as u32, index: entry.href });
            }
            if entry.node_idx as usize >= self.dom_nodes.len() {
                return Err(DocumentBuildError::ToplevelTableEntries { kind: "toc node", raw: index as u32, index: entry.node_idx as u16 });
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
        let Some(node) = self.dom_nodes.get(id.index()) else {
            return;
        };
        match node {
            DomNode::Element(elem) => {
                let tag = self.strings.get(elem.tag);
                println!("{}<{}>", indent, tag);
                for &child in &elem.children {
                    self.print_dom_node(child, depth + 1);
                }
            }
            DomNode::Text(text) => {
                let truncated: String = text.text.chars().take(80).collect();
                println!("{}\"{}\"", indent, truncated);
            }
        }
    }

    pub fn element_ref(&self, node_id: DomNodeId) -> Option<ElementRef<'_>> {
        let idx = self.node_or_err(node_id)?;
        self.dom_nodes.get(idx)?.as_element()?;
        Some(ElementRef { doc: self, node_id })
    }

    pub fn text_ref(&self, node_id: DomNodeId) -> Option<TextRef<'_>> {
        let idx = self.node_or_err(node_id)?;
        self.dom_nodes.get(idx)?.as_text()?;
        Some(TextRef { doc: self, node_id })
    }

    pub fn node_ref(&self, node_id: DomNodeId) -> Option<NodeRef<'_>> {
        let idx = self.node_or_err(node_id)?;
        match self.dom_nodes.get(idx)? {
            DomNode::Element(_) => Some(NodeRef::Element(ElementRef { doc: self, node_id })),
            DomNode::Text(_) => Some(NodeRef::Text(TextRef { doc: self, node_id })),
        }
    }

    pub fn node_count(&self) -> usize {
        self.dom_nodes.len()
    }

    pub fn node_ids(&self) -> impl Iterator<Item = DomNodeId> + '_ {
        (0..self.dom_nodes.len()).map(|idx| DomNodeId::new(idx as u32, self.lineage_token()))
    }

    pub fn nodes(&self) -> impl Iterator<Item = (DomNodeId, NodeRef<'_>)> + '_ {
        self.node_ids().filter_map(|node_id| self.node_ref(node_id).map(|node| (node_id, node)))
    }

    pub fn dom_root(&self) -> Option<DomNodeId> {
        self.dom_root
    }

    pub fn dom_id_in_use(&self, id_idx: u16) -> bool {
        self.dom_nodes.iter().any(|node| matches!(node, DomNode::Element(element) if element.attribute_cache.id == Some(id_idx)))
    }

    pub fn is_text_node(&self, node_id: DomNodeId) -> bool {
        matches!(self.node_ref(node_id), Some(NodeRef::Text(_)))
    }

    pub fn is_element_node(&self, node_id: DomNodeId) -> bool {
        matches!(self.node_ref(node_id), Some(NodeRef::Element(_)))
    }

    pub fn get_dom_tag(&self, node_id: DomNodeId) -> Option<&str> {
        self.element_ref(node_id).map(|element| element.tag())
    }

    pub fn get_dom_id(&self, node_id: DomNodeId) -> Option<&str> {
        self.element_ref(node_id).and_then(|element| element.id())
    }

    pub fn get_dom_classes(&self, node_id: DomNodeId) -> Box<dyn Iterator<Item = &str> + '_> {
        if let Some(element) = self.element_ref(node_id) { Box::new(element.classes()) } else { Box::new(std::iter::empty()) }
    }

    pub fn dom_has_class(&self, node_id: DomNodeId, class: &str) -> bool {
        self.element_ref(node_id).is_some_and(|element| element.has_class(class))
    }

    pub fn get_dom_parent(&self, node_id: DomNodeId) -> Option<DomNodeId> {
        self.node_ref(node_id)?.parent()
    }

    pub fn get_dom_namespace(&self, node_id: DomNodeId) -> Option<&str> {
        self.element_ref(node_id).and_then(|element| element.namespace())
    }

    pub fn is_html_element_in_html_document(&self, node_id: DomNodeId) -> bool {
        self.element_ref(node_id).is_some_and(|element| element.is_html_element_in_html_document())
    }

    pub fn dom_ancestors(&self, node_id: DomNodeId) -> DomAncestorIter<'_> {
        let current = self.node_or_err(node_id).and_then(|idx| self.dom_nodes.get(idx).and_then(|node| node.parent()));
        DomAncestorIter { doc: self, current }
    }

    pub fn get_dom_href(&self, node_id: DomNodeId) -> Option<&str> {
        self.element_ref(node_id).and_then(|element| element.href())
    }

    pub fn get_dom_attr(&self, node_id: DomNodeId, attr: &str) -> Option<&str> {
        self.element_ref(node_id).and_then(|element| element.attr(attr))
    }

    pub fn has_dom_attr(&self, node_id: DomNodeId, attr: &str) -> bool {
        self.element_ref(node_id).is_some_and(|element| element.has_attr(attr))
    }

    pub fn get_dom_attr_expanded(&self, node_id: DomNodeId, namespace: Option<&str>, local_name: &str) -> Option<&str> {
        self.element_ref(node_id).and_then(|element| element.attr_expanded(namespace, local_name))
    }

    pub fn has_dom_attr_expanded(&self, node_id: DomNodeId, namespace: Option<&str>, local_name: &str) -> bool {
        self.element_ref(node_id).is_some_and(|element| element.has_attr_expanded(namespace, local_name))
    }

    pub fn get_dom_attr_in_any_namespace(&self, node_id: DomNodeId, local_name: &str) -> Option<&str> {
        self.element_ref(node_id).and_then(|element| element.attr_in_any_namespace(local_name))
    }

    pub fn has_dom_attr_in_any_namespace(&self, node_id: DomNodeId, local_name: &str) -> bool {
        self.element_ref(node_id).is_some_and(|element| element.has_attr_in_any_namespace(local_name))
    }

    pub fn intern_classes(&mut self, classes: Option<&str>) -> Range<u16> {
        let start = self.class_list.len() as u16;
        if let Some(classes) = classes {
            for class in classes.split_whitespace() {
                let idx = self.strings.intern(class);
                self.class_list.push(idx);
            }
        }
        let end = self.class_list.len() as u16;
        start..end
    }
}

fn input_type_supports_required(kind: &str) -> bool {
    matches!(kind.to_ascii_lowercase().as_str(), "text" | "search" | "url" | "tel" | "email" | "password" | "date" | "month" | "week" | "time" | "datetime-local" | "number" | "checkbox" | "radio" | "file")
}

fn input_type_supports_readonly(kind: &str) -> bool {
    matches!(kind.to_ascii_lowercase().as_str(), "text" | "search" | "url" | "tel" | "email" | "password" | "date" | "month" | "week" | "time" | "datetime-local" | "number")
}

fn input_type_supports_placeholder(kind: &str) -> bool {
    matches!(kind.to_ascii_lowercase().as_str(), "text" | "search" | "url" | "tel" | "email" | "password" | "number")
}

fn element_text_is_empty(document: &Document, root: DomNodeId) -> bool {
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        match document.dom_nodes.get(node.index()) {
            Some(DomNode::Text(text)) if !text.text.is_empty() => return false,
            Some(DomNode::Element(element)) => stack.extend(element.children.iter().copied()),
            _ => {}
        }
    }
    true
}

fn radio_group_owner(document: &Document, element: ElementRef<'_>) -> Option<u32> {
    if let Some(form_id) = element.attr("form")
        && let Some(form) = document.node_ids().find(|node| document.element_ref(*node).is_some_and(|candidate| candidate.tag().eq_ignore_ascii_case("form") && candidate.id() == Some(form_id)))
    {
        return Some(form.raw());
    }
    document.dom_ancestors(element.node_id()).find(|ancestor| document.element_ref(*ancestor).is_some_and(|candidate| candidate.namespace() == Some(HTML_NAMESPACE) && candidate.tag().eq_ignore_ascii_case("form"))).map(DomNodeId::raw)
}

impl<'a> ElementRef<'a> {
    fn element(self) -> &'a DomElement {
        self.doc.dom_nodes[self.node_id.index()].as_element().expect("ElementRef must point to a DOM element")
    }

    pub fn node_id(self) -> DomNodeId {
        self.node_id
    }

    pub fn node_idx(self) -> usize {
        self.node_id.index()
    }

    pub fn tag(self) -> &'a str {
        self.doc.strings.get(self.element().tag)
    }

    pub fn id(self) -> Option<&'a str> {
        self.element().attribute_cache.id.map(|idx| self.doc.strings.get(idx))
    }

    pub fn id_idx(self) -> Option<u16> {
        self.element().id_idx()
    }

    pub fn attr_value_idx_no_namespace(self, name_idx: u16) -> Option<u16> {
        self.element().attr_value_idx_no_namespace(name_idx)
    }

    pub fn classes(self) -> impl Iterator<Item = &'a str> + 'a {
        let range = self.element().attribute_cache.classes.clone();
        self.doc.class_list[range.start as usize..range.end as usize].iter().map(|&idx| self.doc.strings.get(idx))
    }

    pub fn attributes(self) -> impl Iterator<Item = AttributeRef<'a>> + 'a {
        self.element().attributes.iter().map(|attribute| AttributeRef { doc: self.doc, attribute })
    }

    pub fn has_class(self, class: &str) -> bool {
        self.classes().any(|candidate| candidate == class)
    }

    pub fn parent(self) -> Option<DomNodeId> {
        self.element().parent
    }

    pub fn children(self) -> impl Iterator<Item = DomNodeId> + 'a {
        self.element().children.iter().copied()
    }

    pub fn namespace(self) -> Option<&'a str> {
        self.element().namespace.map(|idx| self.doc.strings.get(idx))
    }

    pub fn is_html_element_in_html_document(self) -> bool {
        self.doc.mode == DocumentMode::Html && self.namespace().is_some_and(|namespace| namespace == HTML_NAMESPACE)
    }

    pub fn html_state(self) -> HtmlElementState {
        self.element().html_state
    }

    /// Normalized HTML ordered-list start value. XML elements with the same
    /// spelling deliberately have no HTML list semantics.
    pub fn html_list_start(self) -> Option<i64> {
        (self.namespace() == Some(HTML_NAMESPACE) && self.tag().eq_ignore_ascii_case("ol")).then(|| self.attr("start").and_then(parse_html_integer)).flatten()
    }

    pub fn html_list_reversed(self) -> bool {
        self.namespace() == Some(HTML_NAMESPACE) && self.tag().eq_ignore_ascii_case("ol") && self.has_attr("reversed")
    }

    pub fn html_list_item_value(self) -> Option<i64> {
        (self.namespace() == Some(HTML_NAMESPACE) && self.tag().eq_ignore_ascii_case("li")).then(|| self.attr("value").and_then(parse_html_integer)).flatten()
    }

    pub fn href(self) -> Option<&'a str> {
        self.attr("href")
    }

    pub fn attr(self, attr: &str) -> Option<&'a str> {
        let html_name_matching = self.is_html_element_in_html_document();
        self.element()
            .attributes
            .iter()
            .find(|entry| entry.namespace.is_none() && if html_name_matching { self.doc.strings.get(entry.name).eq_ignore_ascii_case(attr) } else { self.doc.strings.get(entry.name) == attr })
            .map(|entry| self.doc.strings.get(entry.value))
    }

    pub fn has_attr(self, attr: &str) -> bool {
        self.attr(attr).is_some()
    }

    pub fn attr_expanded(self, namespace: Option<&str>, local_name: &str) -> Option<&'a str> {
        self.element()
            .attributes
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
        self.element().attributes.iter().find(|entry| self.doc.strings.get(entry.local_name) == local_name).map(|entry| self.doc.strings.get(entry.value))
    }

    pub fn has_attr_in_any_namespace(self, local_name: &str) -> bool {
        self.attr_in_any_namespace(local_name).is_some()
    }

    pub fn image_idx(self) -> Option<u32> {
        self.element().image_idx
    }

    pub fn previous_element_sibling(self) -> Option<ElementRef<'a>> {
        let parent = self.parent().and_then(|parent_idx| self.doc.element_ref(parent_idx))?;
        let mut previous = None;
        for child_idx in parent.children() {
            if child_idx == self.node_id {
                return previous;
            }
            if let Some(child) = self.doc.element_ref(child_idx) {
                previous = Some(child);
            }
        }
        None
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

impl DomElement {
    pub fn id_idx(&self) -> Option<u16> {
        self.attribute_cache.id
    }

    pub fn classes_range(&self) -> Range<u16> {
        self.attribute_cache.classes.clone()
    }

    pub fn attr_value_idx_no_namespace(&self, name_idx: u16) -> Option<u16> {
        self.attributes.iter().find(|entry| entry.namespace.is_none() && entry.name == name_idx).map(|entry| entry.value)
    }

    pub fn image_idx(&self) -> Option<u32> {
        self.image_idx
    }
}

impl<'a> NodeRef<'a> {
    pub fn node_id(self) -> DomNodeId {
        match self {
            NodeRef::Element(element) => element.node_id(),
            NodeRef::Text(text) => text.node_id(),
        }
    }

    pub fn node_idx(self) -> usize {
        self.node_id().index()
    }

    pub fn parent(self) -> Option<DomNodeId> {
        match self {
            NodeRef::Element(element) => element.parent(),
            NodeRef::Text(text) => text.parent(),
        }
    }
}

impl<'a> TextRef<'a> {
    fn text_node(self) -> &'a DomText {
        self.doc.dom_nodes[self.node_id.index()].as_text().expect("TextRef must point to a DOM text node")
    }

    pub fn node_id(self) -> DomNodeId {
        self.node_id
    }

    pub fn node_idx(self) -> usize {
        self.node_id.index()
    }

    pub fn text(self) -> &'a str {
        &self.text_node().text
    }

    pub fn parent(self) -> Option<DomNodeId> {
        self.text_node().parent
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
        self.current = self.doc.dom_nodes.get(id.index()).and_then(|node| node.parent());
        Some(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_element_state_remains_one_machine_word() {
        assert_eq!(std::mem::size_of::<HtmlElementState>(), std::mem::size_of::<u32>());
    }

    #[test]
    fn finish_without_root_allows_empty_document() {
        let builder = DocumentBuilder::new();
        let document = builder.finish_without_root().expect("empty document");
        assert!(document.dom_root().is_none());
        assert_eq!(document.node_count(), 0);
    }

    #[test]
    fn finish_with_explicit_root_populates_root() {
        let mut builder = DocumentBuilder::new();
        let html_tag = builder.intern_string("html");
        let body_tag = builder.intern_string("body");
        let (html, builder) = builder.append_dom_element(None, DomElementData { tag: html_tag, namespace: None, attributes: Vec::new(), attribute_cache: DomElementAttributeCache::new(None, 0..0), image_idx: None }).expect("root append");
        let mut builder = builder.set_dom_root(html).expect("root set");
        let body = builder.append_dom_element(Some(html), DomElementData { tag: body_tag, namespace: None, attributes: Vec::new(), attribute_cache: DomElementAttributeCache::new(None, 0..0), image_idx: None }).expect("body append");
        let html = builder.document().dom_root().expect("rooted builder stores html root");
        let document = builder.finish().expect("document build");
        assert_eq!(document.dom_root(), Some(html));
        let body_ref = document.element_ref(body).expect("body element");
        assert_eq!(body_ref.parent(), Some(html));
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
        let (html, builder) =
            builder.append_dom_element(None, DomElementData { tag: html_tag, namespace: Some(html_namespace), attributes, attribute_cache: DomElementAttributeCache::new(None, 0..0), image_idx: None }).expect("html root append");
        let builder = builder.set_dom_root(html).expect("set html root");
        let document = builder.finish().expect("document build");
        let element = document.element_ref(html).expect("html element");

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
        let (root, builder) = builder.append_dom_element(None, DomElementData { tag: root_tag, namespace: None, attributes, attribute_cache: DomElementAttributeCache::new(None, 0..0), image_idx: None }).expect("xml root append");
        let builder = builder.set_dom_root(root).expect("set xml root");
        let document = builder.finish().expect("document build");
        let element = document.element_ref(root).expect("xml element");

        assert_eq!(element.attr("title"), Some("PASS"));
        assert_eq!(element.attr("Title"), None);
    }

    #[test]
    fn set_dom_root_rejects_non_element_node() {
        let mut builder = DocumentBuilder::new();
        let html_tag = builder.intern_string("html");
        let (html, mut builder) =
            builder.append_dom_element(None, DomElementData { tag: html_tag, namespace: None, attributes: Vec::new(), attribute_cache: DomElementAttributeCache::new(None, 0..0), image_idx: None }).expect("append html");
        let body = builder.append_dom_text(html, "text").expect("append body text");
        if let DomNode::Text(text) = &mut builder.document_mut().dom_nodes[body.index()] {
            text.parent = None;
        }
        assert!(matches!(
            builder.set_dom_root(body),
            Err(DocumentBuildError::NonElementRoot { raw }) if raw == body.raw()
        ));
    }

    #[test]
    fn set_dom_root_rejects_foreign_node_id() {
        let mut local_builder = DocumentBuilder::new();
        let local_root_tag = local_builder.intern_string("html");
        let (_local_root, local_builder) =
            local_builder.append_dom_element(None, DomElementData { tag: local_root_tag, namespace: None, attributes: Vec::new(), attribute_cache: DomElementAttributeCache::new(None, 0..0), image_idx: None }).expect("append local root");

        let mut foreign_builder = DocumentBuilder::new();
        let foreign_root_tag = foreign_builder.intern_string("html");
        let (foreign_root, _foreign_builder) = foreign_builder
            .append_dom_element(None, DomElementData { tag: foreign_root_tag, namespace: None, attributes: Vec::new(), attribute_cache: DomElementAttributeCache::new(None, 0..0), image_idx: None })
            .expect("append foreign root");
        let forged_id = DomNodeId::new(foreign_root.raw(), 0);

        assert!(matches!(local_builder.set_dom_root(forged_id), Err(DocumentBuildError::ForeignNodeId { raw }) if raw == foreign_root.raw()));
    }

    #[test]
    fn stale_node_ids_never_alias_a_later_document() {
        let stale = {
            let mut builder = DocumentBuilder::new();
            let tag = builder.intern_string("old");
            let (node, _builder) = builder.append_dom_element(None, DomElementData { tag, namespace: None, attributes: Vec::new(), attribute_cache: DomElementAttributeCache::new(None, 0..0), image_idx: None }).expect("old root append");
            node
        };

        let mut builder = DocumentBuilder::new();
        let root_tag = builder.intern_string("html");
        let child_tag = builder.intern_string("body");
        let (root, mut builder) =
            builder.append_dom_element(None, DomElementData { tag: root_tag, namespace: None, attributes: Vec::new(), attribute_cache: DomElementAttributeCache::new(None, 0..0), image_idx: None }).expect("new root append");

        assert!(matches!(
            builder.append_dom_element(Some(stale), DomElementData { tag: child_tag, namespace: None, attributes: Vec::new(), attribute_cache: DomElementAttributeCache::new(None, 0..0), image_idx: None },),
            Err(DocumentBuildError::ForeignNodeId { .. })
        ));
        assert_ne!(root, stale);
    }

    #[test]
    fn finish_rejects_invalid_string_and_image_indices() {
        let builder = DocumentBuilder::new();
        let (root, builder) = builder
            .append_dom_element(None, DomElementData { tag: u16::MAX, namespace: None, attributes: Vec::new(), attribute_cache: DomElementAttributeCache::new(None, 0..0), image_idx: None })
            .expect("invalid indices are accepted only in the in-progress builder");
        let builder = builder.set_dom_root(root).expect("root identity is valid");
        assert!(matches!(builder.finish(), Err(DocumentBuildError::InvalidStringIndex { .. })));

        let mut builder = DocumentBuilder::new();
        let tag = builder.intern_string("html");
        let (root, builder) = builder
            .append_dom_element(None, DomElementData { tag, namespace: None, attributes: Vec::new(), attribute_cache: DomElementAttributeCache::new(None, 0..0), image_idx: Some(0) })
            .expect("invalid image references are checked at finalization");
        let builder = builder.set_dom_root(root).expect("root identity is valid");
        assert!(matches!(builder.finish(), Err(DocumentBuildError::InvalidImageIndex { index: 0, .. })));
    }

    #[test]
    fn finish_rejects_invalid_toc_indices_and_missing_text_parent() {
        let mut builder = DocumentBuilder::new();
        let tag = builder.intern_string("html");
        let (root, builder) = builder.append_dom_element(None, DomElementData { tag, namespace: None, attributes: Vec::new(), attribute_cache: DomElementAttributeCache::new(None, 0..0), image_idx: None }).expect("root append");
        let mut builder = builder.set_dom_root(root).expect("root set");
        builder.push_toc_entry(crate::DocumentTocEntry { level: 1, title: u16::MAX, href: u16::MAX, node_idx: root.raw() });
        assert!(matches!(builder.finish(), Err(DocumentBuildError::ToplevelTableEntries { .. })));

        let mut builder = DocumentBuilder::new();
        let tag = builder.intern_string("html");
        let (root, mut builder) = builder.append_dom_element(None, DomElementData { tag, namespace: None, attributes: Vec::new(), attribute_cache: DomElementAttributeCache::new(None, 0..0), image_idx: None }).expect("root append");
        let text = builder.append_dom_text(root, "text").expect("text append");
        let mut builder = builder.set_dom_root(root).expect("root set");
        if let Some(DomNode::Text(text_node)) = builder.document_mut().dom_nodes.get_mut(text.index()) {
            text_node.parent = None;
        }
        assert!(matches!(builder.finish(), Err(DocumentBuildError::MissingParentForTextNode { .. })));
    }

    #[test]
    fn finish_rejects_multiple_toplevel_roots() {
        let mut builder = DocumentBuilder::new();
        let html_tag = builder.intern_string("html");
        let section_tag = builder.intern_string("section");
        let (html, builder) = builder.append_dom_element(None, DomElementData { tag: html_tag, namespace: None, attributes: Vec::new(), attribute_cache: DomElementAttributeCache::new(None, 0..0), image_idx: None }).expect("append root");
        let mut builder = builder.set_dom_root(html).expect("rooted");
        let _second_root =
            builder.append_dom_element(None, DomElementData { tag: section_tag, namespace: None, attributes: Vec::new(), attribute_cache: DomElementAttributeCache::new(None, 0..0), image_idx: None }).expect("append second root");

        assert!(matches!(builder.finish(), Err(DocumentBuildError::MultipleRoots)));
    }

    #[test]
    fn finish_rejects_recursion_cycle() {
        let mut builder = DocumentBuilder::new();
        let html_tag = builder.intern_string("html");
        let section_tag = builder.intern_string("section");
        let (html, mut builder) =
            builder.append_dom_element(None, DomElementData { tag: html_tag, namespace: None, attributes: Vec::new(), attribute_cache: DomElementAttributeCache::new(None, 0..0), image_idx: None }).expect("append root");
        let section = builder.append_dom_element(Some(html), DomElementData { tag: section_tag, namespace: None, attributes: Vec::new(), attribute_cache: DomElementAttributeCache::new(None, 0..0), image_idx: None }).expect("append child");
        let mut builder = builder.set_dom_root(html).expect("rooted");

        let root = html.raw() as usize;
        let child = section.raw() as usize;
        let lineage_token = builder.document().lineage_token();
        if let Some(DomNode::Element(element)) = &mut builder.document_mut().dom_nodes.get_mut(child) {
            element.children.push(DomNodeId::new(root as u32, lineage_token));
        }
        assert!(matches!(builder.finish(), Err(DocumentBuildError::RecursionCycle { raw: _ } | DocumentBuildError::InvalidParent { .. })));
    }
}
