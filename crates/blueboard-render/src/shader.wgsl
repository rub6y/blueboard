struct CameraUniform {
    view_proj: mat4x4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: CameraUniform;

// vec4s, not vec3s, even though only .xyz is used: WGSL's uniform-buffer layout rules
// pad a bare vec3<f32> to 16 bytes anyway, so using vec4 avoids a layout mismatch with
// the Rust-side struct for free.
struct ThemeUniform {
    grid_color: vec4<f32>,
    stroke_color: vec4<f32>,
};

@group(1) @binding(0)
var<uniform> theme: ThemeUniform;

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
    return theme.grid_color;
}

@fragment
fn fs_stroke(in: VertexOutput) -> @location(0) vec4<f32> {
    // Also used for the live stroke-preview pipeline — both are "pen" colored.
    return theme.stroke_color;
}

// Separate vertex/fragment pair for per-element colored content (committed strokes/shapes
// and their live preview): each vertex carries its own color instead of reading the fixed
// theme uniform `fs_stroke` does — grid lines and UI-only overlays (selection boxes, the
// eraser cursor) stay on the uniform-colored path above, since those aren't board content
// and shouldn't follow whatever color the user's currently drawing with.
struct ColoredVertexInput {
    @location(0) position: vec2<f32>,
    @location(1) color: vec3<f32>,
};

struct ColoredVertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec3<f32>,
};

@vertex
fn vs_colored(in: ColoredVertexInput) -> ColoredVertexOutput {
    var out: ColoredVertexOutput;
    out.clip_position = camera.view_proj * vec4<f32>(in.position, 0.0, 1.0);
    out.color = in.color;
    return out;
}

@fragment
fn fs_colored(in: ColoredVertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color, 1.0);
}
