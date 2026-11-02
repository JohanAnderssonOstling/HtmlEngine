//! Compact retained text fragments. The arena selects its representation once
//! for a scan; exceptional ranges and paint orders retain the full record.

use crate::layout_state::LineTextFragment;
use html_dom::MemoryUsageReport;
use std::fmt;
use std::mem::size_of;
use std::ops::Range;
use std::slice::Iter;

#[derive(Clone, Copy)]
#[repr(C)]
pub struct PackedTextFragment {
    glyph_start: u32,
    glyph_len_minus_one: u16,
    paint_order_and_flags: u16,
    offset_x: f64,
}

const _: () = assert!(size_of::<PackedTextFragment>() == 16);

impl PackedTextFragment {
    #[inline]
    pub fn new(fragment: &LineTextFragment) -> Option<Self> {
        let len = fragment.glyphs.end.checked_sub(fragment.glyphs.start)?;
        if len == 0 || len > 1 << 16 || fragment.paint_order >= 1 << 14 {
            return None;
        }
        Some(Self {
            glyph_start: fragment.glyphs.start,
            glyph_len_minus_one: (len - 1) as u16,
            paint_order_and_flags: fragment.paint_order as u16
                | (u16::from(fragment.rtl) << 14)
                | (u16::from(fragment.visible) << 15),
            offset_x: fragment.offset_x,
        })
    }

    #[inline]
    fn glyphs(self) -> Range<u32> {
        let len = u32::from(self.glyph_len_minus_one) + 1;
        self.glyph_start..self.glyph_start + len
    }

    #[inline]
    pub fn glyphs_end(&self) -> u32 {
        self.glyphs().end
    }

    #[inline]
    pub fn extend_to(&mut self, end: u32, offset_x: f64) {
        let len = end
            .checked_sub(self.glyph_start)
            .expect("fragment end precedes its start");
        assert!(
            (1..=1 << 16).contains(&len),
            "fragment exceeds packed range capacity"
        );
        self.glyph_len_minus_one = (len - 1) as u16;
        self.offset_x = self.offset_x.min(offset_x);
    }

    #[inline]
    fn expand(self) -> LineTextFragment {
        LineTextFragment {
            glyphs: self.glyphs(),
            offset_x: self.offset_x,
            paint_order: u32::from(self.paint_order_and_flags & ((1 << 14) - 1)),
            rtl: self.paint_order_and_flags & (1 << 14) != 0,
            visible: self.paint_order_and_flags & (1 << 15) != 0,
        }
    }
}

/// Consecutive equal-length fragments share source and paint-order metadata.
/// Coordinates are independent: only exactly representable f32 values narrow.
#[derive(Clone)]
struct FragmentRun {
    start: u32,
    glyph_start: u32,
    glyph_len_minus_one: u16,
    paint_order_and_flags: u16,
}

#[derive(Clone)]
enum FragmentOffsets {
    Narrow(Vec<f32>),
    Wide(Vec<f64>),
}

