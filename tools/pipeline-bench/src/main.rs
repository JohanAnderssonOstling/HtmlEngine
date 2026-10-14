use html::layout::{
    FontSlant, GlyphId, GlyphMetric, GlyphResourceStore, GlyphShaper, ShapeError, ShapedTextRun,
    TextRunShapeRequest,
};
use html::pipeline::*;
use html::resources::FileSystemProvider;
use serde_json::json;
use std::collections::HashMap;
use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;
fn layout_fingerprint(document: &html::layout::LaidOutDocument) -> u64 {
    let view = document.render_view();
    let text = view.text();
    let mut hash = 0xcbf29ce484222325u64;
    let mut mix = |value: u64| {
        hash = (hash ^ value).wrapping_mul(0x100000001b3);
    };
    let bits = |value: f64| if value == 0.0 { 0 } else { value.to_bits() };
    mix(text.glyph_count() as u64);
    mix(text.line_count() as u64);
    for index in 0..text.glyph_count() {
        mix(text.glyph_at(index).unwrap_or_default() as u64);
        mix(text
            .line_index_for_glyph(index as u32)
            .unwrap_or(usize::MAX) as u64);
        mix(text
            .character_advance(index as u32)
            .unwrap_or_default()
            .to_bits() as u64);
    }
    for line in text.lines().iter() {
        mix(line.start() as u64);
        mix(line.end() as u64);
        for value in [
            line.point().x,
            line.point().y,
            line.height(),
            line.baseline(),
            line.word_spacing(),
            line.letter_spacing(),
            line.optical_offset_x(),
        ] {
            mix(bits(value));
        }
        mix(line.paint_color().is_some() as u64);
        mix(line.paint_color().unwrap_or_default() as u64);
        if let Some(clip) = text.line_overflow_clip(line.index()) {
            mix(1);
            let rect = clip.rect();
            for value in [rect.x0, rect.y0, rect.x1, rect.y1] {
                mix(bits(value));
            }
            mix(clip.clips_x() as u64);
            mix(clip.clips_y() as u64);
        } else {
            mix(0);
        }
        mix(line.is_in_positioned_layer() as u64);
        mix(line.is_in_negative_positioned_layer() as u64);
        for run in text
            .line_glyph_offsets(line.index())
            .into_iter()
            .flat_map(|runs| runs.iter().collect::<Vec<_>>())
        {
            mix(run.range().start as u64);
            mix(run.range().end as u64);
            mix(bits(run.offset()));
        }
        for run in text
            .line_glyph_advances(line.index())
            .into_iter()
            .flat_map(|runs| runs.iter().collect::<Vec<_>>())
        {
            mix(run.range().start as u64);
            mix(run.range().end as u64);
            mix(bits(run.advance()));
        }
        for fragment in text.line_text_fragments(line.index()).into_iter().flatten() {
            mix(fragment.glyphs().start as u64);
            mix(fragment.glyphs().end as u64);
            mix(bits(fragment.offset_x()));
            mix(fragment.paint_order() as u64);
            mix(fragment.visible() as u64);
        }
        for image in view.fragments().images_for_line(line.index()).iter() {
            mix(image.image_idx() as u64);
            mix(image.line_idx() as u64);
        }
        for decoration in view.fragments().decorations_for_line(line.index()).iter() {
            let rect = decoration.rect();
            for value in [rect.x0, rect.y0, rect.x1, rect.y1] {
                mix(bits(value));
            }
            mix(decoration.color() as u64);
        }
    }
    for decoration in view.fragments().decorations().iter() {
        let rect = decoration.rect();
        for value in [rect.x0, rect.y0, rect.x1, rect.y1] {
            mix(bits(value));
        }
        mix(decoration.color() as u64);
        mix(decoration.is_inline() as u64);
        if let Some(clip) = decoration.overflow_clip() {
            mix(1);
            let rect = clip.rect();
            for value in [rect.x0, rect.y0, rect.x1, rect.y1] {
                mix(bits(value));
            }
            mix(clip.clips_x() as u64);
            mix(clip.clips_y() as u64);
        } else {
            mix(0);
        }
    }
    for image in view.fragments().images().iter() {
        mix(image.image_idx() as u64);
        mix(image.line_idx() as u64);
        let p = image.offset();
        let size = image.size();
        let rect = image.clip();
        for v in [
            p.x,
            p.y,
            size.width,
            size.height,
            rect.x0,
            rect.y0,
            rect.x1,
            rect.y1,
        ] {
            mix(bits(v));
        }
    }
    hash
}
#[derive(Default)]
struct Shaper {
    resources: GlyphResourceStore,
    cache: HashMap<(char, u32, u32), GlyphId>,
    calls: usize,
}
impl GlyphShaper for Shaper {
    fn reset(&mut self) {
        self.resources.clear();
        self.cache.clear();
    }
    fn glyph_resources(&mut self) -> &mut GlyphResourceStore {
        &mut self.resources
    }
    fn shape_glyph(
        &mut self,
        ch: char,
        size: f32,
        _: u16,
        _: FontSlant,
        color: u32,
        _: Option<&str>,
    ) -> Result<GlyphId, ShapeError> {
        self.calls += 1;
        let key = (ch, size.to_bits(), color);
        if let Some(&glyph) = self.cache.get(&key) {
            return Ok(glyph);
        }
        let glyph = self.resources.register(
            GlyphMetric::try_new(ch, size * 0.5, size * 0.75, size * 0.25, 0.0).unwrap(),
        )?;
        self.cache.insert(key, glyph);
        Ok(glyph)
    }
    fn shape_glyph_run(
        &mut self,
        request: TextRunShapeRequest<'_>,
        glyphs: &mut [GlyphId],
    ) -> Result<Option<ShapedTextRun>, ShapeError> {
        let style = request.style();
        for (index, (glyph, ch)) in glyphs.iter_mut().zip(request.text().chars()).enumerate() {
            *glyph = self.shape_glyph(
                ch,
                style.font_size(),
                style.font_weight(),
                style.font_slant(),
                request
                    .paint_colors()
                    .map_or(style.color(), |colors| colors[index]),
                style.font_family(),
            )?;
        }
        Ok(None)
    }
}

