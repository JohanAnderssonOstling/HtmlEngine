//! Parse and cache inline declarations before document resolution.
use crate::style::cascade::resolver::prepared_native::PreparedNativeValue;
use crate::style::property_catalogue::PropertyKey;
use crate::style::rules::prepared::{PreparedDeclarationBlock, prepare_constant_value};
use crate::style::syntax::prepared_renderer::PreparedRendererValue;
use html_dom::{Document, DomNodeId};
use lightningcss::properties::Property;
use lightningcss::stylesheet::{ParserOptions, StyleAttribute};
use rustc_data_structures::fx::FxHashMap;
use static_self::IntoOwned;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// Borrow the matching entry after promoting it to the most recently used end.
pub(crate) fn cached_entry<T>(
    entries: &mut VecDeque<T>,
    matches: impl FnMut(&T) -> bool,
) -> Option<&T> {
    let index = entries.iter().position(matches)?;
    if index + 1 != entries.len() {
        let entry = entries.remove(index)?;
        entries.push_back(entry);
    }
    entries.back()
}

pub(crate) struct PreparedInlineStyle<'css> {
    pub(crate) style: StyleAttribute<'css>,
    pub(crate) prepared: PreparedDeclarationBlock<'css>,
    important_start: usize,
}

impl<'css> From<StyleAttribute<'css>> for PreparedInlineStyle<'css> {
    fn from(mut style: StyleAttribute<'css>) -> Self {
        for property in style
            .declarations
            .declarations
            .iter_mut()
            .chain(&mut style.declarations.important_declarations)
        {
            prepare_constant_value(property);
        }
        let mut prepared = PreparedDeclarationBlock::default();
        prepared.append(&style.declarations.declarations);
        let (important_start, _) = prepared.append(&style.declarations.important_declarations);
        Self {
            style,
            prepared,
            important_start: important_start as usize,
        }
    }
}

impl<'css> PreparedInlineStyle<'css> {
    pub(crate) fn native_values(
        &self,
        important: bool,
        index: usize,
    ) -> &[Option<PreparedNativeValue>] {
        self.prepared
            .native_values(index + if important { self.important_start } else { 0 })
    }

    pub(crate) fn renderer_value(
        &self,
        important: bool,
        index: usize,
    ) -> Option<&PreparedRendererValue> {
        self.prepared
            .renderer_value(index + if important { self.important_start } else { 0 })
    }

    pub(crate) fn requires_viewport_height(&self, important: bool, index: usize) -> bool {
        self.prepared.requires_viewport_height(if important {
            self.declarations.declarations.len() + index
        } else {
            index
        })
    }

    pub(crate) fn metadata(&self, important: bool, index: usize) -> (&[PropertyKey], bool) {
        self.prepared
            .metadata(index + if important { self.important_start } else { 0 })
    }

    pub(crate) fn longhands(&self, important: bool, index: usize) -> Option<&[Property<'css>]> {
        self.prepared
            .longhands(index + if important { self.important_start } else { 0 })
    }
}

impl<'css> std::ops::Deref for PreparedInlineStyle<'css> {
    type Target = StyleAttribute<'css>;
    fn deref(&self) -> &Self::Target {
        &self.style
    }
}

struct CachedInlineStyle {
    css: String,
    style: Option<Arc<PreparedInlineStyle<'static>>>,
}

pub(crate) struct ParsedInlineStyleCache {
    entries: VecDeque<CachedInlineStyle>,
    capacity: usize,
    hits: u64,
    misses: u64,
    revision: u64,
}

impl Default for ParsedInlineStyleCache {
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
            capacity: 256,
            hits: 0,
            misses: 0,
            revision: 0,
        }
    }
}

impl ParsedInlineStyleCache {
    pub(crate) fn stats(&self) -> (usize, u64, u64) {
        (self.entries.len(), self.hits, self.misses)
    }