impl FragmentOffsets {
    fn len(&self) -> usize {
        match self {
            Self::Narrow(values) => values.len(),
            Self::Wide(values) => values.len(),
        }
    }
    fn clear(&mut self) {
        match self {
            Self::Narrow(values) => values.clear(),
            Self::Wide(values) => values.clear(),
        }
    }
    fn reserve<T>(values: &mut Vec<T>, additional: usize) {
        let needed = values.len() + additional;
        if needed > values.capacity() {
            // Match single-element growth while reserving once for the batch.
            // Arbitrary line lengths must not enlarge the retained arena.
            values.reserve_exact(needed.next_power_of_two() - values.len());
        }
    }
    fn extend(&mut self, fragments: &[PackedTextFragment]) {
        if let Self::Narrow(values) = self {
            if fragments.iter().all(|fragment| {
                f64::from(fragment.offset_x as f32).to_bits() == fragment.offset_x.to_bits()
            }) {
                Self::reserve(values, fragments.len());
                values.extend(fragments.iter().map(|fragment| fragment.offset_x as f32));
                return;
            }
            let mut wide = Vec::with_capacity(
                values
                    .capacity()
                    .max((values.len() + fragments.len()).next_power_of_two()),
            );
            wide.extend(values.iter().map(|&value| f64::from(value)));
            *self = Self::Wide(wide);
        }
        let Self::Wide(values) = self else {
            unreachable!()
        };
        Self::reserve(values, fragments.len());
        values.extend(fragments.iter().map(|fragment| fragment.offset_x));
    }
    #[inline]
    fn push_exact(&mut self, value: f64) {
        if let Self::Narrow(values) = self {
            let narrow = value as f32;
            if f64::from(narrow).to_bits() == value.to_bits() {
                values.push(narrow);
                return;
            }
            let mut wide = Vec::with_capacity(values.capacity());
            wide.extend(values.iter().map(|&value| f64::from(value)));
            *self = Self::Wide(wide);
        }
        let Self::Wide(values) = self else {
            unreachable!()
        };
        values.push(value);
    }
    #[inline]
    fn set_last_exact(&mut self, value: f64) {
        if let Self::Narrow(values) = self {
            let narrow = value as f32;
            if f64::from(narrow).to_bits() == value.to_bits() {
                *values.last_mut().expect("current coordinate") = narrow;
                return;
            }
            let mut wide = Vec::with_capacity(values.capacity());
            wide.extend(values.iter().map(|&value| f64::from(value)));
            *self = Self::Wide(wide);
        }
        let Self::Wide(values) = self else {
            unreachable!()
        };
        *values.last_mut().expect("current coordinate") = value;
    }
    fn append(&mut self, source: Self) {
        if let Self::Narrow(values) = self
            && matches!(&source, Self::Wide(_))
        {
            let mut wide = Vec::with_capacity(values.capacity());
            wide.extend(values.iter().map(|&value| f64::from(value)));
            *self = Self::Wide(wide);
        }
        match (self, source) {
            (Self::Narrow(values), Self::Narrow(source)) => {
                Self::reserve(values, source.len());
                values.extend(source);
            }
            (Self::Wide(values), Self::Narrow(source)) => {
                Self::reserve(values, source.len());
                values.extend(source.into_iter().map(f64::from));
            }
            (Self::Wide(values), Self::Wide(source)) => {
                Self::reserve(values, source.len());
                values.extend(source);
            }
            _ => unreachable!("wide coordinates promoted above"),
        }
    }
    fn get(&self, index: usize) -> f64 {
        match self {
            Self::Narrow(values) => f64::from(values[index]),
            Self::Wide(values) => values[index],
        }
    }
    fn bytes(&self) -> usize {
        match self {
            Self::Narrow(values) => values.capacity() * size_of::<f32>(),
            Self::Wide(values) => values.capacity() * size_of::<f64>(),
        }
    }
}

#[derive(Clone)]
pub struct CompactTextFragments {
    runs: Vec<FragmentRun>,
    offsets: FragmentOffsets,
    last: Option<PackedTextFragment>,
}

impl Default for CompactTextFragments {
    fn default() -> Self {
        Self::new()
    }
}

