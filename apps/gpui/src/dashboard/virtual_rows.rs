//! Shared measurement state for dashboard virtual rows.
//!
//! GPUI Kit's virtual list needs row heights before it lays out the visible
//! range. This cache lets visible rows report their natural child bounds via
//! `Div::on_children_prepainted`; unseen rows may use an estimate until they
//! become visible. Keys include all inputs that can change wrapping.

use std::{
    cell::RefCell,
    collections::HashMap,
    hash::{Hash, Hasher},
    rc::Rc,
};

use gpui_kit::{
    AnyElement, IntoElement, ParentElement as _, Pixels, Render, Styled as _, WeakEntity, div, px,
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(super) struct RowMeasureKey {
    pub(super) identity: String,
    pub(super) revision: u64,
    pub(super) layout: u8,
    pub(super) expanded: bool,
}

pub(super) struct RowMeasurements {
    pub(super) geometry: Option<(u32, u32)>,
    pub(super) entries: HashMap<RowMeasureKey, Pixels>,
}

pub(super) type RowMeasureCache = Rc<RefCell<RowMeasurements>>;

pub(super) fn new_row_measure_cache() -> RowMeasureCache {
    Rc::new(RefCell::new(RowMeasurements {
        geometry: None,
        entries: HashMap::new(),
    }))
}

pub(super) fn width_bucket(width: Pixels) -> u32 {
    (width.as_f32().max(0.) * 2.).round() as u32
}

pub(super) fn revision_hash(value: impl Hash) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

pub(super) fn cached_height(cache: &RowMeasureCache, key: &RowMeasureKey, estimate: f32) -> Pixels {
    cache
        .borrow()
        .entries
        .get(key)
        .copied()
        .unwrap_or_else(|| px(estimate.max(1.)))
}

fn update_measured_geometry(
    cache: &RowMeasureCache,
    key: RowMeasureKey,
    width: Pixels,
    rem_size: Pixels,
    height: Pixels,
) -> bool {
    let geometry = (width_bucket(width), width_bucket(rem_size));
    let mut cache = cache.borrow_mut();
    let geometry_changed = cache.geometry != Some(geometry);
    if geometry_changed {
        cache.entries.clear();
        cache.geometry = Some(geometry);
    }
    cache.entries.retain(|candidate, _| {
        candidate.identity != key.identity
            || candidate.layout != key.layout
            || candidate.expanded != key.expanded
            || candidate.revision == key.revision
    });
    let height = px(height.as_f32().max(1.));
    let height_changed = cache
        .entries
        .get(&key)
        .is_none_or(|previous| (previous.as_f32() - height.as_f32()).abs() > 0.5);
    if height_changed {
        cache.entries.insert(key, height);
    }
    geometry_changed || height_changed
}

pub(super) fn retain_identities(
    cache: &RowMeasureCache,
    identities: &std::collections::HashSet<String>,
) {
    cache
        .borrow_mut()
        .entries
        .retain(|key, _| identities.contains(&key.identity));
}

pub(super) fn measured_row<V: Render + 'static>(
    key: RowMeasureKey,
    cache: RowMeasureCache,
    entity: WeakEntity<V>,
    child: impl IntoElement,
) -> AnyElement {
    div()
        .w_full()
        .min_w_0()
        .h_auto()
        .flex_shrink_0()
        .child(child)
        .on_children_prepainted(move |bounds, window, app| {
            let Some(bounds) = bounds.first() else { return };
            if update_measured_geometry(
                &cache,
                key.clone(),
                bounds.size.width,
                window.rem_size(),
                bounds.size.height,
            ) {
                let _ = entity.update(app, |_, cx| cx.notify());
                window.request_animation_frame();
            }
        })
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> RowMeasureKey {
        RowMeasureKey {
            identity: "ISSUE-1".to_owned(),
            revision: 1,
            layout: 1,
            expanded: false,
        }
    }

    #[test]
    fn cache_only_invalidates_when_geometry_changes() {
        let cache = new_row_measure_cache();
        assert!(update_measured_geometry(
            &cache,
            key(),
            px(100.),
            px(16.),
            px(120.)
        ));
        assert!(!update_measured_geometry(
            &cache,
            key(),
            px(100.),
            px(16.),
            px(120.4)
        ));
        assert!(update_measured_geometry(
            &cache,
            key(),
            px(100.),
            px(16.),
            px(122.)
        ));
        assert_eq!(cached_height(&cache, &key(), 80.).as_f32(), 122.);
        let mut other = key();
        other.identity = "ISSUE-2".to_owned();
        assert!(update_measured_geometry(
            &cache,
            other.clone(),
            px(200.),
            px(16.),
            px(90.),
        ));
        assert_eq!(cached_height(&cache, &key(), 80.).as_f32(), 80.);
        assert_eq!(cached_height(&cache, &other, 80.).as_f32(), 90.);

        let mut revised = other.clone();
        revised.revision = revision_hash("same length");
        assert!(update_measured_geometry(
            &cache,
            revised.clone(),
            px(200.),
            px(16.),
            px(91.),
        ));
        assert_eq!(cached_height(&cache, &other, 80.).as_f32(), 80.);
        assert_ne!(revision_hash("AB"), revision_hash("CD"));
    }
}
