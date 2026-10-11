struct Camera {
    // logical viewport size, camera center (logical), zoom, pad
    screen: vec2<f32>,
    center: vec2<f32>,
    zoom: f32,
    _pad: f32,
    _pad2: vec2<f32>,
};
@group(0) @binding(0) var<uniform> cam: Camera;

struct VsIn {
    @location(0) pos: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
};
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let p = (in.pos - cam.center) * cam.zoom / (cam.screen * 0.5);
    out.clip = vec4<f32>(p.x, -p.y, 0.0, 1.0);
    out.uv = in.uv;
    out.color = in.color;
    return out;
}

// indexed sprite: R8Uint texture + 256x1 palette
@group(1) @binding(0) var idx_tex: texture_2d<u32>;
@group(1) @binding(1) var pal_tex: texture_2d<f32>;

@fragment
fn fs_indexed(in: VsOut) -> @location(0) vec4<f32> {
    let dim = vec2<f32>(textureDimensions(idx_tex));
    let texel = vec2<i32>(clamp(in.uv * dim, vec2<f32>(0.0), dim - vec2<f32>(1.0)));
    let i = textureLoad(idx_tex, texel, 0).r;
    let c = textureLoad(pal_tex, vec2<i32>(i32(i), 0), 0);
    return c * in.color;
}

// rgba sprite, or solid color via 1x1 white
@group(1) @binding(0) var rgba_tex: texture_2d<f32>;
@group(1) @binding(1) var rgba_samp: sampler;

@fragment
fn fs_rgba(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(rgba_tex, rgba_samp, in.uv) * in.color;
}
