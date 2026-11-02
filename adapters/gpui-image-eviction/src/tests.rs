use super::*;
use gpui::prelude::*;
use gpui::{Corners, TestAppContext, VisualTestContext, canvas, div, point, px, size};
use image::{Frame, Rgba, RgbaImage};
use smallvec::SmallVec;
use std::time::Duration;

fn image(frames: usize) -> Arc<RenderImage> {
    Arc::new(RenderImage::new(SmallVec::from_iter((0..frames).map(|_| Frame::new(RgbaImage::from_pixel(2, 2, Rgba([0, 0, 0, 255])))))))
}

fn paint(window: &mut VisualTestContext, image: &Arc<RenderImage>) {
    let image = image.clone();
    window.draw(point(px(0.), px(0.)), size(px(100.), px(100.)), move |_, _| {
        canvas(
            |_, _, _| (),
            move |bounds, _, window, cx| {
                track_image(&image, cx);
                for frame in 0..image.frame_count() {
                    window.paint_image(bounds, bounds, Corners::default(), image.clone(), frame, false).unwrap();
                }
            },
        )
        .size_full()
        .into_any_element()
    });
}

fn clear(window: &mut VisualTestContext) {
    window.draw(point(px(0.), px(0.)), size(px(100.), px(100.)), |_, _| div().into_any_element());
}

fn pressure(cx: &mut TestAppContext) {
    // New activity, rather than a timer, asks the cache to reclaim old pixels.
    let next = image(1);
    cx.update(|cx| track_image(&next, cx));
    cx.run_until_parked();
}

#[gpui::test]
fn shared_pages_protect_images_until_cache_and_visible_page_release_them(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_global(PaintedImages::new(16, 0)));
    let mut window = cx.add_empty_window().clone();
    // A shared page can have many owners while its image has only one.
    let cache = Arc::new(vec![image(1)]);
    let visible_page = cache.clone();
    let weak = Arc::downgrade(&cache[0]);
    let mut probe = RenderImage::new(vec![Frame::new(RgbaImage::new(2, 2))]);
    probe.id = cache[0].id;
    paint(&mut window, &cache[0]);
    pressure(cx);
    drop(cache);
    pressure(cx);
    assert!(window.update(|window, _| window.has_image_atlas_entry(&probe)));
    assert!(weak.upgrade().is_some());
    drop(visible_page);
    clear(&mut window);
    pressure(cx);
    assert!(!window.update(|window, _| window.has_image_atlas_entry(&probe)));
    assert!(weak.upgrade().is_none());
}

#[gpui::test]
fn pressure_releases_every_frame_from_every_window(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_global(PaintedImages::new(32, 16)));
    let mut first = cx.add_empty_window().clone();
    let mut second = cx.add_empty_window().clone();
    let current = image(2);
    let weak = Arc::downgrade(&current);
    let mut probe = RenderImage::new(SmallVec::from_iter((0..2).map(|_| Frame::new(RgbaImage::new(2, 2)))));
    probe.id = current.id;
    paint(&mut first, &current);
    paint(&mut second, &current);
    assert!(first.update(|window, _| window.has_image_atlas_entry(&probe)));
    assert!(second.update(|window, _| window.has_image_atlas_entry(&probe)));
    cx.update(|cx| assert_eq!(cx.global::<PaintedImages>().bytes, 32, "count all frames, once per image"));
    drop(current);
    clear(&mut first);
    clear(&mut second);
    pressure(cx);
    assert!(!first.update(|window, _| window.has_image_atlas_entry(&probe)));
    assert!(!second.update(|window, _| window.has_image_atlas_entry(&probe)));
    assert!(weak.upgrade().is_none());
    cx.update(|cx| assert_eq!(cx.global::<PaintedImages>().bytes, 16));
}

#[gpui::test]
fn unused_images_stay_cached_without_timer_work(cx: &mut TestAppContext) {
    let current = image(1);
    let weak = Arc::downgrade(&current);
    cx.update(|cx| {
        for _ in 0..100 {
            track_image(&current, cx);
        }
        let state = cx.global::<PaintedImages>();
        assert_eq!(state.images.len(), 1);
        assert_eq!(state.bytes, 16);
        assert!(!state.cleanup_scheduled);
    });
    cx.run_until_parked();
    assert_eq!(Arc::strong_count(&current), 2);
    drop(current);
    cx.executor().advance_clock(Duration::from_secs(3600));
    cx.run_until_parked();
    assert!(weak.upgrade().is_some(), "elapsed idle time must not run image eviction");
    cx.update(|cx| assert!(!cx.global::<PaintedImages>().cleanup_scheduled));
}

#[gpui::test]
fn pressure_is_coalesced_and_pinned_images_make_the_budget_soft(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_global(PaintedImages::new(16, 0)));
    let first = image(1);
    let second = image(1);
    cx.update(|cx| {
        track_image(&first, cx);
        track_image(&second, cx);
        assert!(cx.global::<PaintedImages>().cleanup_scheduled);
        for _ in 0..100 {
            track_image(&first, cx);
        }
    });
    cx.run_until_parked();
    cx.update(|cx| {
        let state = cx.global::<PaintedImages>();
        assert_eq!(state.images.len(), 2, "external owners survive pressure");
        assert_eq!(state.bytes, 32);
        assert!(!state.cleanup_scheduled, "an unsuccessful sweep must not reschedule itself");
        track_image(&second, cx);
        assert!(!cx.global::<PaintedImages>().cleanup_scheduled, "repainting existing images does not repeat the pressure scan");
    });
    let first_weak = Arc::downgrade(&first);
    drop(first);
    pressure(cx);
    assert!(first_weak.upgrade().is_none());
}

