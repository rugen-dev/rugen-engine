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
    // PalFX multiply rgb + colour saturation (1 = untouched, 0 = greyscale)
    @location(3) fx_mul: vec4<f32>,
    // PalFX add rgb + invert flag
    @location(4) fx_add: vec4<f32>,
    // atlas rectangle [x, y, w, h] sampled as the whole texture (w = 0: none)
    @location(5) rect: vec4<u32>,
};
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) fx_mul: vec4<f32>,
    @location(3) fx_add: vec4<f32>,
    @location(4) @interpolate(flat) rect: vec4<u32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let p = (in.pos - cam.center) * cam.zoom / (cam.screen * 0.5);
    out.clip = vec4<f32>(p.x, -p.y, 0.0, 1.0);
    out.uv = in.uv;
    out.color = in.color;
    out.fx_mul = in.fx_mul;
    out.fx_add = in.fx_add;
    out.rect = in.rect;
    return out;
}

// Apply PalFX-style tint, modulate, premultiply alpha.
// color.a scales the source; fx_mul.a is colour saturation (+2 when inverting);
// fx_add.a is the destination scale for Add/Sub blends, or -1 for normal alpha.
fn finish(c_in: vec4<f32>, in: VsOut) -> vec4<f32> {
    // MUGEN / Ikemen order: invert, colour (the mean of r, g, b), add, then
    // multiply: palfx add = 255 whitens even with mul < 256
    var rgb = c_in.rgb;
    let invert = in.fx_mul.a >= 2.0;
    let sat = select(in.fx_mul.a, in.fx_mul.a - 2.0, invert);
    if (invert) {
        rgb = vec3<f32>(1.0) - rgb;
    }
    let grey = (rgb.r + rgb.g + rgb.b) / 3.0;
    rgb = mix(vec3<f32>(grey), rgb, sat);
    rgb = (rgb + in.fx_add.rgb) * in.fx_mul.rgb;
    rgb = clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)) * in.color.rgb;
    let a = c_in.a * in.color.a;
    let out_a = select(c_in.a * (1.0 - in.fx_add.a), a, in.fx_add.a < 0.0);
    return vec4<f32>(rgb * a, out_a);
}

// indexed sprite: R8Uint texture + 256x1 palette
@group(1) @binding(0) var idx_tex: texture_2d<u32>;
@group(1) @binding(1) var pal_tex: texture_2d<f32>;

@fragment
fn fs_indexed(in: VsOut) -> @location(0) vec4<f32> {
    // a glyph of an atlas page is sampled with its own size and origin, so
    // its pixels come out exactly as from a texture of its own
    let whole = in.rect.z == 0u;
    let dim = select(vec2<f32>(f32(in.rect.z), f32(in.rect.w)), vec2<f32>(textureDimensions(idx_tex)), whole);
    let origin = select(vec2<i32>(i32(in.rect.x), i32(in.rect.y)), vec2<i32>(0, 0), whole);
    let texel = origin + vec2<i32>(clamp(in.uv * dim, vec2<f32>(0.0), dim - vec2<f32>(1.0)));
    let i = textureLoad(idx_tex, texel, 0).r;
    let c = textureLoad(pal_tex, vec2<i32>(i32(i), 0), 0);
    return finish(c, in);
}

// rgba sprite, or solid color via 1x1 white
@group(1) @binding(0) var rgba_tex: texture_2d<f32>;
@group(1) @binding(1) var rgba_samp: sampler;

@fragment
fn fs_rgba(in: VsOut) -> @location(0) vec4<f32> {
    return finish(textureSample(rgba_tex, rgba_samp, in.uv), in);
}

// low-spec mode: the frame rendered at a lower resolution, copied to the
// window as it is (no tint, no premultiply; the pipeline does not blend)
@fragment
fn fs_blit(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(rgba_tex, rgba_samp, in.uv);
}
