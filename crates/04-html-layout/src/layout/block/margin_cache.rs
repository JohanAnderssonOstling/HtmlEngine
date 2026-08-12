use super::margins::{MarginProfile, MarginStrut};

#[derive(Clone, Copy)]
struct CachedMarginProfile {
    width_key: u64,
    profile: MarginProfile,
}

#[derive(Clone, Copy)]
struct CachedAfterMargin {
    width_key: u64,
    height_key: u64,
    margin: MarginStrut,
}

#[derive(Default)]
pub(super) struct MarginCache {
    profiles: Vec<Option<CachedMarginProfile>>,
    after_margins: Vec<Option<CachedAfterMargin>>,
}

impl MarginCache {
    pub(super) fn clear(&mut self) {
        self.profiles.clear();
        self.after_margins.clear();
    }

    pub(super) fn profile(&self, box_idx: usize, width_key: u64) -> Option<MarginProfile> {
        self.profiles
            .get(box_idx)
            .and_then(|entry| *entry)
            .and_then(|entry| (entry.width_key == width_key).then_some(entry.profile))
    }

    pub(super) fn insert_profile(
        &mut self,
        box_idx: usize,
        width_key: u64,
        profile: MarginProfile,
    ) {
        if self.profiles.len() <= box_idx {
            self.profiles.resize(box_idx + 1, None);
        }
        self.profiles[box_idx] = Some(CachedMarginProfile { width_key, profile });
    }

    pub(super) fn after_margin(
        &self,
        box_idx: usize,
        width_key: u64,
        height_key: u64,
    ) -> Option<MarginStrut> {
        self.after_margins
            .get(box_idx)
            .and_then(|entry| *entry)
            .and_then(|entry| {
                (entry.width_key == width_key && entry.height_key == height_key)
                    .then_some(entry.margin)
            })
    }

    pub(super) fn insert_after_margin(
        &mut self,
        box_idx: usize,
        width_key: u64,
        height_key: u64,
        margin: MarginStrut,
    ) {
        if self.after_margins.len() <= box_idx {
            self.after_margins.resize(box_idx + 1, None);
        }
        self.after_margins[box_idx] = Some(CachedAfterMargin {
            width_key,
            height_key,
            margin,
        });
    }
}
