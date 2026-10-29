//! Parse and cache inline declarations before document resolution.
use html_dom::{Document, DomNodeId};
use lightningcss::stylesheet::{ParserOptions, StyleAttribute};
use rustc_data_structures::fx::FxHashMap;
use static_self::IntoOwned;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

struct CachedInlineStyle {
    css: String,
    style: Option<Arc<StyleAttribute<'static>>>,
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
        Self { entries: VecDeque::new(), capacity: 256, hits: 0, misses: 0, revision: 0 }
    }
}

impl ParsedInlineStyleCache {
    pub(crate) fn stats(&self) -> (usize, u64, u64) {
        (self.entries.len(), self.hits, self.misses)
    }

    fn lookup(&mut self, css: &str) -> Option<Option<Arc<StyleAttribute<'static>>>> {
        let position = self.entries.iter().position(|entry| entry.css == css)?;
        if position + 1 != self.entries.len() {
            let entry = self.entries.remove(position).expect("located inline style exists");
            self.entries.push_back(entry);
        }
        Some(self.entries.back().expect("cached inline style exists").style.clone())
    }

    /// Resolve the document's distinct inline sources with at most two locks.
    /// Parsing and disposal of redundant concurrent results happen outside
    /// the shared LRU's critical section.
    pub(crate) fn get_or_parse_batch(shared: &Mutex<Self>, sources: &[&str]) -> Vec<Option<Arc<StyleAttribute<'static>>>> {
        let mut styles = vec![None; sources.len()];
        if sources.is_empty() { return styles; }
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
        if misses.is_empty() { return styles; }
        for &index in &misses {
            let source = sources[index];
            styles[index] = StyleAttribute::parse(source, ParserOptions { error_recovery: true, ..ParserOptions::default() }).ok().map(|style| Arc::new(style.into_owned()));
        }
        let mut discarded = Vec::new();
        let mut cache = shared.lock().expect("inline style cache lock");
        let raced = cache.revision != revision;
        for index in misses {
            if raced && let Some(cached) = cache.lookup(sources[index]) {
                discarded.push(std::mem::replace(&mut styles[index], cached));
                continue;
            }
            if cache.entries.len() == cache.capacity { cache.entries.pop_front(); }
            cache.entries.push_back(CachedInlineStyle { css: sources[index].to_owned(), style: styles[index].clone() });
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
    styles: Vec<Option<Arc<StyleAttribute<'static>>>>,
}

impl InlineStyles {
    pub(crate) fn parse(doc: &Document, shared: &Arc<Mutex<ParsedInlineStyleCache>>) -> Self {
        let mut node_style_ids = vec![None; doc.node_count()];
        let mut sources = Vec::new();
        let mut parsed = FxHashMap::<&str, u32>::default();
        for node_idx in doc.node_ids() {
            let Some(source) = doc
                .get_dom_attr(node_idx, "style")
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

    pub(crate) fn get(&self, node_idx: DomNodeId) -> Option<&StyleAttribute<'static>> {
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
        let cache = Mutex::new(ParsedInlineStyleCache { capacity: 2, ..Default::default() });
        let first = ParsedInlineStyleCache::get_or_parse_batch(&cache, &["color: red", "color: blue"]);
        let second = ParsedInlineStyleCache::get_or_parse_batch(&cache, &["color: green", "color: red", "color: blue"]);
        assert!(Arc::ptr_eq(first[0].as_ref().unwrap(), second[1].as_ref().unwrap()));
        assert!(Arc::ptr_eq(first[1].as_ref().unwrap(), second[2].as_ref().unwrap()));
        let cache = cache.lock().unwrap();
        assert_eq!(cache.entries.len(), 2);
        assert_eq!(cache.hits, 2);
        assert_eq!(cache.misses, 3);
    }
}
