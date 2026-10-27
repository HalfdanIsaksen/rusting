//! GPU vertex/index buffers for stroke meshes, cached by [`StrokeId`].
//!
//! A stroke is tessellated and uploaded once, then reused every frame until
//! its [`StrokeKey`] changes or it is invalidated. Entries that are not drawn
//! for a while are evicted.

use crate::mesh::{MeshSource, valid_index_count};
use animation_core::StrokeId;
use drawing::Stroke;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use wgpu::util::DeviceExt;

/// Renders an entry may go unused before [`StrokeCache::evict_unused`] drops it.
pub const DEFAULT_MAX_UNUSED_RENDERS: u64 = 600;

/// A cheap fingerprint of the stroke data that affects its mesh.
///
/// [`StrokeKey::of`] looks at the point count, brush width and the first and
/// last points, so it catches a stroke growing while it is drawn and width
/// changes in O(1). Edits that keep all of those the same (for example moving
/// a middle point) must call [`StrokeCache::invalidate`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StrokeKey(u64);

impl StrokeKey {
    /// Fingerprint of `stroke`'s geometry inputs.
    pub fn of(stroke: &Stroke) -> Self {
        let mut hasher = DefaultHasher::new();
        stroke.points.len().hash(&mut hasher);
        stroke.style.width.to_bits().hash(&mut hasher);
        for point in [stroke.points.first(), stroke.points.last()]
            .into_iter()
            .flatten()
        {
            point.pos.x.to_bits().hash(&mut hasher);
            point.pos.y.to_bits().hash(&mut hasher);
            point.pressure.to_bits().hash(&mut hasher);
        }
        Self(hasher.finish())
    }

    /// A key from a caller-chosen value (for meshes that are not strokes).
    pub const fn from_raw(value: u64) -> Self {
        Self(value)
    }
}

/// Uploaded geometry of one stroke.
#[derive(Debug)]
pub(crate) struct GpuMesh {
    /// `None` for empty meshes.
    pub(crate) buffers: Option<(wgpu::Buffer, wgpu::Buffer)>,
    pub(crate) index_count: u32,
}

#[derive(Debug)]
struct Entry<T> {
    key: StrokeKey,
    last_used: u64,
    value: T,
}

/// Key/age bookkeeping, separate from wgpu so it can be unit-tested.
#[derive(Debug)]
struct Cache<T> {
    entries: HashMap<StrokeId, Entry<T>>,
    /// Incremented once per render.
    clock: u64,
}

impl<T> Default for Cache<T> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            clock: 0,
        }
    }
}

impl<T> Cache<T> {
    /// Returns true if `id` has an entry with this `key`, marking it used.
    fn touch(&mut self, id: StrokeId, key: StrokeKey) -> bool {
        match self.entries.get_mut(&id) {
            Some(entry) if entry.key == key => {
                entry.last_used = self.clock;
                true
            }
            _ => false,
        }
    }

    fn insert(&mut self, id: StrokeId, key: StrokeKey, value: T) {
        self.entries.insert(
            id,
            Entry {
                key,
                last_used: self.clock,
                value,
            },
        );
    }

    fn get(&mut self, id: StrokeId) -> Option<&T> {
        let entry = self.entries.get_mut(&id)?;
        entry.last_used = self.clock;
        Some(&entry.value)
    }

    fn tick(&mut self) {
        self.clock += 1;
    }

    fn evict_unused(&mut self, max_unused: u64) -> usize {
        let before = self.entries.len();
        let clock = self.clock;
        self.entries
            .retain(|_, entry| clock.saturating_sub(entry.last_used) <= max_unused);
        before - self.entries.len()
    }
}

/// Stroke meshes uploaded to the GPU, keyed by [`StrokeId`].
#[derive(Debug, Default)]
pub struct StrokeCache {
    cache: Cache<GpuMesh>,
    /// Bumped whenever cached geometry changes.
    generation: u64,
}

impl StrokeCache {
    /// An empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Makes sure `id` has an up-to-date mesh. If there is no entry, or its
    /// key differs from `key`, `build` is called and its mesh uploaded.
    /// Returns true if something was uploaded. If `build` returns `None`
    /// nothing is stored and the stroke is not drawn.
    pub fn ensure<M: MeshSource>(
        &mut self,
        device: &wgpu::Device,
        id: StrokeId,
        key: StrokeKey,
        build: impl FnOnce() -> Option<M>,
    ) -> bool {
        if self.cache.touch(id, key) {
            return false;
        }
        let Some(mesh) = build() else {
            self.cache.entries.remove(&id);
            return false;
        };
        self.cache.insert(id, key, upload(device, &mesh));
        self.generation += 1;
        true
    }

