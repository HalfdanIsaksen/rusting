//! GPU vertex/index buffers for stroke, fill and image meshes, cached by
//! [`MeshId`], plus the textures of imported images and of brushes.
//!
//! A stroke is tessellated and uploaded once, then reused every frame until
//! its [`StrokeKey`] changes or it is invalidated. An image is decoded once
//! when first added and uploaded as a texture on the next render. A brushed
//! stroke (ADR-022) caches a quad per stamp instead of its outline, and its
//! brush's tip and grain textures are cached once per brush. Entries that
//! are not drawn for a while are evicted.

use crate::mesh::{MeshSource, valid_index_count};
use animation_core::{AssetId, FillId, ImageId, StrokeId};
use drawing::{BrushData, Fill, Image, Stamp, Stroke};
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
        stroke.style.brush.hash(&mut hasher);
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
    /// A brushed stroke's stamps (ADR-022): one textured quad per stamp,
    /// drawn with the brush pipeline.
    Brushed(StrokeId),
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
struct Cache<T, K = MeshId> {
    entries: HashMap<K, Entry<T>>,
    /// Incremented once per render.
    clock: u64,
}

impl<T, K> Default for Cache<T, K> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            clock: 0,
        }
    }
}

impl<T, K: Copy + Eq + std::hash::Hash> Cache<T, K> {
    /// Returns true if `id` has an entry with this `key`, marking it used.
    fn touch(&mut self, id: K, key: StrokeKey) -> bool {
        match self.entries.get_mut(&id) {
            Some(entry) if entry.key == key => {
                entry.last_used = self.clock;
                true
            }
            _ => false,
        }
    }

    fn insert(&mut self, id: K, key: StrokeKey, value: T) {
        self.entries.insert(
            id,
            Entry {
                key,
                last_used: self.clock,
                value,
            },
        );
    }

    fn get(&mut self, id: K) -> Option<&T> {
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

/// A brush's tip and grain pictures, decoded, waiting to be uploaded. A
/// brush without grain uploads its tip as the grain too (the shader then
/// ignores it).
#[derive(Debug)]
pub(crate) struct DecodedBrush {
    pub(crate) tip: DecodedImage,
    pub(crate) grain: Option<DecodedImage>,
}

/// A brush's textures, or why there are none yet.
#[derive(Debug)]
pub(crate) enum BrushTexture {
    /// Decoded, uploaded on the next render.
    Pending(DecodedBrush),
    /// Ready to draw: tip and grain at group 2.
    Ready(wgpu::BindGroup),
    /// The pictures could not be decoded; strokes with the brush are not
    /// drawn.
    Failed,
}

/// The quads of a brushed stroke's stamps: per vertex position (x, y),
/// tip texture coordinates (u, v) and stamp opacity.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StampMesh {
    /// `[x, y, u, v, opacity]` per vertex, four per stamp.
    pub vertices: Vec<[f32; 5]>,
    /// Two triangles per stamp.
    pub indices: Vec<u32>,
}

impl StampMesh {
    /// One quad per stamp, rotated by its angle.
    pub fn new(stamps: &[Stamp]) -> Self {
        let mut mesh = Self {
            vertices: Vec::with_capacity(stamps.len() * 4),
            indices: Vec::with_capacity(stamps.len() * 6),
        };
        for stamp in stamps {
            let half = stamp.size * 0.5;
            let (sin, cos) = stamp.angle.sin_cos();
            let (ax, ay) = (cos * half, sin * half);
            let (bx, by) = (-sin * half, cos * half);
            let base = mesh.vertices.len() as u32;
            for (su, sv) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                mesh.vertices.push([
                    stamp.pos.x + ax * su + bx * sv,
                    stamp.pos.y + ay * su + by * sv,
                    (su + 1.0) * 0.5,
                    (sv + 1.0) * 0.5,
                    stamp.opacity,
                ]);
            }
            mesh.indices
                .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        mesh
    }
}

/// Stroke and fill meshes uploaded to the GPU, keyed by [`MeshId`]
/// (a [`StrokeId`], [`FillId`] or [`ImageId`] converts into one), and image
/// textures keyed by image.
#[derive(Debug, Default)]
pub struct StrokeCache {
    cache: Cache<GpuMesh>,
    textures: Cache<ImageTexture>,
    brushes: Cache<BrushTexture, AssetId>,
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