impl CompactTextFragments {
    fn new() -> Self {
        Self {
            runs: Vec::new(),
            offsets: FragmentOffsets::Narrow(Vec::new()),
            last: None,
        }
    }
    /// Build a regular LTR sequence without materializing one metadata record per
    /// glyph. Coordinates still retain their exact source precision.
    pub fn from_glyph_offsets(
        glyphs: Range<u32>,
        paint_order: u32,
        visible: bool,
        offsets: impl IntoIterator<Item = f64>,
    ) -> Self {
        let count = glyphs
            .end
            .checked_sub(glyphs.start)
            .expect("ordered fragment glyph range");
        assert!(
            paint_order
                .checked_add(count)
                .is_some_and(|end| end <= 1 << 14)
        );
        let mut result = Self::new();
        if let FragmentOffsets::Narrow(values) = &mut result.offsets {
            FragmentOffsets::reserve(values, count as usize);
        }
        for offset in offsets {
            result.offsets.push_exact(offset);
        }
        assert_eq!(
            result.offsets.len(),
            count as usize,
            "one coordinate per fragment"
        );
        if count != 0 {
            let flags = u16::from(visible) << 15;
            result.runs.push(FragmentRun {
                start: 0,
                glyph_start: glyphs.start,
                glyph_len_minus_one: 0,
                paint_order_and_flags: flags | paint_order as u16,
            });
            result.last = Some(PackedTextFragment {
                glyph_start: glyphs.end - 1,
                glyph_len_minus_one: 0,
                paint_order_and_flags: flags | (paint_order + count - 1) as u16,
                offset_x: result.offsets.get(count as usize - 1),
            });
        }
        result
    }
    #[inline]
    pub fn push(&mut self, fragment: LineTextFragment) {
        let fragment =
            PackedTextFragment::new(&fragment).expect("line bounds prove packed fragment fields");
        let length = u32::from(fragment.glyph_len_minus_one) + 1;
        let merges = self.last.is_some_and(|previous| {
            previous.glyph_len_minus_one == fragment.glyph_len_minus_one
                && previous.glyphs_end() == fragment.glyph_start
                && previous.paint_order_and_flags & 0xc000
                    == fragment.paint_order_and_flags & 0xc000
                && u32::from(previous.paint_order_and_flags & 0x3fff) + length
                    == u32::from(fragment.paint_order_and_flags & 0x3fff)
        });
        if !merges {
            self.runs.push(FragmentRun {
                start: self.offsets.len() as u32,
                glyph_start: fragment.glyph_start,
                glyph_len_minus_one: fragment.glyph_len_minus_one,
                paint_order_and_flags: fragment.paint_order_and_flags,
            });
        }
        self.offsets.push_exact(fragment.offset_x);
        self.last = Some(fragment);
    }
    #[inline]
    pub fn last_glyphs_end(&self) -> u32 {
        self.last.expect("current text fragment").glyphs_end()
    }
    #[inline]
    pub fn extend_last_to(&mut self, end: u32, offset_x: f64) {
        let mut fragment = self.last.expect("current text fragment");
        fragment.extend_to(end, offset_x);
        let index = self.offsets.len() as u32 - 1;
        let run = self.runs.last_mut().expect("current fragment run");
        if run.start == index {
            run.glyph_len_minus_one = fragment.glyph_len_minus_one;
        } else {
            self.runs.push(FragmentRun {
                start: index,
                glyph_start: fragment.glyph_start,
                glyph_len_minus_one: fragment.glyph_len_minus_one,
                paint_order_and_flags: fragment.paint_order_and_flags,
            });
        }
        self.offsets.set_last_exact(fragment.offset_x);
        self.last = Some(fragment);
    }
    fn append(&mut self, mut source: Self) {
        let start = self.offsets.len() as u32;
        if source.offsets.len() == 0 {
            return;
        }
        let first = &source.runs[0];
        let merges = self.last.is_some_and(|previous| {
            previous.glyph_len_minus_one == first.glyph_len_minus_one
                && previous.glyphs_end() == first.glyph_start
                && previous.paint_order_and_flags & 0xc000 == first.paint_order_and_flags & 0xc000
                && u32::from(previous.paint_order_and_flags & 0x3fff)
                    + u32::from(first.glyph_len_minus_one)
                    + 1
                    == u32::from(first.paint_order_and_flags & 0x3fff)
        });
        let additional = source.runs.len() - usize::from(merges);
        let needed = self.runs.len() + additional;
        if needed > self.runs.capacity() {
            self.runs
                .reserve_exact(needed.next_power_of_two() - self.runs.len());
        }
        self.runs
            .extend(source.runs.drain(usize::from(merges)..).map(|mut run| {
                run.start += start;
                run
            }));
        self.offsets.append(source.offsets);
        self.last = source.last;
    }
    fn extend(&mut self, fragments: &[PackedTextFragment]) {
        let start = self.offsets.len() as u32;
        let (mut next_glyph, mut next_paint, mut length, mut flags) =
            self.runs.last().map_or((0, u32::MAX, 0, 0), |run| {
                let length = u32::from(run.glyph_len_minus_one) + 1;
                let delta = (start - run.start) * length;
                (
                    run.glyph_start + delta,
                    u32::from(run.paint_order_and_flags & 0x3fff) + delta,
                    length,
                    run.paint_order_and_flags & 0xc000,
                )
            });
        // Compare adjacent records directly. The run's initial source and paint
        // order need no multiplication for each glyph in a regular sequence.
        for (offset, fragment) in fragments.iter().enumerate() {
            let fragment_length = u32::from(fragment.glyph_len_minus_one) + 1;
            let fragment_flags = fragment.paint_order_and_flags & 0xc000;
            let paint_order = u32::from(fragment.paint_order_and_flags & 0x3fff);
            if fragment.glyph_start != next_glyph
                || paint_order != next_paint
                || fragment_length != length
                || fragment_flags != flags
            {
                self.runs.push(FragmentRun {
                    start: start + offset as u32,
                    glyph_start: fragment.glyph_start,
                    glyph_len_minus_one: fragment.glyph_len_minus_one,
                    paint_order_and_flags: fragment.paint_order_and_flags,
                });
            }
            next_glyph = fragment.glyph_start + fragment_length;
            next_paint = paint_order + fragment_length;
            length = fragment_length;
            flags = fragment_flags;
        }
        self.offsets.extend(fragments);
        if let Some(last) = fragments.last() {
            self.last = Some(*last);
        }
    }
    fn glyphs(&self, run: &FragmentRun, index: usize) -> Range<u32> {
        let len = u32::from(run.glyph_len_minus_one) + 1;
        let start = run.glyph_start + (index as u32 - run.start) * len;
        start..start + len
    }
    fn expand(&self, run: &FragmentRun, index: usize) -> LineTextFragment {
        let glyphs = self.glyphs(run, index);
        LineTextFragment {
            paint_order: u32::from(run.paint_order_and_flags & 0x3fff)
                + (glyphs.start - run.glyph_start),
            glyphs,
            offset_x: self.offsets.get(index),
            rtl: run.paint_order_and_flags & 0x4000 != 0,
            visible: run.paint_order_and_flags & 0x8000 != 0,
        }
    }
    fn cursor(&self, range: Range<usize>) -> CompactFragmentIter<'_> {
        assert!(range.start <= range.end && range.end <= self.offsets.len());
        let run = self
            .runs
            .partition_point(|run| run.start as usize <= range.start)
            .saturating_sub(1);
        CompactFragmentIter {
            source: self,
            run,
            range,
        }
    }
    fn bytes(&self) -> usize {
        self.runs.capacity() * size_of::<FragmentRun>() + self.offsets.bytes()
    }
    fn retained_bytes(&self) -> usize {
        self.runs.len() * size_of::<FragmentRun>()
            + self.offsets.len()
                * match self.offsets {
                    FragmentOffsets::Narrow(_) => size_of::<f32>(),
                    FragmentOffsets::Wide(_) => size_of::<f64>(),
                }
    }
}