#[test]
fn eviction_uses_recency_and_batches_down_to_the_low_water_mark() {
    let mut cache = PaintedImages::new(48, 32);
    let first = image(1);
    let second = image(1);
    let third = image(1);
    let first_weak = Arc::downgrade(&first);
    let second_weak = Arc::downgrade(&second);
    let third_weak = Arc::downgrade(&third);
    cache.track(&first);
    cache.track(&second);
    cache.track(&third);
    drop(first);
    drop(second);
    drop(third);
    assert!(cache.evict_unused().is_empty(), "the budget is inclusive");
    cache.track(&first_weak.upgrade().unwrap());
    let fourth = image(1);
    cache.track(&fourth);
    drop(cache.evict_unused());
    assert_eq!(cache.bytes, 32);
    assert!(first_weak.upgrade().is_some());
    assert!(second_weak.upgrade().is_none());
    assert!(third_weak.upgrade().is_none());
}

#[test]
fn the_newest_prefetched_image_survives_until_it_can_be_painted() {
    let mut cache = PaintedImages::new(16, 0);
    let visible = image(1);
    cache.track(&visible);
    let next = image(2);
    let weak = Arc::downgrade(&next);
    cache.track(&next);
    drop(next);
    assert!(cache.evict_unused().is_empty());
    assert!(weak.upgrade().is_some(), "do not immediately evict the image whose arrival triggered the sweep");
}

#[gpui::test]
fn explicit_trim_releases_unused_images_below_budget_and_protects_visible_owners(cx: &mut TestAppContext) {
    let mut first = cx.add_empty_window().clone();
    let mut second = cx.add_empty_window().clone();
    let current = image(2);
    let weak = Arc::downgrade(&current);
    let mut probe = RenderImage::new(vec![Frame::new(RgbaImage::new(2, 2)); 2]);
    probe.id = current.id;
    paint(&mut first, &current);
    paint(&mut second, &current);
    cx.update(trim_unused_images);
    cx.run_until_parked();
    assert!(weak.upgrade().is_some(), "a reader or visible frame still owns this image");
    drop(current);
    clear(&mut first);
    clear(&mut second);
    cx.update(trim_unused_images);
    cx.run_until_parked();
    assert!(weak.upgrade().is_none(), "explicit trim must reclaim even the newest image below budget");
    assert!(!first.update(|window, _| window.has_image_atlas_entry(&probe)));
    assert!(!second.update(|window, _| window.has_image_atlas_entry(&probe)));
    cx.update(|cx| assert_eq!(cx.global::<PaintedImages>().bytes, 0));
}

#[gpui::test]
fn closing_one_reader_reclaims_its_images_and_preserves_another_windows_images(cx: &mut TestAppContext) {
    let mut first = cx.add_empty_window().clone();
    let mut second = cx.add_empty_window().clone();
    let first_reader = image(2);
    let second_reader = first_reader.clone();
    let shared_weak = Arc::downgrade(&first_reader);
    let first_only = image(1);
    let first_only_weak = Arc::downgrade(&first_only);
    cx.update(|cx| track_image(&first_only, cx));
    let mut probe = RenderImage::new(vec![Frame::new(RgbaImage::new(2, 2)); 2]);
    probe.id = first_reader.id;
    paint(&mut first, &first_reader);
    paint(&mut second, &second_reader);

    drop(first_only);
    drop(first_reader);
    first.update(|window, _| trim_unused_images_after_transition(window));
    first.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
    assert!(first_only_weak.upgrade().is_some(), "cleanup must wait for the outgoing frame to be replaced");
    clear(&mut first);
    first.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
    assert!(first_only_weak.upgrade().is_none(), "closing a reader must release its unused images below budget");
    assert!(shared_weak.upgrade().is_some());
    assert!(second.update(|window, _| window.has_image_atlas_entry(&probe)), "the other window must keep its shared atlas image");

    drop(second_reader);
    second.update(|window, _| trim_unused_images_after_transition(window));
    second.update(|window, cx| window.simulate_next_frame(cx));
    clear(&mut second);
    second.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
    assert!(shared_weak.upgrade().is_none(), "the last reader closing must release the shared pixels");
    assert!(!first.update(|window, _| window.has_image_atlas_entry(&probe)));
    assert!(!second.update(|window, _| window.has_image_atlas_entry(&probe)));
    cx.update(|cx| assert_eq!(cx.global::<PaintedImages>().bytes, 0));
}

#[gpui::test]
fn explicit_trim_overrides_an_already_scheduled_pressure_sweep(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_global(PaintedImages::new(16, 0)));
    let first = image(1);
    let second = image(1);
    let weak = Arc::downgrade(&second);
    cx.update(move |cx| {
        track_image(&first, cx);
        track_image(&second, cx);
        assert!(cx.global::<PaintedImages>().cleanup_scheduled);
        trim_unused_images(cx);
        assert!(cx.global::<PaintedImages>().trim_requested);
        drop(first);
        drop(second);
    });
    cx.run_until_parked();
    assert!(weak.upgrade().is_none());
    cx.update(|cx| assert_eq!(cx.global::<PaintedImages>().bytes, 0));
}

#[gpui::test]
fn page_turns_keep_reusable_pixels_within_the_budget(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_global(PaintedImages::new(48, 32)));
    let mut window = cx.add_empty_window().clone();
    let mut old = Vec::new();
    for _ in 0..20 {
        let current = image(1);
        old.push(Arc::downgrade(&current));
        paint(&mut window, &current);
        cx.run_until_parked();
    }
    cx.update(|cx| assert!(cx.global::<PaintedImages>().bytes <= 48));
    assert!(old[..16].iter().all(|image| image.upgrade().is_none()));
}