    /// Makes sure brushed `stroke` has an up-to-date stamp mesh, drawn with
    /// `brush` (library asset `asset`), and that the brush has textures. A
    /// new or changed brush is decoded now and uploaded by the next render.
    pub fn ensure_brushed(
        &mut self,
        device: &wgpu::Device,
        stroke: &Stroke,
        asset: AssetId,
        brush: &BrushData,
    ) {
        let id = MeshId::Brushed(stroke.id);
        let key = StrokeKey::of(stroke);
        if !self.cache.touch(id, key) {
            let mesh = StampMesh::new(&drawing::stamps(stroke, &brush.settings));
            let index_bytes: Vec<u8> = mesh.indices.iter().flat_map(|i| i.to_le_bytes()).collect();
            let vertex_bytes: Vec<u8> = mesh
                .vertices
                .iter()
                .flatten()
                .flat_map(|v| v.to_le_bytes())
                .collect();
            let gpu = upload_bytes(device, &vertex_bytes, &index_bytes, mesh.indices.len());
            self.cache.insert(id, key, gpu);
            self.generation += 1;
        }
        let key = brush_key(brush);
        if !self.brushes.touch(asset, key) {
            let max_size = device.limits().max_texture_dimension_2d;
            let tip = decode_image(&brush.tip.bytes, max_size);
            let grain = brush
                .grain
                .as_ref()
                .map(|g| decode_image(&g.bytes, max_size));
            let texture = match (tip, grain) {
                (Some(tip), None) => BrushTexture::Pending(DecodedBrush { tip, grain: None }),
                (Some(tip), Some(Some(grain))) => BrushTexture::Pending(DecodedBrush {
                    tip,
                    grain: Some(grain),
                }),
                _ => {
                    tracing::warn!("could not decode brush {asset}");
                    BrushTexture::Failed
                }
            };
            self.brushes.insert(asset, key, texture);
            self.generation += 1;
        }
    }

    /// Brush textures still waiting for upload, for the renderer.
    pub(crate) fn pending_brushes(&mut self) -> impl Iterator<Item = &mut BrushTexture> {
        self.brushes
            .entries
            .values_mut()
            .map(|entry| &mut entry.value)
            .filter(|t| matches!(t, BrushTexture::Pending(_)))
    }

    /// The bind group of a brush's uploaded tip and grain.
    pub(crate) fn brush_texture(&mut self, asset: AssetId) -> Option<&wgpu::BindGroup> {
        match self.brushes.get(asset)? {
            BrushTexture::Ready(bind_group) => Some(bind_group),
            BrushTexture::Pending(_) | BrushTexture::Failed => None,
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
        if let MeshId::Stroke(stroke) = id {
            self.cache.entries.remove(&MeshId::Brushed(stroke));
        }
        self.generation += 1;
    }

    /// Drops every cached mesh.
    pub fn clear(&mut self) {
        self.cache.entries.clear();
        self.textures.entries.clear();
        self.brushes.entries.clear();
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
        self.brushes.evict_unused(max_unused);
        self.cache.evict_unused(max_unused)
    }

    /// Advances the render clock used for eviction.
    pub(crate) fn tick(&mut self) {
        self.cache.tick();
        self.textures.tick();
        self.brushes.tick();
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

/// Fingerprint of a brush's pictures and settings. Brush bytes are never
/// edited in place, so the shared buffers' addresses identify them.
fn brush_key(brush: &BrushData) -> StrokeKey {
    let mut hasher = DefaultHasher::new();
    for data in std::iter::once(&brush.tip).chain(&brush.grain) {
        data.width.hash(&mut hasher);
        data.height.hash(&mut hasher);
        data.bytes.len().hash(&mut hasher);
        std::ptr::hash(data.bytes.as_ptr(), &mut hasher);
    }
    StrokeKey(hasher.finish())
}

fn upload(device: &wgpu::Device, mesh: &impl MeshSource) -> GpuMesh {
    let index_count = valid_index_count(mesh);
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
    upload_bytes(device, &vertex_bytes, &index_bytes, index_count as usize)
}

/// Uploads vertex and index bytes (`index_count` `u32` indices).
fn upload_bytes(
    device: &wgpu::Device,
    vertex_bytes: &[u8],
    index_bytes: &[u8],
    index_count: usize,
) -> GpuMesh {
    let Ok(index_count) = u32::try_from(index_count) else {
        return GpuMesh {
            buffers: None,
            index_count: 0,
        };
    };
    if index_count == 0 || vertex_bytes.is_empty() {
        return GpuMesh {
            buffers: None,
            index_count: 0,
        };
    }
    let vertex = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("stroke-vertices"),
        contents: vertex_bytes,
        usage: wgpu::BufferUsages::VERTEX,
    });
    let index = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("stroke-indices"),
        contents: index_bytes,
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
                brush: None,
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
    fn brush_changes_the_key() {
        let a = stroke(&[(0.0, 0.0), (1.0, 1.0)], 2.0);
        let mut brushed = a.clone();
        brushed.style.brush = Some(AssetId::new());
        assert_ne!(StrokeKey::of(&a), StrokeKey::of(&brushed));
    }

    #[test]
    fn stamp_quads_are_centred_and_rotated() {
        let mesh = StampMesh::new(&[Stamp {
            pos: Vec2::new(10.0, 20.0),
            size: 4.0,
            angle: std::f32::consts::FRAC_PI_2,
            opacity: 0.5,
        }]);
        assert_eq!(mesh.indices, [0, 1, 2, 0, 2, 3]);
        let close =
            |v: [f32; 5], x: f32, y: f32| (v[0] - x).abs() < 1e-5 && (v[1] - y).abs() < 1e-5;
        // A quarter turn maps the tip's (-1, -1) corner to (+2, -2).
        assert!(
            close(mesh.vertices[0], 12.0, 18.0),
            "{:?}",
            mesh.vertices[0]
        );
        assert_eq!((mesh.vertices[0][2], mesh.vertices[0][3]), (0.0, 0.0));
        assert_eq!((mesh.vertices[2][2], mesh.vertices[2][3]), (1.0, 1.0));
        assert!(mesh.vertices.iter().all(|v| v[4] == 0.5));
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