fn input(source: String) -> PipelineInputs {
    PipelineInputs {
        source: source.into(),
        markup_syntax: MarkupSyntax::Html,
        user_styles: vec![],
        reader_overrides: ReaderStyleOverrides {
            canonical_font_size: Some(16.0),
            link_foreground: Some(0x466853ff),
            ..Default::default()
        },
        note_flow: Default::default(),
        source_revision: SourceRevision::INITIAL,
        base_uri: "index.html".into(),
        resource_revision: ResourceRevision::INITIAL,
        stylesheet_revision: StylesheetRevision::INITIAL,
        style_environment: Default::default(),
        font_environment: FontEnvironmentRevision::INITIAL,
        image_metrics_revision: ImageMetricsRevision::INITIAL,
        layout: LayoutConstraints {
            viewport_width: 600.0,
            viewport_height: None,
            line_height: 16.0,
            image_sizing_policy: Default::default(),
            text_composition_policy: Default::default(),
        },
        image_metrics: Default::default(),
        paint: PaintSettingsRevision::INITIAL,
    }
}

#[cfg(feature = "heap")]
mod heap {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
    pub static LIVE: AtomicUsize = AtomicUsize::new(0);
    pub static PEAK: AtomicUsize = AtomicUsize::new(0);
    pub static ALLOCS: AtomicUsize = AtomicUsize::new(0);
    pub static BYTES: AtomicUsize = AtomicUsize::new(0);
    pub struct Tracked;
    fn allocated(size: usize) {
        let live = LIVE.fetch_add(size, Relaxed) + size;
        PEAK.fetch_max(live, Relaxed);
        ALLOCS.fetch_add(1, Relaxed);
        BYTES.fetch_add(size, Relaxed);
    }
    unsafe impl GlobalAlloc for Tracked {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let result = unsafe { System.alloc(layout) };
            if !result.is_null() {
                allocated(layout.size());
            }
            result
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            let result = unsafe { System.alloc_zeroed(layout) };
            if !result.is_null() {
                allocated(layout.size());
            }
            result
        }
        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            LIVE.fetch_sub(layout.size(), Relaxed);
            unsafe { System.dealloc(pointer, layout) };
        }
        unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            let result = unsafe { System.realloc(pointer, layout, size) };
            if !result.is_null() {
                LIVE.fetch_sub(layout.size(), Relaxed);
                allocated(size);
            }
            result
        }
    }
    #[global_allocator]
    static ALLOCATOR: Tracked = Tracked;
    pub fn reset() {
        PEAK.store(LIVE.load(Relaxed), Relaxed);
        ALLOCS.store(0, Relaxed);
        BYTES.store(0, Relaxed);
    }
    pub fn sample() -> (usize, usize, usize, usize) {
        (
            LIVE.load(Relaxed),
            PEAK.load(Relaxed),
            ALLOCS.load(Relaxed),
            BYTES.load(Relaxed),
        )
    }
}
#[cfg(not(feature = "heap"))]
mod heap {
    pub fn reset() {}
    pub fn sample() -> (usize, usize, usize, usize) {
        (0, 0, 0, 0)
    }
}

