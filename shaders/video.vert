#version 450
//! Fullscreen aspect-fit video quad (6 vertices, no vertex buffer).
//!
//! Ported from the wgpu WGSL vertex stage. Vulkan's clip space has +Y pointing
//! down (opposite wgpu/Metal), so the texture V is derived without the WGSL Y
//! flip: NDC-top (y=-1) maps to texture-top (v=0), keeping the image upright.

layout(push_constant) uniform Params {
    vec2 scale;
    uint hdr;
    uint hdr_output;
} u;

layout(location = 0) out vec2 vUV;

void main() {
    vec2 corners[6] = vec2[](
        vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(1.0, 1.0),
        vec2(-1.0, -1.0), vec2(1.0, 1.0), vec2(-1.0, 1.0)
    );
    vec2 c = corners[gl_VertexIndex];
    gl_Position = vec4(c * u.scale, 0.0, 1.0);
    vUV = vec2(c.x * 0.5 + 0.5, c.y * 0.5 + 0.5);
}
