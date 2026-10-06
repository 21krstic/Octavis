use std::collections::HashMap;
use std::sync::Mutex;

use eframe::egui_wgpu::{self, wgpu, wgpu::util::DeviceExt};
use glam::{IVec3, Mat4};
use octavis_core::BlockTable;
use octavis_mesh::{Mesh, Vertex};

/// Depth format requested from eframe (`depth_buffer: 32`).
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const PALETTE_SIZE: usize = 256;

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

/// GPU state, stored in egui's callback resources.
struct Resources {
    pipeline: wgpu::RenderPipeline,
    globals_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    meshes: HashMap<IVec3, GpuMesh>,
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
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("voxel_pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
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
            targets: &[Some(render_state.target_format.into())],
        }),
        primitive: wgpu::PrimitiveState {
            cull_mode: Some(wgpu::Face::Back),
            front_face: wgpu::FrontFace::Ccw,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });

    render_state.renderer.write().callback_resources.insert(Resources {
        pipeline,
        globals_buffer,
        bind_group,
        meshes: HashMap::new(),
        srgb_target: render_state.target_format.is_srgb(),
    });
}

/// Per-frame callback. `uploads` carries only what changed.
pub struct ViewportCallback {
    pub view_proj: Mat4,
    pub palette: Mutex<Option<Vec<[f32; 3]>>>,
    /// `None` mesh means the section was removed.
    pub uploads: Mutex<Vec<(IVec3, Option<Mesh>)>>,
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
            match mesh {
                Some(m) if !m.indices.is_empty() => {
                    let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("section_vertices"),
                        contents: bytemuck::cast_slice(&m.vertices),
                        usage: wgpu::BufferUsages::VERTEX,
                    });
                    let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("section_indices"),
                        contents: bytemuck::cast_slice(&m.indices),
                        usage: wgpu::BufferUsages::INDEX,
                    });
                    res.meshes.insert(
                        pos,
                        GpuMesh { vertices, indices, index_count: m.indices.len() as u32 },
                    );
                }
                _ => {
                    res.meshes.remove(&pos);
                }
            }
        }

        let mut globals = Globals {
            view_proj: self.view_proj.to_cols_array_2d(),
            palette: [[1.0, 0.0, 1.0, 1.0]; PALETTE_SIZE],
        };
        // The matrix changes every frame; the 4 KB palette only when sent.
        if let Some(p) = self.palette.lock().unwrap().take() {
            for (i, c) in p.iter().take(PALETTE_SIZE).enumerate() {
                let c = if res.srgb_target { c.map(srgb_to_linear) } else { *c };
                globals.palette[i] = [c[0], c[1], c[2], 1.0];
            }
            queue.write_buffer(&res.globals_buffer, 0, bytemuck::bytes_of(&globals));
        } else {
            queue.write_buffer(&res.globals_buffer, 0, bytemuck::bytes_of(&globals.view_proj));
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
        pass.set_pipeline(&res.pipeline);
        pass.set_bind_group(0, &res.bind_group, &[]);
        for mesh in res.meshes.values() {
            pass.set_vertex_buffer(0, mesh.vertices.slice(..));
            pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.index_count, 0, 0..1);
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
