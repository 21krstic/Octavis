//! CPU meshing of voxel sections. Pure data out (vertices + indices); no
//! graphics API here, so the renderer, OBJ export and tests all share it.
//!
//! Culled meshing with per-vertex ambient occlusion (see README: greedy
//! meshing conflicts with AO and varied textures). Every non-air block is
//! treated as an opaque full cube until the block registry knows shapes.

use bytemuck::{Pod, Zeroable};
use glam::IVec3;
use octavis_core::{BlockId, SECTION_SIZE, World};

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct Vertex {
    /// World-space position.
    pub position: [f32; 3],
    /// Face index into [`FACES`]: +X, -X, +Y, -Y, +Z, -Z.
    pub face: u32,
    /// Raw `BlockId`; the renderer maps it to a colour or texture.
    pub block: u32,
    /// Ambient occlusion, 0 (darkest) to 3 (unoccluded).
    pub ao: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

pub struct Face {
    pub normal: IVec3,
    /// Tangent axes with `u x v == normal`, so (0,0)->(1,0)->(1,1) is
    /// counter-clockwise seen from outside.
    pub u: IVec3,
    pub v: IVec3,
}

const fn face(normal: [i32; 3], u: [i32; 3], v: [i32; 3]) -> Face {
    Face { normal: IVec3::from_array(normal), u: IVec3::from_array(u), v: IVec3::from_array(v) }
}

pub const FACES: [Face; 6] = [
    face([1, 0, 0], [0, 1, 0], [0, 0, 1]),
    face([-1, 0, 0], [0, 0, 1], [0, 1, 0]),
    face([0, 1, 0], [0, 0, 1], [1, 0, 0]),
    face([0, -1, 0], [1, 0, 0], [0, 0, 1]),
    face([0, 0, 1], [1, 0, 0], [0, 1, 0]),
    face([0, 0, -1], [0, 1, 0], [1, 0, 0]),
];

/// Meshes one section. Faces on the section border consult neighbouring
/// sections through `world`, so no seams or hidden internal faces appear.
pub fn mesh_section(world: &World, section: IVec3) -> Mesh {
    let mut mesh = Mesh::default();
    let Some(sec) = world.section(section) else { return mesh };
    let origin = section * SECTION_SIZE as i32;
    let solid = |p: IVec3| world.get(p) != BlockId::AIR;

    for y in 0..SECTION_SIZE {
        for z in 0..SECTION_SIZE {
            for x in 0..SECTION_SIZE {
                let block = sec.get(x, y, z);
                if block == BlockId::AIR {
                    continue;
                }
                let pos = origin + IVec3::new(x as i32, y as i32, z as i32);
                for (fi, f) in FACES.iter().enumerate() {
                    let front = pos + f.normal;
                    if solid(front) {
                        continue;
                    }
                    emit_face(&mut mesh, fi, f, pos, front, block, &solid);
                }
            }
        }
    }
    mesh
}

fn emit_face(
    mesh: &mut Mesh,
    fi: usize,
    f: &Face,
    pos: IVec3,
    front: IVec3,
    block: BlockId,
    solid: &impl Fn(IVec3) -> bool,
) {
    // Faces with a negative normal sit on the block's min side.
    let base = if f.normal.x + f.normal.y + f.normal.z > 0 { pos + f.normal } else { pos };
    let corners = [(0, 0), (1, 0), (1, 1), (0, 1)];
    let mut ao = [0u32; 4];
    let first = mesh.vertices.len() as u32;

    for (i, &(a, b)) in corners.iter().enumerate() {
        let du = if a == 1 { f.u } else { -f.u };
        let dv = if b == 1 { f.v } else { -f.v };
        let (s1, s2, c) = (solid(front + du), solid(front + dv), solid(front + du + dv));
        ao[i] = if s1 && s2 { 0 } else { 3 - (s1 as u32 + s2 as u32 + c as u32) };
        let p = base + f.u * a + f.v * b;
        mesh.vertices.push(Vertex {
            position: p.as_vec3().to_array(),
            face: fi as u32,
            block: block.0,
            ao: ao[i],
        });
    }

    // Split along the diagonal with the smaller AO difference to avoid
    // the anisotropy artefact.
    let tris: [u32; 6] = if ao[0] + ao[2] >= ao[1] + ao[3] {
        [0, 1, 2, 0, 2, 3]
    } else {
        [1, 2, 3, 1, 3, 0]
    };
    mesh.indices.extend(tris.iter().map(|t| first + t));
}

#[cfg(test)]
mod tests {
    use super::*;
    use octavis_core::BlockState;

    fn world_with(blocks: &[IVec3]) -> World {
        let mut w = World::new();
        let stone = w.blocks.intern(BlockState::new("minecraft:stone"));
        for &b in blocks {
            w.set(b, stone);
        }
        w
    }

    fn faces(mesh: &Mesh) -> usize {
        mesh.indices.len() / 6
    }

    #[test]
    fn single_block_has_six_faces() {
        let w = world_with(&[IVec3::new(2, 2, 2)]);
        let m = mesh_section(&w, IVec3::ZERO);
        assert_eq!(faces(&m), 6);
        assert_eq!(m.vertices.len(), 24);
        assert!(m.vertices.iter().all(|v| v.ao == 3));
    }

    #[test]
    fn adjacent_blocks_hide_shared_faces() {
        let w = world_with(&[IVec3::new(2, 2, 2), IVec3::new(3, 2, 2)]);
        assert_eq!(faces(&mesh_section(&w, IVec3::ZERO)), 10);
    }

    #[test]
    fn neighbour_section_culls_border_faces() {
        // Blocks either side of the x=15 | x=16 section border.
        let w = world_with(&[IVec3::new(15, 0, 0), IVec3::new(16, 0, 0)]);
        assert_eq!(faces(&mesh_section(&w, IVec3::ZERO)), 5);
        assert_eq!(faces(&mesh_section(&w, IVec3::X)), 5);
    }

    #[test]
    fn missing_section_is_empty() {
        let w = World::new();
        assert!(mesh_section(&w, IVec3::ZERO).indices.is_empty());
    }

    #[test]
    fn winding_is_counter_clockwise_outward() {
        let w = world_with(&[IVec3::new(5, 5, 5)]);
        let m = mesh_section(&w, IVec3::ZERO);
        for tri in m.indices.chunks(3) {
            let p: Vec<glam::Vec3> =
                tri.iter().map(|&i| glam::Vec3::from(m.vertices[i as usize].position)).collect();
            let n = (p[1] - p[0]).cross(p[2] - p[0]);
            let expected = FACES[m.vertices[tri[0] as usize].face as usize].normal.as_vec3();
            assert!(n.dot(expected) > 0.0, "triangle faces inward");
        }
    }

    #[test]
    fn ao_darkens_concave_corner() {
        // Floor block with a wall block beside it: the top face vertices
        // touching the wall are occluded.
        let w = world_with(&[IVec3::new(5, 5, 5), IVec3::new(6, 6, 5)]);
        let m = mesh_section(&w, IVec3::ZERO);
        let darkest =
            m.vertices.iter().filter(|v| v.face == 2 && v.block != 0).map(|v| v.ao).min();
        assert_eq!(darkest, Some(2));
    }
}
