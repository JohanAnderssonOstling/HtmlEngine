use html::pipeline::ResourceContext;
use html::text::{GlyphRequest, ShapedGlyph};
mod access;
mod book_css;
mod counter;
mod geometry;
mod opening;
#[cfg(feature = "profiling")]
mod opening_deep;
mod queries;
use html::layout::{
    GlyphId, GlyphMetric, GlyphResourceGeneration, GlyphResourceStore, GlyphShaper, ShapeError,
    ShapedTextRun, TextRunShapeRequest,
};
use html::pipeline::*;
use html::resources::FileSystemProvider;
use serde_json::json;
use std::collections::{BTreeMap, HashMap};
use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;
fn layout_fingerprint(
    document: &html::layout::LaidOutDocument,
    resources: &GlyphResourceStore,
) -> u64 {
    let view = document.render_view(resources);
    let text = view.text();
    let mut hash = 0xcbf29ce484222325u64;
    let mut mix = |value: u64| {
        hash = (hash ^ value).wrapping_mul(0x100000001b3);
    };
    let bits = |value: f64| if value == 0.0 { 0 } else { value.to_bits() };
    mix(text.glyph_count() as u64);
    mix(text.line_count() as u64);
    mix(view.canvas_background_color().unwrap_or(0) as u64);
    for index in 0..text.glyph_count() {
        mix(text
            .glyph_at(index)
            .and_then(|glyph| text.glyph_metric(glyph))
            .map_or(0, |metric| metric.ch() as u64));
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
        mix(decoration.is_background() as u64);
        mix(decoration.is_border() as u64);
        mix(decoration.is_foreground() as u64);
        mix(decoration.pattern() as u64);
        mix(decoration.is_in_positioned_layer() as u64);
        mix(decoration.is_in_negative_positioned_layer() as u64);
        mix(decoration.is_in_independent_positioned_layer() as u64);
        mix(decoration.line_idx().unwrap_or(usize::MAX) as u64);
        mix(decoration.paint_order() as u64);
        mix(decoration.border_width().map_or(0, |width| width.to_bits()) as u64);
        if let Some(radii) = decoration.radii() {
            mix(1);
            for (x, y) in [
                radii.top_left,
                radii.top_right,
                radii.bottom_right,
                radii.bottom_left,
            ] {
                mix(x.to_bits() as u64);
                mix(y.to_bits() as u64);
            }
        } else {
            mix(0);
        }

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
    register_runs: bool,
    append_checkpoint: Option<usize>,
    generation: GlyphResourceGeneration,
    cache: HashMap<(char, u32, u32), GlyphId>,
    calls: usize,
}
impl GlyphShaper for Shaper {
    fn begin_append_shaping(&mut self) -> Result<(), ShapeError> {
        assert!(self.append_checkpoint.is_none());
        self.append_checkpoint = Some(self.cache.len());
        Ok(())
    }
    fn commit_append_shaping(&mut self) {
        self.append_checkpoint = None;
    }
    fn rollback_append_shaping(&mut self) {
        if let Some(checkpoint) = self.append_checkpoint.take() {
            self.cache.retain(|_, glyph| (*glyph as usize) < checkpoint);
        }
    }
    fn reset(&mut self) {
        self.generation = GlyphResourceGeneration::default();
        self.cache.clear();
    }
    fn resource_generation(&self) -> GlyphResourceGeneration {
        self.generation
    }
    fn shape_glyph(&mut self, request: GlyphRequest<'_>) -> Result<ShapedGlyph, ShapeError> {
        self.calls += 1;
        let key = (
            request.character,
            request.font_size.to_bits(),
            request.color,
        );
        let metric = GlyphMetric::try_new(
            request.character,
            request.font_size * 0.5,
            request.font_size * 0.75,
            request.font_size * 0.25,
        )
        .unwrap();
        if let Some(&glyph) = self.cache.get(&key) {
            return Ok(ShapedGlyph::from_metric(glyph, metric));
        }
        let glyph = self.cache.len() as GlyphId;
        self.cache.insert(key, glyph);
        Ok(ShapedGlyph::from_metric(glyph, metric))
    }
    fn shape_glyph_run(
        &mut self,
        request: TextRunShapeRequest<'_>,
        glyphs: &mut [GlyphId],
        resources: &mut GlyphResourceStore,
    ) -> Result<Option<ShapedTextRun>, ShapeError> {
        #[cfg(feature = "profiling")]
        let _profile_scope = html::layout::profiling::scope("shape.mock_backend");
        let style = request.style();
        if self.register_runs {
            let count = glyphs.len();
            let geometry = ShapedTextRun::new(
                vec![style.font_size() * 0.5; count].into(),
                vec![true; count + 1].into(),
            )
            .unwrap();
            let range = resources.register_run(
                request.text(),
                &geometry,
                style.font_size() * 0.75,
                style.font_size() * 0.25,
            )?;
            for (slot, glyph) in glyphs.iter_mut().zip(range) {
                *slot = glyph;
            }
            return Ok(Some(geometry));
        }
        for (index, (glyph, ch)) in glyphs.iter_mut().zip(request.text().chars()).enumerate() {
            *glyph = resources.shape_glyph(
                self,
                GlyphRequest {
                    character: ch,
                    font_size: style.font_size(),
                    font_weight: style.font_weight(),
                    font_slant: style.font_slant(),
                    color: request
                        .paint_colors()
                        .map_or(style.color(), |colors| colors[index]),
                    font_family: style.font_family(),
                },
            )?;
        }
        Ok(None)
    }
}

fn input(source: String) -> PipelineInputs {
    PipelineInputs {
        source,
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
fn source_fingerprint(doc: &html::layout::LaidOutDocument, resources: &GlyphResourceStore) -> u64 {
    let addressing = doc.render_view(resources).addressing();
    let mut hash = 0xcbf29ce484222325u64;
    let mut mix = |value: u64| {
        hash = (hash ^ value).wrapping_mul(0x100000001b3);
    };
    for run in doc.render_view(resources).text().text_runs() {
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
fn text_color_fingerprint(
    document: &html::layout::LaidOutDocument,
    shaper: &Shaper,
    resources: &GlyphResourceStore,
) -> u64 {
    let mut glyph_colors = HashMap::new();
    for (&(_, _, color), &glyph) in &shaper.cache {
        glyph_colors.insert(glyph, color);
    }
    let text = document.render_view(resources).text();
    let mut hash = 0xcbf29ce484222325u64;
    for index in 0..text.glyph_count() {
        let glyph = text.glyph_at(index).unwrap();
        let color = text.paint_color_at(index as u32).unwrap_or_else(|| {
            text.paint_handle(glyph)
                .and_then(|handle| glyph_colors.get(&handle).copied())
                .unwrap_or_default()
        });
        hash = (hash ^ u64::from(color)).wrapping_mul(0x100000001b3);
    }
    hash
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let path = &args[1];
    let case = &args[2];
    let count: usize = args.get(3).map_or(11, |n| n.parse().unwrap());
    if matches!(
        case.as_str(),
        "book_css" | "book_css_profile" | "book_cache"
    ) {
        book_css::run(
            path,
            count,
            case == "book_css_profile",
            case == "book_cache",
        );
        return;
    }
    if case.starts_with("access_") {
        access::run(path, case, count);
        return;
    }
    if case.starts_with("query_") {
        queries::run(path, case, count);
        return;
    }
    let mut inputs = input(std::fs::read_to_string(path).unwrap());
    inputs.base_uri = path.clone();
    inputs.markup_syntax = MarkupSyntax::from_uri(path);
    if case.ends_with("_book") {
        inputs.layout.text_composition_policy = TextCompositionPolicy::BookOptimized;
    }
    let palette = args.get(4).map(String::as_str).unwrap_or("links");
    match palette {
        "foreground" => {
            inputs.reader_overrides.foreground = Some(0x123456ff);
            inputs.reader_overrides.background = Some(0xeeeeddff);
            inputs.reader_overrides.link_foreground = None;
        }
        "theme" => {
            inputs.reader_overrides.theme = Some(html::layout::ThemeAdaptation {
                ink: 0xeeeeeeff,
                surface: 0x222222ff,
            });
            inputs.reader_overrides.link_foreground = Some(0x99aabbff);
        }
        "default" => {
            inputs.reader_overrides.link_foreground = None;
        }
        "links" => {}
        _ => panic!("unknown palette"),
    }
    #[cfg(feature = "profiling")]
    if matches!(
        case.as_str(),
        "open_deep_book" | "open_deep_wall_book" | "open_solver_book"
    ) {
        opening_deep::run(path, case, count, &inputs);
        return;
    }
    if matches!(case.as_str(), "open_profile" | "open_profile_book") {
        opening::run(path, case, count, &inputs);
        return;
    }
    if case == "profile" {
        let mut samples: HashMap<&str, Vec<f64>> = HashMap::new();
        for iteration in 0..count + 3 {
            let mut factory = DocumentFactory::new();
            let factory_provider = FileSystemProvider;
            let factory_resources = ResourceContext::new(&factory_provider, (path).as_ref());
            factory.set_reader_overrides(inputs.reader_overrides.layout_overrides());
            factory.set_media_environment(inputs.style_environment.media);
            let start = cpu_seconds();
            let parsed = parse_document(&inputs.source, inputs.markup_syntax).unwrap();
            let parsed_at = cpu_seconds();
            let base = factory
                .with_resources(factory_resources)
                .build_pipeline_from_parsed(&parsed, &[]);
            let prepared_at = cpu_seconds();
            let prepared = base
                .with_reader_paint_overrides(&inputs.reader_overrides)
                .unwrap();
            let painted_at = cpu_seconds();
            let mut shaper = Shaper::default();
            let mut shaped_resources = GlyphResourceStore::default();
            let shaped = prepared.shape(&mut shaper, &mut shaped_resources).unwrap();
            let shaped_at = cpu_seconds();
            let document = shaped
                .layout_with_metrics_and_shaper(
                    html::layout::LayoutConstraints::new(600.0, 16.0).unwrap(),
                    &inputs.image_metrics,
                    &mut shaper,
                    &mut shaped_resources,
                )
                .unwrap();
            let laid_out_at = cpu_seconds();
            black_box(&document);
            if iteration >= 3 {
                for (name, seconds) in [
                    ("parse", parsed_at - start),
                    ("prepare", prepared_at - parsed_at),
                    ("paint_prepare", painted_at - prepared_at),
                    ("shape", shaped_at - painted_at),
                    ("layout", laid_out_at - shaped_at),
                ] {
                    samples.entry(name).or_default().push(seconds * 1000.0);
                }
            }
        }
        let result: serde_json::Map<_, _> = samples
            .into_iter()
            .map(|(name, mut values)| {
                (
                    name.to_owned(),
                    json!({"median_cpu_ms":median(&mut values),"samples":values}),
                )
            })
            .collect();
        println!("{}", json!({"case":case,"stages":result}));
        return;
    }
    if case == "reference" {
        if palette == "links" && count % 2 == 0 {
            inputs.reader_overrides.link_foreground = Some(0x3388ccff);
        }
        let mut factory = DocumentFactory::new();
        let factory_provider = FileSystemProvider;
        let factory_resources = ResourceContext::new(&factory_provider, (path).as_ref());
        factory.set_reader_overrides(inputs.reader_overrides.layout_overrides());
        factory.set_media_environment(inputs.style_environment.media);
        let parsed = parse_document(&inputs.source, inputs.markup_syntax).unwrap();
        let prepared = factory
            .with_resources(factory_resources)
            .build_pipeline_from_parsed(&parsed, &[]);
        let mut shaper = Shaper::default();
        let mut doc_resources = GlyphResourceStore::default();
        let doc = prepared
            .shape(&mut shaper, &mut doc_resources)
            .unwrap()
            .layout_with_metrics_and_shaper(
                html::layout::LayoutConstraints::new(600.0, 16.0).unwrap(),
                &inputs.image_metrics,
                &mut shaper,
                &mut doc_resources,
            )
            .unwrap();
        println!(
            "{}",
            json!({"case":case,"chapter":path.rsplit('/').next().unwrap(),"glyphs":doc.render_view(&doc_resources).text().glyph_count(),"lines":doc.render_view(&doc_resources).text().line_count(),"render_fingerprint":format!("{:016x}",layout_fingerprint(&doc, &doc_resources)),"source_fingerprint":format!("{:016x}",source_fingerprint(&doc, &doc_resources)),"text_color_fingerprint":format!("{:016x}",text_color_fingerprint(&doc,&shaper, &doc_resources))})
        );
        return;
    }

    let mut session = PipelineSession::new(Arc::new(FileSystemProvider));
    let mut shaper = Shaper::default();
    // Cold loads need a reusable fixture; ordinary updates keep their source
    // exclusively in the session and submit only the fields being changed.
    let cold_inputs = if matches!(case.as_str(), "cold" | "reader_load" | "cold_book") {
        Some(inputs)
    } else {
        session.update(inputs, &mut shaper).unwrap();
        None
    };
    // Use an existing figure rather than bullet icons; vary decoded intrinsic
    // height while retaining source, width, CSS and all text inputs.
    let image_metric = if case.starts_with("image") {
        let doc = session.document().unwrap();
        let mut metrics = session.inputs().unwrap().image_metrics.clone();
        for (index, image) in doc
            .render_view(session.glyph_resources())
            .images()
            .iter()
            .enumerate()
        {
            if let Some((width, height)) =
                html::resources::probe_dimensions(&FileSystemProvider, &image.source)
            {
                metrics.set(index as u32, width, height);
            }
        }
        let (index, image) = doc
            .render_view(session.glyph_resources())
            .images()
            .iter()
            .enumerate()
            .find(|(index, _)| {
                doc.render_view(session.glyph_resources())
                    .image_uri(*index as u32)
                    .is_some_and(|uri| !uri.contains("square"))
            })
            .expect("real chapter contains a figure");
        let (width, height) = metrics
            .get(index as u32)
            .unwrap_or((image.width.max(1), image.height.max(1)));
        let metric = (index as u32, width, height);
        let revision = session.inputs().unwrap().image_metrics_revision.next();
        session
            .apply_changes(
                PipelineChanges {
                    image_metrics: Some(metrics),
                    image_metrics_revision: Some(revision),
                    ..Default::default()
                },
                &mut shaper,
            )
            .unwrap();
        Some(metric)
    } else {
        None
    };
    let mut instructions = Vec::with_capacity(count);
    let mut counter = if cfg!(feature = "heap") {
        None
    } else {
        counter::InstructionCounter::new()
    };
    let mut walls = Vec::with_capacity(count);
    let mut cpus = Vec::with_capacity(count);
    let mut retained = Vec::with_capacity(count);
    let mut peaks = Vec::with_capacity(count);
    let mut allocations = Vec::with_capacity(count);
    let mut allocated_bytes = Vec::with_capacity(count);
    for iteration in 0..count + 3 {
        let mut changes = PipelineChanges::default();
        match case.as_str() {
            "cold" | "reader_load" | "cold_book" => {
                session = PipelineSession::new(Arc::new(FileSystemProvider));
                shaper = Shaper::default();
            }
            "resize" | "handoff" | "resize_book" => {
                let mut layout = session.inputs().unwrap().layout;
                layout.viewport_width = if iteration % 2 == 0 { 620.0 } else { 600.0 };
                changes.layout = Some(layout);
            }
            "paint" => {
                let mut overrides = session.inputs().unwrap().reader_overrides.clone();
                overrides.link_foreground = Some(if iteration % 2 == 0 {
                    0x3388ccff
                } else {
                    0x466853ff
                });
                changes.reader_overrides = Some(overrides);
            }
            "font" => {
                changes.font_environment = Some(session.inputs().unwrap().font_environment.next())
            }
            "rehydrate" => {}
            "image" | "image_book" => {
                let (index, width, height) = image_metric.unwrap();
                let current = session.inputs().unwrap();
                let mut metrics = current.image_metrics.clone();
                metrics.set(
                    index,
                    width,
                    if iteration % 2 == 0 {
                        height + 80
                    } else {
                        height
                    },
                );
                changes.image_metrics = Some(metrics);
                changes.image_metrics_revision = Some(current.image_metrics_revision.next());
            }
            _ => panic!("unknown case"),
        }
        heap::reset();
        if let Some(counter) = counter.as_mut() {
            counter.start();
        }
        let cpu = cpu_seconds();
        let wall = Instant::now();
        if case == "rehydrate" {
            session.rehydrate_glyphs(&mut shaper).unwrap();
        } else if let Some(inputs) = &cold_inputs {
            black_box(session.update(inputs.clone(), &mut shaper).unwrap());
        } else {
            black_box(session.apply_changes(changes, &mut shaper).unwrap());
        }
        // The reader borrows the engine's published result.
        black_box(
            session
                .document()
                .unwrap()
                .render_view(session.glyph_resources())
                .text()
                .line_count(),
        );
        let elapsed = wall.elapsed().as_secs_f64() * 1000.0;
        let consumed = (cpu_seconds() - cpu) * 1000.0;
        let instruction_count = counter.as_mut().and_then(counter::InstructionCounter::stop);
        let (live, peak, allocs, bytes) = heap::sample();
        if iteration >= 3 {
            if let Some(value) = instruction_count {
                instructions.push(value as f64);
            }
            walls.push(elapsed);
            cpus.push(consumed);
            retained.push(live as f64);
            peaks.push(peak as f64);
            allocations.push(allocs as f64);
            allocated_bytes.push(bytes as f64);
        }
    }
    let doc = session.document().unwrap();
    let rss = peak_rss();
    let report = doc.memory_usage_report_with_resources(session.glyph_resources());
    let reported = report.total_bytes();
    let paragraph_cache: Vec<_> = report
        .entries
        .iter()
        .filter(|entry| entry.label.contains("paragraph_layouts"))
        .map(|entry| json!({"label":entry.label,"bytes":entry.bytes}))
        .collect();
    // Per-node allocations share labels; combine them for a compact breakdown.
    let mut by_label = BTreeMap::<String, (usize, usize)>::new();
    for entry in report.entries {
        let totals = by_label.entry(entry.label).or_default();
        totals.0 += entry.bytes;
        totals.1 += entry.count;
    }
    let memory_breakdown: Vec<_> = by_label
        .into_iter()
        .map(|(label, (bytes, count))| json!({"label":label,"bytes":bytes,"count":count}))
        .collect();
    let output = json!({"case":case,"chapter":path.rsplit('/').next().unwrap(),"samples":count,"font_backend":"deterministic mock","heap_instrumented":cfg!(feature="heap"),"reader_borrows_layout":true,"image_metric":image_metric,"paragraph_cache":paragraph_cache,"memory_breakdown":memory_breakdown,"wall_median_ms":median(&mut walls),"cpu_median_ms":median(&mut cpus),"user_instructions":if instructions.len()==count {Some(median(&mut instructions) as u64)} else {None},"heap_live_bytes":median(&mut retained) as usize,"heap_peak_bytes":median(&mut peaks) as usize,"allocation_requests":median(&mut allocations) as usize,"allocated_bytes":median(&mut allocated_bytes) as usize,"rss_peak_bytes":rss,"reported_document_bytes":reported,"glyphs":doc.render_view(session.glyph_resources()).text().glyph_count(),"lines":doc.render_view(session.glyph_resources()).text().line_count(),"render_fingerprint":format!("{:016x}",layout_fingerprint(doc, session.glyph_resources())),"text_color_fingerprint":format!("{:016x}",text_color_fingerprint(doc,&shaper, session.glyph_resources())),"source_fingerprint":format!("{:016x}",source_fingerprint(doc, session.glyph_resources()))});
    let mut output = output;
    if args.get(5).is_some_and(|argument| argument == "geometry") {
        output["geometry"] = geometry::snapshot(doc, session.glyph_resources());
    }
    println!("{output}");
}