    /// Marks `id` dirty so its mesh is rebuilt on the next [`Self::ensure`].
    pub fn invalidate(&mut self, id: StrokeId) {
        self.cache.entries.remove(&id);
        self.generation += 1;
    }

    /// Drops every cached mesh.
    pub fn clear(&mut self) {
        self.cache.entries.clear();
        self.generation += 1;
    }

    /// A counter that changes whenever cached geometry is uploaded,
    /// invalidated or cleared. Callers can compare it between frames to know
    /// whether the canvas needs re-rendering.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Whether `id` has a cached mesh.
    pub fn contains(&self, id: StrokeId) -> bool {
        self.cache.entries.contains_key(&id)
    }

    /// Number of cached meshes.
    pub fn len(&self) -> usize {
        self.cache.entries.len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.cache.entries.is_empty()
    }

    /// Drops meshes not used in the last `max_unused` renders. Returns how many.
    pub fn evict_unused(&mut self, max_unused: u64) -> usize {
        self.cache.evict_unused(max_unused)
    }

    /// Advances the render clock used for eviction.
    pub(crate) fn tick(&mut self) {
        self.cache.tick();
    }

    pub(crate) fn get(&mut self, id: StrokeId) -> Option<&GpuMesh> {
        self.cache.get(id)
    }
}

fn upload(device: &wgpu::Device, mesh: &impl MeshSource) -> GpuMesh {
    let index_count = valid_index_count(mesh);
    if index_count == 0 {
        return GpuMesh {
            buffers: None,
            index_count: 0,
        };
    }
    let vertex_bytes: Vec<u8> = mesh
        .vertices()
        .iter()
        .flatten()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    let index_bytes: Vec<u8> = mesh.indices()[..index_count as usize]
        .iter()
        .flat_map(|i| i.to_le_bytes())
        .collect();
    let vertex = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("stroke-vertices"),
        contents: &vertex_bytes,
        usage: wgpu::BufferUsages::VERTEX,
    });
    let index = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("stroke-indices"),
        contents: &index_bytes,
        usage: wgpu::BufferUsages::INDEX,
    });
    GpuMesh {
        buffers: Some((vertex, index)),
        index_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use animation_core::{Rgba, Vec2};
    use drawing::{BrushStyle, StrokePoint};

    fn stroke(points: &[(f32, f32)], width: f32) -> Stroke {
        Stroke::new(
            points
                .iter()
                .map(|&(x, y)| StrokePoint::new(Vec2::new(x, y)))
                .collect(),
            BrushStyle {
                color: Rgba::BLACK,
                width,
                opacity: 1.0,
            },
        )
    }

    #[test]
    fn key_changes_with_geometry_inputs() {
        let a = stroke(&[(0.0, 0.0), (1.0, 1.0)], 2.0);
        assert_eq!(StrokeKey::of(&a), StrokeKey::of(&a.clone()));
        let mut grown = a.clone();
        grown.points.push(StrokePoint::new(Vec2::new(2.0, 2.0)));
        assert_ne!(StrokeKey::of(&a), StrokeKey::of(&grown));
        let mut wider = a.clone();
        wider.style.width = 3.0;
        assert_ne!(StrokeKey::of(&a), StrokeKey::of(&wider));
        let mut recolored = a.clone();
        recolored.style.color = Rgba::WHITE;
        assert_eq!(StrokeKey::of(&a), StrokeKey::of(&recolored));
    }

    #[test]
    fn touch_insert_and_dirty_key() {
        let mut cache = Cache::<u32>::default();
        let id = StrokeId::new();
        let k1 = StrokeKey::from_raw(1);
        assert!(!cache.touch(id, k1));
        cache.insert(id, k1, 7);
        assert!(cache.touch(id, k1));
        assert!(!cache.touch(id, StrokeKey::from_raw(2)), "new key is dirty");
        assert_eq!(cache.get(id), Some(&7));
    }

    #[test]
    fn eviction_drops_only_stale_entries() {
        let mut cache = Cache::<u32>::default();
        let (old, fresh) = (StrokeId::new(), StrokeId::new());
        let key = StrokeKey::from_raw(0);
        cache.insert(old, key, 1);
        cache.insert(fresh, key, 2);
        for _ in 0..10 {
            cache.tick();
            assert!(cache.touch(fresh, key));
        }
        assert_eq!(cache.evict_unused(10), 0);
        cache.tick();
        assert!(cache.touch(fresh, key));
        assert_eq!(cache.evict_unused(10), 1);
        assert!(cache.get(old).is_none());
        assert!(cache.get(fresh).is_some());
    }
}
