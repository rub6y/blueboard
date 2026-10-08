struct CameraUniform {
    view_proj: mat4x4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: CameraUniform;

struct VertexInput {
    @location(0) position: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = camera.view_proj * vec4<f32>(in.position, 0.0, 1.0);
    return out;
}

@fragment
fn fs_grid(in: VertexOutput) -> @location(0) vec4<f32> {
    // Muted blue-grey, visible against the board background without competing with
    // actual strokes.
    return vec4<f32>(0.3, 0.38, 0.58, 1.0);
}

@fragment
fn fs_stroke(in: VertexOutput) -> @location(0) vec4<f32> {
    // Pen strokes (and their live preview) are drawn in flat white.
    return vec4<f32>(1.0, 1.0, 1.0, 1.0);
}
