#pragma once
//! Vulkan setup and the compositing render pass. Ported from `couchcast::render`
//! (which used wgpu). Draws in two passes into one render pass: the live video
//! texture as a fullscreen aspect-fit quad (YUV -> RGB in-shader), then Dear
//! ImGui on top. One pipeline handles the SDR path (8-bit NV12, BT.709) and the
//! HDR path (10-bit P010, BT.2020 + PQ), differing only in plane texture formats
//! and the push-constant `hdr` flag.
//!
//! SDR presents to an sRGB swapchain; HDR presents to an scRGB (Rgba16Float,
//! VK_COLOR_SPACE_EXTENDED_SRGB_LINEAR) swapchain when the surface advertises it.

#include <vulkan/vulkan.h>

#include <cstdint>
#include <memory>
#include <optional>
#include <string>
#include <vector>

struct SDL_Window;

namespace couchcast::media {
class VideoFrame;
enum class PixelFormat;
}  // namespace couchcast::media

namespace couchcast::render {

class Renderer {
   public:
    ~Renderer();
    Renderer(const Renderer&) = delete;
    Renderer& operator=(const Renderer&) = delete;

    /// Create the Vulkan device/surface for `window`. `prefer_hdr` requests an
    /// scRGB swapchain when the surface advertises one. Returns nullptr on failure.
    static std::unique_ptr<Renderer> create(SDL_Window* window, bool prefer_hdr);

    /// Initialize the Dear ImGui SDL3 + Vulkan backends against this renderer.
    void init_imgui();

    const std::string& adapter_info() const { return adapter_info_; }
    bool hdr_available() const { return hdr_format_.has_value(); }
    bool hdr_output() const { return hdr_output_; }

    /// Switch the swapchain between SDR (sRGB) and HDR (scRGB). Returns whether
    /// the surface format actually changed.
    bool set_hdr_output(bool on);

    /// Upload a decoded frame (NV12 or P010) into the video textures, recreating
    /// them if the resolution or pixel format changed.
    void upload_video(const media::VideoFrame& frame);

    /// Begin an ImGui frame (calls the Vulkan + SDL3 backend NewFrame).
    void new_frame();

    /// Draw one frame: video quad (or black) then ImGui (from ImGui::GetDrawData).
    void render();

   private:
    Renderer() = default;

    // --- init steps ---
    bool init(SDL_Window* window, bool prefer_hdr);
    bool create_instance();
    bool create_surface();
    bool pick_physical_device();
    bool create_device();
    void query_surface_formats();
    bool create_swapchain();
    void destroy_swapchain();
    bool create_render_pass();
    void destroy_render_pass();
    bool create_framebuffers();
    void destroy_framebuffers();
    bool create_video_resources();   // layouts, sampler, pool, descriptor, shaders
    void destroy_video_resources();
    bool build_video_pipeline();      // the VkPipeline (rebuilt on format change)
    void destroy_video_pipeline();
    bool create_sync_and_commands();
    bool recreate_swapchain();

    // --- video textures ---
    struct VideoTextures {
        VkImage y_image = VK_NULL_HANDLE;
        VkDeviceMemory y_memory = VK_NULL_HANDLE;
        VkImageView y_view = VK_NULL_HANDLE;
        VkImage uv_image = VK_NULL_HANDLE;
        VkDeviceMemory uv_memory = VK_NULL_HANDLE;
        VkImageView uv_view = VK_NULL_HANDLE;
        uint32_t width = 0;
        uint32_t height = 0;
        int format = 0;  // media::PixelFormat
        bool valid = false;
    };
    void ensure_video_textures(uint32_t w, uint32_t h, media::PixelFormat fmt);
    void destroy_video_textures();
    void update_video_descriptor();

    // --- helpers ---
    uint32_t find_memory_type(uint32_t type_bits, VkMemoryPropertyFlags props) const;
    bool create_image(uint32_t w, uint32_t h, VkFormat format, VkImageUsageFlags usage,
                      VkImage& image, VkDeviceMemory& memory);
    VkImageView create_image_view(VkImage image, VkFormat format);
    bool create_buffer(VkDeviceSize size, VkBufferUsageFlags usage,
                       VkMemoryPropertyFlags props, VkBuffer& buffer,
                       VkDeviceMemory& memory);
    void ensure_staging(VkDeviceSize size);
    VkShaderModule load_shader(const uint32_t* code, size_t size_bytes);

    // --- members ---
    SDL_Window* window_ = nullptr;

    VkInstance instance_ = VK_NULL_HANDLE;
    VkDebugUtilsMessengerEXT debug_messenger_ = VK_NULL_HANDLE;
    VkSurfaceKHR surface_ = VK_NULL_HANDLE;
    VkPhysicalDevice physical_ = VK_NULL_HANDLE;
    uint32_t queue_family_ = 0;
    VkDevice device_ = VK_NULL_HANDLE;
    VkQueue queue_ = VK_NULL_HANDLE;

    VkSwapchainKHR swapchain_ = VK_NULL_HANDLE;
    VkFormat swapchain_format_ = VK_FORMAT_UNDEFINED;
    VkColorSpaceKHR swapchain_color_space_ = VK_COLOR_SPACE_SRGB_NONLINEAR_KHR;
    VkExtent2D extent_{};
    std::vector<VkImage> swap_images_;
    std::vector<VkImageView> swap_views_;
    std::vector<VkFramebuffer> framebuffers_;

    VkRenderPass render_pass_ = VK_NULL_HANDLE;

    VkDescriptorSetLayout video_set_layout_ = VK_NULL_HANDLE;
    VkPipelineLayout video_pipeline_layout_ = VK_NULL_HANDLE;
    VkPipeline video_pipeline_ = VK_NULL_HANDLE;
    VkShaderModule vert_module_ = VK_NULL_HANDLE;
    VkShaderModule frag_module_ = VK_NULL_HANDLE;
    VkSampler video_sampler_ = VK_NULL_HANDLE;
    VkDescriptorPool descriptor_pool_ = VK_NULL_HANDLE;
    VkDescriptorSet video_descriptor_ = VK_NULL_HANDLE;
    VkDescriptorPool imgui_pool_ = VK_NULL_HANDLE;

    VkCommandPool command_pool_ = VK_NULL_HANDLE;
    VkCommandBuffer command_buffer_ = VK_NULL_HANDLE;
    VkCommandBuffer upload_cmd_ = VK_NULL_HANDLE;
    VkSemaphore image_available_ = VK_NULL_HANDLE;
    VkSemaphore render_finished_ = VK_NULL_HANDLE;
    VkFence in_flight_ = VK_NULL_HANDLE;

    VideoTextures video_;
    VkBuffer staging_ = VK_NULL_HANDLE;
    VkDeviceMemory staging_memory_ = VK_NULL_HANDLE;
    VkDeviceSize staging_size_ = 0;
    void* staging_mapped_ = nullptr;
    bool video_hdr_ = false;

    // Surface format resolution.
    VkFormat sdr_format_ = VK_FORMAT_B8G8R8A8_SRGB;
    std::optional<VkFormat> hdr_format_;  // scRGB Rgba16Float, if available
    bool hdr_output_ = false;
    bool colorspace_ext_ = false;

    std::string adapter_info_;
    bool imgui_ready_ = false;
};

}  // namespace couchcast::render
