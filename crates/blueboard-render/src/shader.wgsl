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
    selection_color: vec4<f32>,
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
fn fs_selection(in: VertexOutput) -> @location(0) vec4<f32> {
    // Selection-box outlines and the eraser cursor — never a drawn stroke's color.
    return theme.selection_color;
}

// Separate vertex/fragment pair for per-element colored content (committed strokes/shapes
// and their live preview): each vertex carries its own color instead of reading the fixed
// theme uniform `fs_selection` does — grid lines and UI-only overlays (selection boxes, the
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

// Background: gradient and image fills. Both draw a 6-vertex quad carrying a `uv` in
// addition to `position` — a gradient quad is always the full clip-space viewport (uv
// spans exactly 0..1, `vs_fullscreen`, no camera transform, since a gradient covers the
// screen regardless of pan/zoom); an image quad is either the same full-viewport shape
// (tile mode, uv scaled up so `background_sampler`'s repeat wrap tiles it) or a
// board-space quad sized to the image's native pixels (zoom-with-camera mode,
// `vs_board_tex`, so it pans/zooms like board content). See `BackgroundMode` in lib.rs.
struct TexVertexInput {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
};

struct TexVertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_fullscreen(in: TexVertexInput) -> TexVertexOutput {
    var out: TexVertexOutput;
    out.clip_position = vec4<f32>(in.position, 0.0, 1.0);
    out.uv = in.uv;
    return out;
}

@vertex
fn vs_board_tex(in: TexVertexInput) -> TexVertexOutput {
    var out: TexVertexOutput;
    out.clip_position = camera.view_proj * vec4<f32>(in.position, 0.0, 1.0);
    out.uv = in.uv;
    return out;
}

// Own binding group (distinct from `theme`'s group 1): a gradient's two colors + its
// direction, written fresh each frame `BackgroundMode::Gradient` is active (cheap, a
// handful of floats) rather than kept in sync with a command like the theme uniform is.
// `from`/`to` are WGSL reserved identifiers (even though neither is a keyword today) —
// `from_color`/`to_color` sidestep that rather than relying on them never becoming one.
struct BgGradientUniform {
    from_color: vec4<f32>,
    to_color: vec4<f32>,
    // Unit vector in uv-space the gradient runs along; see `lib.rs`'s angle-to-`dir`
    // conversion for why this is precomputed on the CPU side rather than passing the
    // angle itself and doing trig per-fragment.
    dir: vec2<f32>,
    _pad: vec2<f32>,
};

@group(2) @binding(0)
var<uniform> bg_gradient: BgGradientUniform;

@fragment
fn fs_background_gradient(in: TexVertexOutput) -> @location(0) vec4<f32> {
    let centered = in.uv - vec2<f32>(0.5, 0.5);
    let t = clamp(dot(centered, bg_gradient.dir) + 0.5, 0.0, 1.0);
    return mix(bg_gradient.from_color, bg_gradient.to_color, t);
}

// A separate group (3, not 2 — a module-scope WGSL binding can't be redeclared as a
// different resource kind at the same group/binding even across disjoint entry points)
// since this is a texture+sampler pair, not a uniform buffer like `bg_gradient` above.
@group(3) @binding(0)
var background_texture: texture_2d<f32>;
@group(3) @binding(1)
var background_sampler: sampler;

@fragment
fn fs_background_image(in: TexVertexOutput) -> @location(0) vec4<f32> {
    return textureSample(background_texture, background_sampler, in.uv);
}
