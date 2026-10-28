//! Triangle meshes the renderer accepts as stroke geometry.
//!
//! The renderer does not tessellate. Strokes are turned into triangles by the
//! `drawing` crate; anything that exposes a vertex and index slice can be
//! handed to the renderer through [`MeshSource`].

/// A triangle list: positions in layer (document) coordinates and indices
/// into them, three per triangle.
pub trait MeshSource {
    /// Vertex positions.
    fn vertices(&self) -> &[[f32; 2]];
    /// Triangle indices into [`Self::vertices`].
    fn indices(&self) -> &[u32];
}

/// An owned triangle mesh.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    /// Vertex positions in layer (document) coordinates.
    pub vertices: Vec<[f32; 2]>,
    /// Triangle indices, three per triangle.
    pub indices: Vec<u32>,
}

impl MeshSource for Mesh {
    fn vertices(&self) -> &[[f32; 2]] {
        &self.vertices
    }

    fn indices(&self) -> &[u32] {
        &self.indices
    }
}

/// The `drawing` crate's tessellator output can be uploaded directly.
impl MeshSource for drawing::Mesh {
    fn vertices(&self) -> &[[f32; 2]] {
        &self.vertices
    }

    fn indices(&self) -> &[u32] {
        &self.indices
    }
}

/// A borrowed triangle mesh.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeshRef<'a> {
    /// Vertex positions in layer (document) coordinates.
    pub vertices: &'a [[f32; 2]],
    /// Triangle indices, three per triangle.
    pub indices: &'a [u32],
}

impl MeshSource for MeshRef<'_> {
    fn vertices(&self) -> &[[f32; 2]] {
        self.vertices
    }

    fn indices(&self) -> &[u32] {
        self.indices
    }
}

impl<M: MeshSource + ?Sized> MeshSource for &M {
    fn vertices(&self) -> &[[f32; 2]] {
        (**self).vertices()
    }

    fn indices(&self) -> &[u32] {
        (**self).indices()
    }
}

/// Number of indices that form whole triangles and point at existing
/// vertices. Returns 0 if any index is out of range, so a bad mesh is skipped
/// instead of reading garbage on the GPU.
pub(crate) fn valid_index_count(mesh: &impl MeshSource) -> u32 {
    let vertex_count = mesh.vertices().len();
    let indices = mesh.indices();
    let whole = indices.len() - indices.len() % 3;
    if indices[..whole]
        .iter()
        .any(|&i| usize::try_from(i).map_or(true, |i| i >= vertex_count))
    {
        return 0;
    }
    u32::try_from(whole).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_validation() {
        let mut mesh = Mesh {
            vertices: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
            indices: vec![0, 1, 2],
        };
        assert_eq!(valid_index_count(&mesh), 3);
        mesh.indices.push(1); // trailing partial triangle is dropped
        assert_eq!(valid_index_count(&mesh), 3);
        mesh.indices = vec![0, 1, 3];
        assert_eq!(valid_index_count(&mesh), 0);
        assert_eq!(valid_index_count(&Mesh::default()), 0);
        let borrowed = MeshRef {
            vertices: &mesh.vertices,
            indices: &[2, 1, 0],
        };
        assert_eq!(valid_index_count(&borrowed), 3);
    }
}
