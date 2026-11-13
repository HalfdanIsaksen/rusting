//! GPU vertex/index buffers for stroke, fill and image meshes, cached by
//! [`MeshId`], plus the textures of imported images.
//!
//! A stroke is tessellated and uploaded once, then reused every frame until
//! its [`StrokeKey`] changes or it is invalidated. An image is decoded once
//! when first added and uploaded as a texture on the next render. Entries
//! that are not drawn for a while are evicted.

use crate::mesh::{MeshSource, valid_index_count};
use animation_core::{FillId, ImageId, StrokeId};
use drawing::{Fill, Image, Stroke};
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

    /// Fingerprint of `fill`'s geometry: point counts and the first and last
    /// point of every contour. Fills are not edited in place (recolouring
    /// replaces the fill), so this is enough.
    pub fn of_fill(fill: &Fill) -> Self {
        let mut hasher = DefaultHasher::new();
        fill.contours.len().hash(&mut hasher);
        for contour in &fill.contours {
            contour.len().hash(&mut hasher);
            for p in [contour.first(), contour.last()].into_iter().flatten() {
                p.x.to_bits().hash(&mut hasher);
                p.y.to_bits().hash(&mut hasher);
            }
        }
        Self(hasher.finish())
    }

    /// Fingerprint of where `image` sits (its quad's corners).
    pub fn of_image(image: &Image) -> Self {
        let mut hasher = DefaultHasher::new();
        for v in image.transform.to_cols_array() {
            v.to_bits().hash(&mut hasher);
        }
        Self(hasher.finish())
    }

    /// Fingerprint of an image's picture: its size and which bytes it holds.
    /// Image bytes are never edited in place, so the shared buffer's address
    /// and length identify them.
    fn of_image_data(image: &Image) -> Self {
        let mut hasher = DefaultHasher::new();
        image.data.width.hash(&mut hasher);
        image.data.height.hash(&mut hasher);
        image.data.bytes.len().hash(&mut hasher);
        std::ptr::hash(image.data.bytes.as_ptr(), &mut hasher);
        Self(hasher.finish())
    }

    /// A key from a caller-chosen value (for meshes that are not strokes).
    pub const fn from_raw(value: u64) -> Self {
        Self(value)
    }
}

/// What a cached mesh belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MeshId {
    /// A stroke's mesh.
    Stroke(StrokeId),
    /// A fill's mesh.
    Fill(FillId),
    /// An image's quad (four corners: top-left, top-right, bottom-right,
    /// bottom-left, so the shader can derive texture coordinates from the
    /// vertex index).
    Image(ImageId),
}

impl From<ImageId> for MeshId {
    fn from(id: ImageId) -> Self {
        Self::Image(id)
    }
}

impl From<StrokeId> for MeshId {
    fn from(id: StrokeId) -> Self {
        Self::Stroke(id)
    }
}

impl From<FillId> for MeshId {
    fn from(id: FillId) -> Self {
        Self::Fill(id)
    }
}