fn cpu_seconds() -> f64 {
    let mut time = std::mem::MaybeUninit::<libc::timespec>::uninit();
    assert_eq!(
        unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, time.as_mut_ptr()) },
        0
    );
    let time = unsafe { time.assume_init() };
    time.tv_sec as f64 + time.tv_nsec as f64 * 1e-9
}
fn peak_rss() -> usize {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    assert_eq!(
        unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) },
        0
    );
    unsafe { usage.assume_init() }.ru_maxrss as usize * 1024
}
fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}
fn source_fingerprint(doc: &html::layout::LaidOutDocument) -> u64 {
    let addressing = doc.render_view().addressing();
    let mut hash = 0xcbf29ce484222325u64;
    let mut mix = |value: u64| {
        hash = (hash ^ value).wrapping_mul(0x100000001b3);
    };
    for run in doc.render_view().text().text_runs() {
        let range = run.glyphs();
        if range.is_empty() {
            continue;
        }
        for glyph in [range.start, (range.start + range.end) / 2, range.end - 1] {
            for after in [false, true] {
                mix(glyph as u64);
                mix(after as u64);
                if let Some(position) = addressing.source_position_for_glyph(glyph, after) {
                    mix(1);
                    mix(position.text_step() as u64);
                    mix(position.utf16_offset() as u64);
                    for element in position.elements() {
                        mix(element.step() as u64);
                    }
                } else {
                    mix(0);
                }
            }
        }
    }
    hash
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let path = &args[1];
    let case = &args[2];
    let count: usize = args.get(3).map_or(11, |n| n.parse().unwrap());
    let mut inputs = input(std::fs::read_to_string(path).unwrap());
    inputs.base_uri = path.clone();
    inputs.markup_syntax = MarkupSyntax::from_uri(path);
    let mut session = PipelineSession::new(Arc::new(FileSystemProvider));
    let mut shaper = Shaper::default();
    if case != "cold" {
        session.update(inputs.clone(), &mut shaper).unwrap();
    }
    let mut view = if case == "handoff" {
        session.document().cloned()
    } else {
        None
    };
    black_box(&view);
    let mut walls = Vec::with_capacity(count);
    let mut cpus = Vec::with_capacity(count);
    let mut retained = Vec::with_capacity(count);
    let mut peaks = Vec::with_capacity(count);
    let mut allocations = Vec::with_capacity(count);
    let mut allocated_bytes = Vec::with_capacity(count);
    for iteration in 0..count + 3 {
        match case.as_str() {
            "cold" => {
                session = PipelineSession::new(Arc::new(FileSystemProvider));
                shaper = Shaper::default();
            }
            "resize" | "handoff" => {
                inputs.layout.viewport_width = if iteration % 2 == 0 { 620.0 } else { 600.0 }
            }
            "paint" => {
                inputs.reader_overrides.link_foreground = Some(if iteration % 2 == 0 {
                    0x3388ccff
                } else {
                    0x466853ff
                })
            }
            "font" => inputs.font_environment = inputs.font_environment.next(),
            _ => panic!("unknown case"),
        }
        heap::reset();
        let cpu = cpu_seconds();
        let wall = Instant::now();
        black_box(session.update(inputs.clone(), &mut shaper).unwrap());
        if case == "handoff" {
            view = session.document().cloned();
            black_box(&view);
        }
        let elapsed = wall.elapsed().as_secs_f64() * 1000.0;
        let consumed = (cpu_seconds() - cpu) * 1000.0;
        let (live, peak, allocs, bytes) = heap::sample();
        if iteration >= 3 {
            walls.push(elapsed);
            cpus.push(consumed);
            retained.push(live as f64);
            peaks.push(peak as f64);
            allocations.push(allocs as f64);
            allocated_bytes.push(bytes as f64);
        }
    }
    let doc = session.document().unwrap();
    let reported = doc.memory_usage_bytes();
    let rss = peak_rss();
    let output = json!({"case":case,"chapter":path.rsplit('/').next().unwrap(),"samples":count,"font_backend":"deterministic mock","heap_instrumented":cfg!(feature="heap"),"wall_median_ms":median(&mut walls),"cpu_median_ms":median(&mut cpus),"heap_live_bytes":median(&mut retained) as usize,"heap_peak_bytes":median(&mut peaks) as usize,"allocation_requests":median(&mut allocations) as usize,"allocated_bytes":median(&mut allocated_bytes) as usize,"rss_peak_bytes":rss,"reported_document_bytes":reported,"glyphs":doc.render_view().text().glyph_count(),"lines":doc.render_view().text().line_count(),"render_fingerprint":format!("{:016x}",layout_fingerprint(doc)),"source_fingerprint":format!("{:016x}",source_fingerprint(doc))});
    println!("{output}");
}
