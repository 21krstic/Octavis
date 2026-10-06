struct Globals {
    view_proj: mat4x4<f32>,
    palette: array<vec4<f32>, 256>,
};

@group(0) @binding(0) var<uniform> globals: Globals;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) face: u32,
    @location(2) block: u32,
    @location(3) ao: u32,
};

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec3<f32>,
};

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    // Face order matches octavis_mesh::FACES: +X, -X, +Y, -Y, +Z, -Z.
    var face_light = array<f32, 6>(0.8, 0.8, 1.0, 0.5, 0.65, 0.65);
    var ao_light = array<f32, 4>(0.45, 0.65, 0.85, 1.0);

    var out: VertexOut;
    out.clip = globals.view_proj * vec4<f32>(in.position, 1.0);
    let base = globals.palette[in.block & 255u].rgb;
    out.color = base * face_light[in.face] * ao_light[in.ao];
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color, 1.0);
}