struct CompactFragmentIter<'a> {
    source: &'a CompactTextFragments,
    run: usize,
    range: Range<usize>,
}
impl Iterator for CompactFragmentIter<'_> {
    type Item = LineTextFragment;
    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        let index = self.range.next()?;
        while self
            .source
            .runs
            .get(self.run + 1)
            .is_some_and(|run| run.start as usize <= index)
        {
            self.run += 1;
        }
        Some(self.source.expand(&self.source.runs[self.run], index))
    }
}

#[derive(Clone)]
enum Storage {
    Compact(CompactTextFragments),
    Packed(Vec<PackedTextFragment>),
    Wide(Vec<LineTextFragment>),
}

/// Positioned fragments produced in their selected retained format. Ordinary
/// lines avoid an intermediate array of expanded records before publication.
pub enum OwnedTextFragments {
    Compact(CompactTextFragments),
    Packed(Box<[PackedTextFragment]>),
    Wide(Box<[LineTextFragment]>),
}

impl OwnedTextFragments {
    fn len(&self) -> usize {
        match self {
            Self::Compact(records) => records.offsets.len(),
            Self::Packed(records) => records.len(),
            Self::Wide(records) => records.len(),
        }
    }
}

/// One shared arena for all positioned text fragments. A record that cannot
/// fit the packed fields promotes the arena without changing its indexes.
#[derive(Clone)]
pub struct TextFragmentStore {
    storage: Storage,
}

impl Default for TextFragmentStore {
    fn default() -> Self {
        Self {
            storage: Storage::Compact(CompactTextFragments::new()),
        }
    }
}

