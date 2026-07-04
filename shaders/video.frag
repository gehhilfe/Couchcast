#version 450
//! YUV -> RGB video fragment shader for both the SDR and HDR paths.
//!
//! A direct port of the wgpu WGSL fragment stage. Both formats are semi-planar
//! 4:2:0 (full-res Y + half-res interleaved UV) sampled as normalized floats.
//! The `hdr` push-constant selects BT.709 SDR vs BT.2020+PQ HDR colour math;
//! `hdr_output` selects scRGB HDR passthrough vs tone-map to the SDR display.

layout(set = 0, binding = 0) uniform sampler2D yTex;
layout(set = 0, binding = 1) uniform sampler2D uvTex;

layout(push_constant) uniform Params {
    vec2 scale;
    uint hdr;
    uint hdr_output;
} u;

layout(location = 0) in vec2 vUV;
layout(location = 0) out vec4 outColor;

// SMPTE ST 2084 (PQ) EOTF: nonlinear signal [0,1] -> linear luminance where 1.0
// corresponds to 10000 cd/m^2. Applied per channel.
vec3 pq_eotf(vec3 n) {
    float m1 = 0.1593017578125;
    float m2 = 78.84375;
    float c1 = 0.8359375;
    float c2 = 18.8515625;
    float c3 = 18.6875;
    vec3 np = pow(max(n, vec3(0.0)), vec3(1.0 / m2));
    vec3 num = max(np - c1, vec3(0.0));
    vec3 den = c2 - c3 * np;
    return pow(num / den, vec3(1.0 / m1));
}

void main() {
    float y = texture(yTex, vUV).r;
    vec2 uv = texture(uvTex, vUV).rg;

    if (u.hdr == 0u) {
        // --- SDR path: NV12, BT.709 limited range ---
        float yv = 1.1643 * (y - 0.0627);
        float cb = uv.x - 0.5020;
        float cr = uv.y - 0.5020;
        vec3 rgb = vec3(
            yv + 1.7927 * cr,
            yv - 0.2132 * cb - 0.5329 * cr,
            yv + 2.1124 * cb);
        rgb = clamp(rgb, vec3(0.0), vec3(1.0));
        // Gamma-encoded video -> linear, so the sRGB target encodes it back.
        outColor = vec4(pow(rgb, vec3(2.2)), 1.0);
        return;
    }

    // --- HDR path: P010, BT.2020 limited range, PQ ---
    float yv = 1.1678 * (y - 0.0626);
    float cb = 1.1417 * (uv.x - 0.5005);
    float cr = 1.1417 * (uv.y - 0.5005);
    vec3 pq = vec3(
        yv + 1.4746 * cr,
        yv - 0.16455 * cb - 0.57135 * cr,
        yv + 1.8814 * cb);
    pq = clamp(pq, vec3(0.0), vec3(1.0));
    vec3 lin = pq_eotf(pq);

    if (u.hdr_output != 0u) {
        // --- HDR passthrough to a scRGB (extended-sRGB-linear) swapchain ---
        float scrgb_white = 80.0;
        vec3 nits = lin * 10000.0;
        vec3 scene = nits / scrgb_white;
        float r = dot(vec3( 1.66049, -0.58764, -0.07285), scene);
        float g = dot(vec3(-0.12455,  1.13290, -0.00835), scene);
        float b = dot(vec3(-0.01821, -0.10064,  1.11885), scene);
        outColor = vec4(max(vec3(r, g, b), vec3(0.0)), 1.0);
        return;
    }

    // --- Tone-map to the SDR display ---
    float ref_white = 203.0;
    float peak = 1000.0;
    float exposure = 2.0;
    vec3 x = lin * (10000.0 / ref_white) * exposure;
    float w = (peak / ref_white) * exposure;
    vec3 mapped = x * (1.0 + x / (w * w)) / (1.0 + x);

    float r = dot(vec3( 1.66049, -0.58764, -0.07285), mapped);
    float g = dot(vec3(-0.12455,  1.13290, -0.00835), mapped);
    float b = dot(vec3(-0.01821, -0.10064,  1.11885), mapped);
    vec3 rgb = clamp(vec3(r, g, b), vec3(0.0), vec3(1.0));
    outColor = vec4(rgb, 1.0);
}