/// Uploaded geometry of one stroke or fill.
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
    entries: HashMap<MeshId, Entry<T>>,
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
    fn touch(&mut self, id: MeshId, key: StrokeKey) -> bool {
        match self.entries.get_mut(&id) {
            Some(entry) if entry.key == key => {
                entry.last_used = self.clock;
                true
            }
            _ => false,
        }
    }

    fn insert(&mut self, id: MeshId, key: StrokeKey, value: T) {
        self.entries.insert(
            id,
            Entry {
                key,
                last_used: self.clock,
                value,
            },
        );
    }

    fn get(&mut self, id: MeshId) -> Option<&T> {
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

/// Decoded pixels of an image, waiting to be uploaded: RGBA8 mip levels,
/// largest first.
#[derive(Debug)]
pub(crate) struct DecodedImage {
    pub(crate) levels: Vec<(u32, u32, Vec<u8>)>,
}

/// An image's texture, or why there is none yet.
#[derive(Debug)]
pub(crate) enum ImageTexture {
    /// Decoded, uploaded on the next render.
    Pending(DecodedImage),
    /// Ready to draw.
    Ready(wgpu::BindGroup),
    /// The bytes could not be decoded; the image is not drawn.
    Failed,
}

/// Stroke and fill meshes uploaded to the GPU, keyed by [`MeshId`]
/// (a [`StrokeId`], [`FillId`] or [`ImageId`] converts into one), and image
/// textures keyed by image.
#[derive(Debug, Default)]
pub struct StrokeCache {
    cache: Cache<GpuMesh>,
    textures: Cache<ImageTexture>,
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
        id: impl Into<MeshId>,
        key: StrokeKey,
        build: impl FnOnce() -> Option<M>,
    ) -> bool {
        let id = id.into();
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

    /// Makes sure `image` has an up-to-date quad mesh and a texture. A new
    /// or changed picture is decoded now (`max_size` caps its larger side,
    /// see [`wgpu::Limits::max_texture_dimension_2d`]) and uploaded by the
    /// next render.
    pub fn ensure_image(&mut self, device: &wgpu::Device, image: &Image) {
        let id = MeshId::Image(image.id);
        self.ensure(device, id, StrokeKey::of_image(image), || {
            Some(crate::mesh::Mesh {
                vertices: image.corners().map(|c| [c.x, c.y]).to_vec(),
                indices: vec![0, 1, 2, 0, 2, 3],
            })
        });
        let key = StrokeKey::of_image_data(image);
        if !self.textures.touch(id, key) {
            let max_size = device.limits().max_texture_dimension_2d;
            let texture = match decode_image(&image.data.bytes, max_size) {
                Some(decoded) => ImageTexture::Pending(decoded),
                None => {
                    tracing::warn!("could not decode image {}", image.id);
                    ImageTexture::Failed
                }
            };
            self.textures.insert(id, key, texture);
            self.generation += 1;
        }
    }

    /// Image textures still waiting for upload, for the renderer.
    pub(crate) fn pending_textures(&mut self) -> impl Iterator<Item = &mut ImageTexture> {
        self.textures
            .entries
            .values_mut()
            .map(|entry| &mut entry.value)
            .filter(|t| matches!(t, ImageTexture::Pending(_)))
    }

    /// The bind group of an uploaded image texture.
    pub(crate) fn texture(&mut self, id: MeshId) -> Option<&wgpu::BindGroup> {
        match self.textures.get(id)? {
            ImageTexture::Ready(bind_group) => Some(bind_group),
            ImageTexture::Pending(_) | ImageTexture::Failed => None,
        }
    }

    /// Marks `id` dirty so its mesh is rebuilt on the next [`Self::ensure`].
    pub fn invalidate(&mut self, id: impl Into<MeshId>) {
        let id = id.into();
        self.cache.entries.remove(&id);
        self.textures.entries.remove(&id);
        self.generation += 1;
    }

    /// Drops every cached mesh.
    pub fn clear(&mut self) {
        self.cache.entries.clear();
        self.textures.entries.clear();
        self.generation += 1;
    }

    /// A counter that changes whenever cached geometry is uploaded,
    /// invalidated or cleared. Callers can compare it between frames to know
    /// whether the canvas needs re-rendering.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Whether `id` has a cached mesh.
    pub fn contains(&self, id: impl Into<MeshId>) -> bool {
        self.cache.entries.contains_key(&id.into())
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
        self.textures.evict_unused(max_unused);
        self.cache.evict_unused(max_unused)
    }

    /// Advances the render clock used for eviction.
    pub(crate) fn tick(&mut self) {
        self.cache.tick();
        self.textures.tick();
    }

    pub(crate) fn get(&mut self, id: MeshId) -> Option<&GpuMesh> {
        self.cache.get(id)
    }
}

/// Decodes a PNG/JPEG into RGBA8 mip levels (largest first, down to 1x1),
/// shrinking it first if its larger side is above `max_size`.
pub(crate) fn decode_image(bytes: &[u8], max_size: u32) -> Option<DecodedImage> {
    use image::imageops::FilterType;
    let mut rgba = image::load_from_memory(bytes).ok()?.into_rgba8();
    let max_size = max_size.max(1);
    if rgba.width() > max_size || rgba.height() > max_size {
        let scale = max_size as f32 / rgba.width().max(rgba.height()) as f32;
        let w = ((rgba.width() as f32 * scale) as u32).clamp(1, max_size);
        let h = ((rgba.height() as f32 * scale) as u32).clamp(1, max_size);
        rgba = image::imageops::resize(&rgba, w, h, FilterType::Triangle);
    }
    if rgba.width() == 0 || rgba.height() == 0 {
        return None;
    }
    let mut levels = Vec::new();
    loop {
        let (w, h) = rgba.dimensions();
        let next = (w > 1 || h > 1).then(|| {
            image::imageops::resize(&rgba, (w / 2).max(1), (h / 2).max(1), FilterType::Triangle)
        });
        levels.push((w, h, rgba.into_raw()));
        match next {
            Some(smaller) => rgba = smaller,
            None => break,
        }
    }
    Some(DecodedImage { levels })
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
        let id = MeshId::from(StrokeId::new());
        let k1 = StrokeKey::from_raw(1);
        assert!(!cache.touch(id, k1));
        cache.insert(id, k1, 7);
        assert!(cache.touch(id, k1));
        assert!(!cache.touch(id, StrokeKey::from_raw(2)), "new key is dirty");
        assert_eq!(cache.get(id), Some(&7));
    }

    #[test]
    fn decoding_builds_a_mip_chain_and_caps_the_size() {
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(8, 4, image::Rgba([255, 0, 0, 128]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let decoded = decode_image(&png, 4096).unwrap();
        let sizes: Vec<_> = decoded.levels.iter().map(|l| (l.0, l.1)).collect();
        assert_eq!(sizes, [(8, 4), (4, 2), (2, 1), (1, 1)]);
        assert_eq!(&decoded.levels[0].2[..4], &[255, 0, 0, 128]);
        let capped = decode_image(&png, 2).unwrap();
        assert_eq!((capped.levels[0].0, capped.levels[0].1), (2, 1));
        assert!(decode_image(b"not an image", 4096).is_none());
    }

    #[test]
    fn eviction_drops_only_stale_entries() {
        let mut cache = Cache::<u32>::default();
        let (old, fresh) = (MeshId::from(StrokeId::new()), MeshId::from(FillId::new()));
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