impl TextFragmentStore {
    pub fn len(&self) -> usize {
        match &self.storage {
            Storage::Compact(records) => records.offsets.len(),
            Storage::Packed(records) => records.len(),
            Storage::Wide(records) => records.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Reuse capacity in either format, including an arena promoted by a
    /// previous exceptional line. A new document starts with packed storage.
    pub fn clear(&mut self) {
        match &mut self.storage {
            Storage::Compact(records) => {
                records.runs.clear();
                records.offsets.clear();
                records.last = None;
            }
            Storage::Packed(records) => records.clear(),
            Storage::Wide(records) => records.clear(),
        }
    }

    pub(crate) fn append(&mut self, fragments: OwnedTextFragments) -> Range<u32> {
        let start = u32::try_from(self.len()).expect("line text-fragment capacity exhausted");
        let len = u32::try_from(fragments.len()).expect("line text-fragment capacity exhausted");
        let end = start
            .checked_add(len)
            .expect("line text-fragment capacity exhausted");
        let fragments = match fragments {
            OwnedTextFragments::Wide(source) if matches!(self.storage, Storage::Compact(_)) => {
                if source
                    .iter()
                    .all(|record| PackedTextFragment::new(record).is_some())
                {
                    OwnedTextFragments::Packed(
                        source
                            .iter()
                            .map(|record| {
                                PackedTextFragment::new(record).expect("validated packed fragment")
                            })
                            .collect(),
                    )
                } else {
                    let Storage::Compact(records) = &self.storage else {
                        unreachable!()
                    };
                    let mut wide = Vec::with_capacity(end as usize);
                    wide.extend(records.cursor(0..start as usize));
                    self.storage = Storage::Wide(wide);
                    OwnedTextFragments::Wide(source)
                }
            }
            other => other,
        };
        match (&mut self.storage, fragments) {
            (Storage::Compact(records), OwnedTextFragments::Compact(source)) => {
                records.append(source)
            }
            (Storage::Packed(records), OwnedTextFragments::Compact(source)) => {
                records.extend(source.cursor(0..source.offsets.len()).map(|fragment| {
                    PackedTextFragment::new(&fragment).expect("compact fields fit packed record")
                }))
            }
            (Storage::Wide(records), OwnedTextFragments::Compact(source)) => {
                records.extend(source.cursor(0..source.offsets.len()))
            }
            (Storage::Compact(records), OwnedTextFragments::Packed(source)) => {
                records.extend(&source);
            }
            (Storage::Compact(_), OwnedTextFragments::Wide(_)) => {
                unreachable!("wide fragments promoted above")
            }
            (Storage::Packed(records), OwnedTextFragments::Packed(source)) => {
                records.extend(Vec::from(source));
            }
            (Storage::Wide(records), OwnedTextFragments::Wide(source)) => {
                records.extend(Vec::from(source));
            }
            (Storage::Wide(records), OwnedTextFragments::Packed(source)) => {
                records.extend(Vec::from(source).into_iter().map(|record| record.expand()));
            }
            (Storage::Packed(records), OwnedTextFragments::Wide(source)) => {
                // Paint order grows through a line. Reject a non-fitting last
                // record first, then validate all records before conversion.
                let can_pack = source
                    .last()
                    .is_none_or(|record| PackedTextFragment::new(record).is_some())
                    && source
                        .iter()
                        .all(|record| PackedTextFragment::new(record).is_some());
                if can_pack {
                    records.extend(source.iter().map(|record| {
                        PackedTextFragment::new(record)
                            .expect("validated fragment fits packed fields")
                    }));
                } else {
                    let mut wide = Vec::with_capacity(end as usize);
                    wide.extend(records.iter().map(|record| record.expand()));
                    wide.extend(Vec::from(source));
                    self.storage = Storage::Wide(wide);
                }
            }
        }
        if let Storage::Compact(records) = &self.storage {
            // Irregular metadata or wide coordinates can make direct records
            // cheaper. Promote once; preserve all logical fragment indexes.
            if records.offsets.len() >= 1024
                && records.retained_bytes()
                    > records.offsets.len() * size_of::<PackedTextFragment>()
            {
                let packed = records
                    .cursor(0..records.offsets.len())
                    .map(|record| {
                        PackedTextFragment::new(&record)
                            .expect("compact fragments fit packed fields")
                    })
                    .collect();
                self.storage = Storage::Packed(packed);
            }
        }
        start..end
    }

    pub(crate) fn iter(&self, range: Range<u32>) -> LineTextFragments<'_> {
        let range = range.start as usize..range.end as usize;
        LineTextFragments {
            source: match &self.storage {
                Storage::Compact(records) => FragmentSource::Compact(records.cursor(range)),
                Storage::Packed(records) => FragmentSource::Packed(records[range].iter()),
                Storage::Wide(records) => FragmentSource::Wide(records[range].iter()),
            },
        }
    }

    /// Visit adjacent source-glyph ranges together, without reading positions
    /// or paint flags. Gaps and cross-line ownership remain distinct.
    #[inline]
    pub fn for_each_glyph_span(&self, range: Range<u32>, visit: impl FnMut(Range<u32>)) {
        let range = range.start as usize..range.end as usize;
        match &self.storage {
            Storage::Compact(records) => {
                let mut cursor = records.cursor(range);
                let ranges = std::iter::from_fn(|| {
                    let index = cursor.range.start;
                    if index == cursor.range.end {
                        return None;
                    }
                    while records
                        .runs
                        .get(cursor.run + 1)
                        .is_some_and(|run| run.start as usize <= index)
                    {
                        cursor.run += 1;
                    }
                    let end = records
                        .runs
                        .get(cursor.run + 1)
                        .map_or(cursor.range.end, |run| {
                            (run.start as usize).min(cursor.range.end)
                        });
                    let run = &records.runs[cursor.run];
                    cursor.range.start = end;
                    Some(records.glyphs(run, index).start..records.glyphs(run, end - 1).end)
                });
                visit_spans(ranges, visit)
            }
            Storage::Packed(records) => {
                visit_spans(records[range].iter().map(|record| record.glyphs()), visit)
            }
            Storage::Wide(records) => visit_spans(
                records[range].iter().map(|record| record.glyphs.clone()),
                visit,
            ),
        }
    }

    pub fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        match &self.storage {
            Storage::Compact(records) => {
                report.add("storage", records.bytes(), records.offsets.len());
            }
            Storage::Packed(records) => report.add_vec_storage("storage", records),
            Storage::Wide(records) => report.add_vec_storage("storage", records),
        }
        report
    }
}

