use std::collections::HashMap;
use std::sync::Mutex;

use eframe::egui_wgpu::{self, wgpu, wgpu::util::DeviceExt};
use glam::{IVec3, Mat4};
use octavis_core::BlockTable;
use octavis_mesh::{Mesh, Vertex};

/// Depth format requested from eframe (`depth_buffer: 32`).
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const PALETTE_SIZE: usize = 256;

/// Palette slots 250..=255 are reserved for overlays, so real blocks are
/// limited to ids below 250 until texture packs replace this palette.
pub const OVERLAY_SELECTION: u32 = 250;
pub const OVERLAY_PREVIEW: u32 = 251;
pub const OVERLAY_HOVER: u32 = 252;
pub const MAX_BLOCK_IDS: usize = 250;

/// Overlay mesh slots: the committed selection and the hover highlight are
/// separate so moving the mouse never re-meshes a large selection.
pub const SLOT_SELECTION: usize = 0;
pub const SLOT_HOVER: usize = 1;
const SLOTS: usize = 2;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    view_proj: [[f32; 4]; 4],
    palette: [[f32; 4]; PALETTE_SIZE],
}

struct GpuMesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
}

impl GpuMesh {
    fn upload(device: &wgpu::Device, m: &Mesh) -> Option<Self> {
        if m.indices.is_empty() {
            return None;
        }
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh_vertices"),
            contents: bytemuck::cast_slice(&m.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh_indices"),
            contents: bytemuck::cast_slice(&m.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        Some(Self { vertices, indices, index_count: m.indices.len() as u32 })
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'static>) {
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..self.index_count, 0, 0..1);
    }
}

/// GPU state, stored in egui's callback resources.
struct Resources {
    pipeline: wgpu::RenderPipeline,
    overlay_pipeline: wgpu::RenderPipeline,
    globals_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    meshes: HashMap<IVec3, GpuMesh>,
    overlays: [Option<GpuMesh>; SLOTS],
    srgb_target: bool,
}

pub fn init(render_state: &egui_wgpu::RenderState) {
    let device = &render_state.device;
    let shader = device.create_shader_module(wgpu::include_wgsl!("shader.wgsl"));

    let globals_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("globals"),
        size: std::mem::size_of::<Globals>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("globals_layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("globals_bind_group"),
        layout: &bind_group_layout,
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals_buffer.as_entire_binding() }],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("voxel_layout"),
        bind_group_layouts: &[Some(&bind_group_layout)],
        immediate_size: 0,
    });

    let attributes = wgpu::vertex_attr_array![0 => Float32x3, 1 => Uint32, 2 => Uint32, 3 => Uint32];
    let make_pipeline = |label: &str, vs: &str, blend: Option<wgpu::BlendState>, solid: bool| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some(vs),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attributes,
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: render_state.target_format,
                    blend,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: Some(wgpu::Face::Back),
                front_face: wgpu::FrontFace::Ccw,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(solid),
                // Overlay faces are coplanar with block faces, so allow equal depth.
                depth_compare: Some(if solid {
                    wgpu::CompareFunction::Less
                } else {
                    wgpu::CompareFunction::LessEqual
                }),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        })
    };
    let pipeline = make_pipeline("voxel_pipeline", "vs_main", None, true);
    let overlay_pipeline =
        make_pipeline("overlay_pipeline", "vs_overlay", Some(wgpu::BlendState::ALPHA_BLENDING), false);

    render_state.renderer.write().callback_resources.insert(Resources {
        pipeline,
        overlay_pipeline,
        globals_buffer,
        bind_group,
        meshes: HashMap::new(),
        overlays: [None, None],
        srgb_target: render_state.target_format.is_srgb(),
    });
}

/// Per-frame callback. `uploads` carries only what changed.
pub struct ViewportCallback {
    pub view_proj: Mat4,
    pub palette: Mutex<Option<Vec<[f32; 3]>>>,
    /// `None` mesh means the section was removed.
    pub uploads: Mutex<Vec<(IVec3, Option<Mesh>)>>,
    /// Replaces an overlay slot; an empty mesh clears it.
    pub overlay_uploads: Mutex<Vec<(usize, Mesh)>>,
}

impl egui_wgpu::CallbackTrait for ViewportCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let res: &mut Resources = resources.get_mut().expect("render::init not called");

        for (pos, mesh) in self.uploads.lock().unwrap().drain(..) {
            match mesh.as_ref().and_then(|m| GpuMesh::upload(device, m)) {
                Some(gpu) => {
                    res.meshes.insert(pos, gpu);
                }
                None => {
                    res.meshes.remove(&pos);
                }
            }
        }
        for (slot, mesh) in self.overlay_uploads.lock().unwrap().drain(..) {
            res.overlays[slot] = GpuMesh::upload(device, &mesh);
        }

        // The matrix changes every frame; the 4 KB palette only when sent.
        let view_proj = self.view_proj.to_cols_array_2d();
        match self.palette.lock().unwrap().take() {
            Some(colors) => {
                let conv = |c: [f32; 3], a: f32| {
                    let c = if res.srgb_target { c.map(srgb_to_linear) } else { c };
                    [c[0], c[1], c[2], a]
                };
                let mut globals = Globals { view_proj, palette: [[1.0, 0.0, 1.0, 1.0]; PALETTE_SIZE] };
                for (i, c) in colors.iter().take(MAX_BLOCK_IDS).enumerate() {
                    globals.palette[i] = conv(*c, 1.0);
                }
                globals.palette[OVERLAY_SELECTION as usize] = conv([0.25, 0.6, 1.0], 0.35);
                globals.palette[OVERLAY_PREVIEW as usize] = conv([1.0, 0.8, 0.2], 0.4);
                globals.palette[OVERLAY_HOVER as usize] = conv([1.0, 1.0, 1.0], 0.3);
                queue.write_buffer(&res.globals_buffer, 0, bytemuck::bytes_of(&globals));
            }
            None => queue.write_buffer(&res.globals_buffer, 0, bytemuck::bytes_of(&view_proj)),
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: eframe::egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        let res: &Resources = resources.get().expect("render::init not called");
        pass.set_bind_group(0, &res.bind_group, &[]);
        pass.set_pipeline(&res.pipeline);
        for mesh in res.meshes.values() {
            mesh.draw(pass);
        }
        pass.set_pipeline(&res.overlay_pipeline);
        for mesh in res.overlays.iter().flatten() {
            mesh.draw(pass);
        }
    }
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// Placeholder colours until texture packs exist (milestone 7).
pub fn palette_colors(blocks: &BlockTable) -> Vec<[f32; 3]> {
    (0..blocks.len())
        .map(|i| {
            let name = &blocks.get(octavis_core::BlockId(i as u32)).unwrap().name;
            let rgb: [u8; 3] = match name.as_str() {
                "minecraft:grass_block" => [95, 159, 53],
                "minecraft:dirt" => [134, 96, 67],
                "minecraft:stone" => [125, 125, 125],
                "minecraft:oak_planks" => [162, 130, 78],
                "minecraft:sand" => [219, 207, 142],
                "minecraft:bricks" => [150, 97, 83],
                _ => hash_color(name),
            };
            rgb.map(|c| c as f32 / 255.0)
        })
        .collect()
}

fn hash_color(name: &str) -> [u8; 3] {
    let h = name.bytes().fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619));
    [(h & 255) as u8, ((h >> 8) & 255) as u8, ((h >> 16) & 255) as u8]
}
