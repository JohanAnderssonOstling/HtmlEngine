use super::margins::MarginProfile;

#[derive(Clone, Copy)]
struct CachedMarginProfile {
    width_key: u64,
    profile: MarginProfile,
}

#[derive(Default)]
pub(super) struct MarginCache {
    profiles: Vec<Option<CachedMarginProfile>>,
}

impl MarginCache {
    pub(super) fn clear(&mut self) {
        self.profiles.clear();
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
}