#[inline]
fn visit_spans(ranges: impl Iterator<Item = Range<u32>>, mut visit: impl FnMut(Range<u32>)) {
    let mut ranges = ranges.filter(|range| !range.is_empty());
    let Some(mut pending) = ranges.next() else {
        return;
    };
    for range in ranges {
        if pending.end == range.start {
            pending.end = range.end;
        } else {
            visit(pending);
            pending = range;
        }
    }
    visit(pending);
}

impl fmt::Debug for TextFragmentStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries(self.iter(0..self.len() as u32))
            .finish()
    }
}

enum FragmentSource<'a> {
    Compact(CompactFragmentIter<'a>),
    Packed(Iter<'a, PackedTextFragment>),
    Wide(Iter<'a, LineTextFragment>),
    Implicit(Option<Range<u32>>),
}

/// Iterates stored text fragments or the allocation-free whole-line default.
pub struct LineTextFragments<'a> {
    source: FragmentSource<'a>,
}

impl LineTextFragments<'_> {
    pub(crate) fn implicit(glyphs: Range<u32>) -> Self {
        Self {
            source: FragmentSource::Implicit((!glyphs.is_empty()).then_some(glyphs)),
        }
    }
}

impl Iterator for LineTextFragments<'_> {
    type Item = LineTextFragment;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.source {
            FragmentSource::Compact(records) => records.next(),
            FragmentSource::Packed(records) => records.next().map(|record| record.expand()),
            FragmentSource::Wide(records) => records.next().cloned(),
            FragmentSource::Implicit(glyphs) => Some(LineTextFragment {
                glyphs: glyphs.take()?,
                offset_x: 0.0,
                rtl: false,
                paint_order: 0,
                visible: true,
            }),
        }
    }
}
