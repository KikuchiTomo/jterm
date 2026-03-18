// jterm image renderer
//
// Draws inline terminal images (Kitty/iTerm2/Sixel) as textured quads.
// Each image placement is drawn as a single quad covering a cell region.

struct ImageUniforms {
    // Quad position in NDC: (x, y) = top-left corner
    quad_pos: vec2<f32>,
    // Quad size in NDC: (width, height)
    quad_size: vec2<f32>,
}

@group(0) @binding(0) var<uniform> uniforms: ImageUniforms;
@group(0) @binding(1) var image_texture: texture_2d<f32>;
@group(0) @binding(2) var image_sampler: sampler;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

// 6 vertices for a quad (two triangles)
var<private> QUAD_POS: array<vec2<f32>, 6> = array<vec2<f32>, 6>(
    vec2<f32>(0.0, 0.0),
    vec2<f32>(1.0, 0.0),
    vec2<f32>(0.0, 1.0),
    vec2<f32>(1.0, 0.0),
    vec2<f32>(1.0, 1.0),
    vec2<f32>(0.0, 1.0),
);

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    let quad = QUAD_POS[vertex_index];

    // Position the quad in NDC space.
    let pos = vec2<f32>(
        uniforms.quad_pos.x + quad.x * uniforms.quad_size.x,
        uniforms.quad_pos.y - quad.y * uniforms.quad_size.y,
    );

    var out: VertexOutput;
    out.position = vec4<f32>(pos, 0.0, 1.0);
    out.uv = quad;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let color = textureSample(image_texture, image_sampler, in.uv);
    // Premultiplied alpha blending: image over existing content.
    return color;
}