    fn lookup(&mut self, css: &str) -> Option<Option<Arc<PreparedInlineStyle<'static>>>> {
        cached_entry(&mut self.entries, |entry| entry.css == css).map(|entry| entry.style.clone())
    }

    /// Resolve the document's distinct inline sources with at most two locks.
    /// Parsing and disposal of redundant concurrent results happen outside
    /// the shared LRU's critical section.
    pub(crate) fn get_or_parse_batch(
        shared: &Mutex<Self>,
        sources: &[&str],
    ) -> Vec<Option<Arc<PreparedInlineStyle<'static>>>> {
        let mut styles = vec![None; sources.len()];
        if sources.is_empty() {
            return styles;
        }
        let mut misses = Vec::new();
        let revision = {
            let mut cache = shared.lock().expect("inline style cache lock");
            for (index, source) in sources.iter().enumerate() {
                if let Some(style) = cache.lookup(source) {
                    cache.hits += 1;
                    styles[index] = style;
                } else {
                    cache.misses += 1;
                    misses.push(index);
                }
            }
            cache.revision
        };
        if misses.is_empty() {
            return styles;
        }
        for &index in &misses {
            let source = sources[index];
            styles[index] = StyleAttribute::parse(
                source,
                ParserOptions {
                    error_recovery: true,
                    ..ParserOptions::default()
                },
            )
            .ok()
            .map(|style| Arc::new(PreparedInlineStyle::from(style.into_owned())));
        }
        let mut discarded = Vec::new();
        let mut cache = shared.lock().expect("inline style cache lock");
        let raced = cache.revision != revision;
        for index in misses {
            if raced && let Some(cached) = cache.lookup(sources[index]) {
                discarded.push(std::mem::replace(&mut styles[index], cached));
                continue;
            }
            if cache.entries.len() == cache.capacity {
                cache.entries.pop_front();
            }
            cache.entries.push_back(CachedInlineStyle {
                css: sources[index].to_owned(),
                style: styles[index].clone(),
            });
            cache.revision = cache.revision.wrapping_add(1);
        }
        drop(cache);
        drop(discarded);
        styles
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.revision = self.revision.wrapping_add(1);
        self.hits = 0;
        self.misses = 0;
    }
}

pub(crate) struct InlineStyles {
    node_style_ids: Vec<Option<u32>>,
    styles: Vec<Option<Arc<PreparedInlineStyle<'static>>>>,
}

impl InlineStyles {
    pub(crate) fn parse(doc: &Document, shared: &Arc<Mutex<ParsedInlineStyleCache>>) -> Self {
        let mut node_style_ids = vec![None; doc.element_count()];
        let mut sources = Vec::new();
        let mut parsed = FxHashMap::<&str, u32>::default();
        for node_idx in doc.element_ids() {
            let Some(source) = doc
                .element_ref(node_idx)
                .attr("style")
                .filter(|source| !source.trim().is_empty())
            else {
                continue;
            };
            let style_id = *parsed.entry(source).or_insert_with(|| {
                let id = u32::try_from(sources.len()).expect("inline style count fits in u32");
                sources.push(source);
                id
            });
            node_style_ids[node_idx.index()] = Some(style_id);
        }
        let styles = ParsedInlineStyleCache::get_or_parse_batch(shared, &sources);
        Self {
            node_style_ids,
            styles,
        }
    }

    pub(crate) fn get(&self, node_idx: DomNodeId) -> Option<&PreparedInlineStyle<'static>> {
        if node_idx.is_text() {
            return None;
        }
        let id = self
            .node_style_ids
            .get(node_idx.index())
            .copied()
            .flatten()?;
        self.styles.get(id as usize)?.as_deref()
    }
}

#[cfg(test)]
mod inline_cache_tests {
    use super::*;
    use std::sync::Barrier;

    #[test]
    fn simultaneous_inline_cache_misses_publish_one_shared_style() {
        let cache = Mutex::new(ParsedInlineStyleCache::default());
        let ready = Barrier::new(4);
        let styles = std::thread::scope(|scope| {
            let jobs: Vec<_> = (0..4)
                .map(|_| {
                    scope.spawn(|| {
                        ready.wait();
                        ParsedInlineStyleCache::get_or_parse_batch(
                            &cache,
                            &["color: red; padding: 1px"],
                        )
                        .pop()
                        .flatten()
                        .unwrap()
                    })
                })
                .collect();
            jobs.into_iter()
                .map(|job| job.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert!(styles.iter().all(|style| Arc::ptr_eq(style, &styles[0])));
        let cache = cache.lock().unwrap();
        assert_eq!(cache.entries.len(), 1);
        assert_eq!(cache.hits + cache.misses, 4);
    }

    #[test]
    fn a_batch_pins_hits_before_new_sources_evict_them() {
        let cache = Mutex::new(ParsedInlineStyleCache {
            capacity: 2,
            ..Default::default()
        });
        let first =
            ParsedInlineStyleCache::get_or_parse_batch(&cache, &["color: red", "color: blue"]);
        let second = ParsedInlineStyleCache::get_or_parse_batch(
            &cache,
            &["color: green", "color: red", "color: blue"],
        );
        assert!(Arc::ptr_eq(
            first[0].as_ref().unwrap(),
            second[1].as_ref().unwrap()
        ));
        assert!(Arc::ptr_eq(
            first[1].as_ref().unwrap(),
            second[2].as_ref().unwrap()
        ));
        let cache = cache.lock().unwrap();
        assert_eq!(cache.entries.len(), 2);
        assert_eq!(cache.hits, 2);
        assert_eq!(cache.misses, 3);
    }
}
