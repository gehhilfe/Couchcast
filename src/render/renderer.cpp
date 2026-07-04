#include "render/renderer.hpp"

#include <SDL3/SDL.h>
#include <SDL3/SDL_vulkan.h>
#include <backends/imgui_impl_sdl3.h>
#include <backends/imgui_impl_vulkan.h>
#include <imgui.h>

#include <algorithm>
#include <cstdlib>
#include <cstring>

#include "log.hpp"
#include "media/frame.hpp"

// SPIR-V generated from shaders/*.{vert,frag} by glslc (-mfmt=c) at build time.
static const uint32_t kVideoVertSpv[] =
#include "video_vert.inc"
    ;
static const uint32_t kVideoFragSpv[] =
#include "video_frag.inc"
    ;

namespace couchcast::render {

namespace {
constexpr int MAX_FRAMES = 1;  // single frame in flight: simplest, low-latency

struct PushConstants {
    float scale[2];
    uint32_t hdr;
    uint32_t hdr_output;
};

bool vk_ok(VkResult r, const char* what) {
    if (r != VK_SUCCESS) {
        CC_ERROR("vulkan: %s failed (%d)", what, static_cast<int>(r));
        return false;
    }
    return true;
}
}  // namespace

Renderer::~Renderer() {
    if (device_) vkDeviceWaitIdle(device_);

    if (imgui_ready_) {
        ImGui_ImplVulkan_Shutdown();
        ImGui_ImplSDL3_Shutdown();
    }

    destroy_video_textures();
    if (staging_) {
        if (staging_mapped_) vkUnmapMemory(device_, staging_memory_);
        vkDestroyBuffer(device_, staging_, nullptr);
        vkFreeMemory(device_, staging_memory_, nullptr);
    }

    if (image_available_) vkDestroySemaphore(device_, image_available_, nullptr);
    if (render_finished_) vkDestroySemaphore(device_, render_finished_, nullptr);
    if (in_flight_) vkDestroyFence(device_, in_flight_, nullptr);
    if (command_pool_) vkDestroyCommandPool(device_, command_pool_, nullptr);

    destroy_video_pipeline();
    destroy_video_resources();

    destroy_framebuffers();
    destroy_render_pass();
    destroy_swapchain();

    if (imgui_pool_) vkDestroyDescriptorPool(device_, imgui_pool_, nullptr);

    if (device_) vkDestroyDevice(device_, nullptr);
    if (surface_) vkDestroySurfaceKHR(instance_, surface_, nullptr);
    if (debug_messenger_) {
        auto f = reinterpret_cast<PFN_vkDestroyDebugUtilsMessengerEXT>(
            vkGetInstanceProcAddr(instance_, "vkDestroyDebugUtilsMessengerEXT"));
        if (f) f(instance_, debug_messenger_, nullptr);
    }
    if (instance_) vkDestroyInstance(instance_, nullptr);
}

std::unique_ptr<Renderer> Renderer::create(SDL_Window* window, bool prefer_hdr) {
    auto r = std::unique_ptr<Renderer>(new Renderer());
    if (!r->init(window, prefer_hdr)) return nullptr;
    return r;
}

bool Renderer::init(SDL_Window* window, bool prefer_hdr) {
    window_ = window;
    if (!create_instance()) return false;
    if (!create_surface()) return false;
    if (!pick_physical_device()) return false;
    if (!create_device()) return false;
    query_surface_formats();
    hdr_output_ = prefer_hdr && hdr_format_.has_value();
    if (!create_swapchain()) return false;
    if (!create_render_pass()) return false;
    if (!create_framebuffers()) return false;
    if (!create_video_resources()) return false;
    if (!build_video_pipeline()) return false;
    if (!create_sync_and_commands()) return false;
    return true;
}

// --------------------------------------------------------------------------
// Instance / surface / device
// --------------------------------------------------------------------------
bool Renderer::create_instance() {
    uint32_t sdl_ext_count = 0;
    const char* const* sdl_exts = SDL_Vulkan_GetInstanceExtensions(&sdl_ext_count);
    if (!sdl_exts) {
        CC_ERROR("SDL_Vulkan_GetInstanceExtensions: %s", SDL_GetError());
        return false;
    }
    std::vector<const char*> extensions(sdl_exts, sdl_exts + sdl_ext_count);

    // Probe for VK_EXT_swapchain_colorspace (needed to advertise the scRGB HDR
    // color space) and VK_KHR_get_surface_capabilities2.
    uint32_t avail_count = 0;
    vkEnumerateInstanceExtensionProperties(nullptr, &avail_count, nullptr);
    std::vector<VkExtensionProperties> avail(avail_count);
    vkEnumerateInstanceExtensionProperties(nullptr, &avail_count, avail.data());
    auto has = [&](const char* name) {
        for (const auto& e : avail)
            if (std::strcmp(e.extensionName, name) == 0) return true;
        return false;
    };
    if (has(VK_EXT_SWAPCHAIN_COLOR_SPACE_EXTENSION_NAME)) {
        extensions.push_back(VK_EXT_SWAPCHAIN_COLOR_SPACE_EXTENSION_NAME);
        colorspace_ext_ = true;
    }

    std::vector<const char*> layers;
    bool want_validation = std::getenv("COUCHCAST_VK_VALIDATION") != nullptr;
    if (want_validation) {
        layers.push_back("VK_LAYER_KHRONOS_validation");
        if (has(VK_EXT_DEBUG_UTILS_EXTENSION_NAME))
            extensions.push_back(VK_EXT_DEBUG_UTILS_EXTENSION_NAME);
    }

    VkApplicationInfo app{VK_STRUCTURE_TYPE_APPLICATION_INFO};
    app.pApplicationName = "Couchcast";
    app.apiVersion = VK_API_VERSION_1_2;

    VkInstanceCreateInfo ci{VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO};
    ci.pApplicationInfo = &app;
    ci.enabledExtensionCount = static_cast<uint32_t>(extensions.size());
    ci.ppEnabledExtensionNames = extensions.data();
    ci.enabledLayerCount = static_cast<uint32_t>(layers.size());
    ci.ppEnabledLayerNames = layers.data();

    if (!vk_ok(vkCreateInstance(&ci, nullptr, &instance_), "vkCreateInstance"))
        return false;
    return true;
}

bool Renderer::create_surface() {
    if (!SDL_Vulkan_CreateSurface(window_, instance_, nullptr, &surface_)) {
        CC_ERROR("SDL_Vulkan_CreateSurface: %s", SDL_GetError());
        return false;
    }
    return true;
}

bool Renderer::pick_physical_device() {
    uint32_t count = 0;
    vkEnumeratePhysicalDevices(instance_, &count, nullptr);
    if (count == 0) {
        CC_ERROR("no Vulkan physical devices");
        return false;
    }
    std::vector<VkPhysicalDevice> devices(count);
    vkEnumeratePhysicalDevices(instance_, &count, devices.data());

    VkPhysicalDevice best = VK_NULL_HANDLE;
    uint32_t best_family = 0;
    int best_score = -1;
    for (VkPhysicalDevice dev : devices) {
        uint32_t qcount = 0;
        vkGetPhysicalDeviceQueueFamilyProperties(dev, &qcount, nullptr);
        std::vector<VkQueueFamilyProperties> families(qcount);
        vkGetPhysicalDeviceQueueFamilyProperties(dev, &qcount, families.data());
        for (uint32_t i = 0; i < qcount; ++i) {
            VkBool32 present = VK_FALSE;
            vkGetPhysicalDeviceSurfaceSupportKHR(dev, i, surface_, &present);
            if ((families[i].queueFlags & VK_QUEUE_GRAPHICS_BIT) && present) {
                VkPhysicalDeviceProperties props;
                vkGetPhysicalDeviceProperties(dev, &props);
                int score =
                    props.deviceType == VK_PHYSICAL_DEVICE_TYPE_DISCRETE_GPU ? 2 : 1;
                if (score > best_score) {
                    best_score = score;
                    best = dev;
                    best_family = i;
                }
                break;
            }
        }
    }
    if (!best) {
        CC_ERROR("no suitable graphics+present queue family");
        return false;
    }
    physical_ = best;
    queue_family_ = best_family;

    VkPhysicalDeviceProperties props;
    vkGetPhysicalDeviceProperties(physical_, &props);
    const char* type = props.deviceType == VK_PHYSICAL_DEVICE_TYPE_DISCRETE_GPU
                           ? "Discrete"
                           : (props.deviceType == VK_PHYSICAL_DEVICE_TYPE_INTEGRATED_GPU
                                  ? "Integrated"
                                  : "Other");
    adapter_info_ = std::string(props.deviceName) + " (Vulkan, " + type + ")";
    CC_INFO("selected GPU adapter: %s", adapter_info_.c_str());
    return true;
}

bool Renderer::create_device() {
    float priority = 1.0f;
    VkDeviceQueueCreateInfo qci{VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO};
    qci.queueFamilyIndex = queue_family_;
    qci.queueCount = 1;
    qci.pQueuePriorities = &priority;

    const char* device_exts[] = {VK_KHR_SWAPCHAIN_EXTENSION_NAME};

    VkDeviceCreateInfo ci{VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO};
    ci.queueCreateInfoCount = 1;
    ci.pQueueCreateInfos = &qci;
    ci.enabledExtensionCount = 1;
    ci.ppEnabledExtensionNames = device_exts;

    if (!vk_ok(vkCreateDevice(physical_, &ci, nullptr, &device_), "vkCreateDevice"))
        return false;
    vkGetDeviceQueue(device_, queue_family_, 0, &queue_);

    // R16/Rg16 UNORM back the 10-bit P010 planes; warn (don't fail) if missing.
    VkFormatProperties fp;
    vkGetPhysicalDeviceFormatProperties(physical_, VK_FORMAT_R16_UNORM, &fp);
    if (!(fp.optimalTilingFeatures & VK_FORMAT_FEATURE_SAMPLED_IMAGE_BIT)) {
        CC_WARN("adapter lacks R16_UNORM sampling; P010/HDR capture will not render");
    }
    return true;
}

void Renderer::query_surface_formats() {
    uint32_t count = 0;
    vkGetPhysicalDeviceSurfaceFormatsKHR(physical_, surface_, &count, nullptr);
    std::vector<VkSurfaceFormatKHR> formats(count);
    vkGetPhysicalDeviceSurfaceFormatsKHR(physical_, surface_, &count, formats.data());

    // SDR: first sRGB format (prefer B8G8R8A8_SRGB).
    sdr_format_ = VK_FORMAT_B8G8R8A8_SRGB;
    bool found_sdr = false;
    for (const auto& f : formats) {
        if (f.colorSpace == VK_COLOR_SPACE_SRGB_NONLINEAR_KHR &&
            (f.format == VK_FORMAT_B8G8R8A8_SRGB || f.format == VK_FORMAT_R8G8B8A8_SRGB)) {
            sdr_format_ = f.format;
            found_sdr = true;
            break;
        }
    }
    if (!found_sdr && count > 0) sdr_format_ = formats[0].format;

    // HDR: scRGB Rgba16Float + EXTENDED_SRGB_LINEAR color space.
    for (const auto& f : formats) {
        if (f.format == VK_FORMAT_R16G16B16A16_SFLOAT &&
            f.colorSpace == VK_COLOR_SPACE_EXTENDED_SRGB_LINEAR_EXT) {
            hdr_format_ = f.format;
            break;
        }
    }
    if (hdr_format_)
        CC_INFO("HDR (scRGB) surface format available");
    else
        CC_INFO("no HDR surface format advertised; SDR output only");
}

// --------------------------------------------------------------------------
// Swapchain / render pass / framebuffers
// --------------------------------------------------------------------------
bool Renderer::create_swapchain() {
    VkSurfaceCapabilitiesKHR caps{};
    if (!vk_ok(vkGetPhysicalDeviceSurfaceCapabilitiesKHR(physical_, surface_, &caps),
               "vkGetPhysicalDeviceSurfaceCapabilitiesKHR"))
        return false;

    int pw = 0, ph = 0;
    SDL_GetWindowSizeInPixels(window_, &pw, &ph);

    // Prefer the surface's fixed extent; otherwise (0xFFFFFFFF "you decide", or a
    // compositor that reports 0x0) fall back to the window's pixel size.
    if (caps.currentExtent.width != 0xFFFFFFFF && caps.currentExtent.width != 0 &&
        caps.currentExtent.height != 0) {
        extent_ = caps.currentExtent;
    } else {
        extent_.width = pw > 0 ? static_cast<uint32_t>(pw) : 1280;
        extent_.height = ph > 0 ? static_cast<uint32_t>(ph) : 720;
    }
    // Clamp only against non-degenerate bounds (some headless compositors report
    // a 0x0 range even though they accept a real swapchain size).
    if (caps.minImageExtent.width > 0)
        extent_.width = std::max(extent_.width, caps.minImageExtent.width);
    if (caps.minImageExtent.height > 0)
        extent_.height = std::max(extent_.height, caps.minImageExtent.height);
    if (caps.maxImageExtent.width > 0)
        extent_.width = std::min(extent_.width, caps.maxImageExtent.width);
    if (caps.maxImageExtent.height > 0)
        extent_.height = std::min(extent_.height, caps.maxImageExtent.height);
    if (extent_.width == 0 || extent_.height == 0) {
        // Minimized; defer creation.
        return false;
    }

    // preTransform must be a single supported bit.
    VkSurfaceTransformFlagBitsKHR pre_transform =
        (caps.supportedTransforms & VK_SURFACE_TRANSFORM_IDENTITY_BIT_KHR)
            ? VK_SURFACE_TRANSFORM_IDENTITY_BIT_KHR
            : caps.currentTransform;

    // compositeAlpha must be a single supported bit.
    VkCompositeAlphaFlagBitsKHR composite_alpha = VK_COMPOSITE_ALPHA_OPAQUE_BIT_KHR;
    if (!(caps.supportedCompositeAlpha & composite_alpha)) {
        for (VkCompositeAlphaFlagBitsKHR bit :
             {VK_COMPOSITE_ALPHA_INHERIT_BIT_KHR, VK_COMPOSITE_ALPHA_PRE_MULTIPLIED_BIT_KHR,
              VK_COMPOSITE_ALPHA_POST_MULTIPLIED_BIT_KHR}) {
            if (caps.supportedCompositeAlpha & bit) {
                composite_alpha = bit;
                break;
            }
        }
    }

    if (hdr_output_ && hdr_format_) {
        swapchain_format_ = *hdr_format_;
        swapchain_color_space_ = VK_COLOR_SPACE_EXTENDED_SRGB_LINEAR_EXT;
    } else {
        swapchain_format_ = sdr_format_;
        swapchain_color_space_ = VK_COLOR_SPACE_SRGB_NONLINEAR_KHR;
    }

    uint32_t image_count = caps.minImageCount + 1;
    if (caps.maxImageCount > 0 && image_count > caps.maxImageCount)
        image_count = caps.maxImageCount;

    VkSwapchainCreateInfoKHR ci{VK_STRUCTURE_TYPE_SWAPCHAIN_CREATE_INFO_KHR};
    ci.surface = surface_;
    ci.minImageCount = image_count;
    ci.imageFormat = swapchain_format_;
    ci.imageColorSpace = swapchain_color_space_;
    ci.imageExtent = extent_;
    ci.imageArrayLayers = 1;
    ci.imageUsage = VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT;
    ci.imageSharingMode = VK_SHARING_MODE_EXCLUSIVE;
    ci.preTransform = pre_transform;
    ci.compositeAlpha = composite_alpha;
    ci.presentMode = VK_PRESENT_MODE_FIFO_KHR;  // vsync
    ci.clipped = VK_TRUE;

    if (!vk_ok(vkCreateSwapchainKHR(device_, &ci, nullptr, &swapchain_),
               "vkCreateSwapchainKHR"))
        return false;

    uint32_t n = 0;
    vkGetSwapchainImagesKHR(device_, swapchain_, &n, nullptr);
    swap_images_.resize(n);
    vkGetSwapchainImagesKHR(device_, swapchain_, &n, swap_images_.data());

    swap_views_.resize(n);
    for (uint32_t i = 0; i < n; ++i) {
        swap_views_[i] = create_image_view(swap_images_[i], swapchain_format_);
    }
    return true;
}

void Renderer::destroy_swapchain() {
    for (auto v : swap_views_) vkDestroyImageView(device_, v, nullptr);
    swap_views_.clear();
    swap_images_.clear();
    if (swapchain_) {
        vkDestroySwapchainKHR(device_, swapchain_, nullptr);
        swapchain_ = VK_NULL_HANDLE;
    }
}

bool Renderer::create_render_pass() {
    VkAttachmentDescription color{};
    color.format = swapchain_format_;
    color.samples = VK_SAMPLE_COUNT_1_BIT;
    color.loadOp = VK_ATTACHMENT_LOAD_OP_CLEAR;
    color.storeOp = VK_ATTACHMENT_STORE_OP_STORE;
    color.stencilLoadOp = VK_ATTACHMENT_LOAD_OP_DONT_CARE;
    color.stencilStoreOp = VK_ATTACHMENT_STORE_OP_DONT_CARE;
    color.initialLayout = VK_IMAGE_LAYOUT_UNDEFINED;
    color.finalLayout = VK_IMAGE_LAYOUT_PRESENT_SRC_KHR;

    VkAttachmentReference ref{0, VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL};
    VkSubpassDescription subpass{};
    subpass.pipelineBindPoint = VK_PIPELINE_BIND_POINT_GRAPHICS;
    subpass.colorAttachmentCount = 1;
    subpass.pColorAttachments = &ref;

    VkSubpassDependency dep{};
    dep.srcSubpass = VK_SUBPASS_EXTERNAL;
    dep.dstSubpass = 0;
    dep.srcStageMask = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;
    dep.dstStageMask = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;
    dep.srcAccessMask = 0;
    dep.dstAccessMask = VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT;

    VkRenderPassCreateInfo ci{VK_STRUCTURE_TYPE_RENDER_PASS_CREATE_INFO};
    ci.attachmentCount = 1;
    ci.pAttachments = &color;
    ci.subpassCount = 1;
    ci.pSubpasses = &subpass;
    ci.dependencyCount = 1;
    ci.pDependencies = &dep;

    return vk_ok(vkCreateRenderPass(device_, &ci, nullptr, &render_pass_),
                 "vkCreateRenderPass");
}

void Renderer::destroy_render_pass() {
    if (render_pass_) {
        vkDestroyRenderPass(device_, render_pass_, nullptr);
        render_pass_ = VK_NULL_HANDLE;
    }
}

bool Renderer::create_framebuffers() {
    framebuffers_.resize(swap_views_.size());
    for (size_t i = 0; i < swap_views_.size(); ++i) {
        VkFramebufferCreateInfo ci{VK_STRUCTURE_TYPE_FRAMEBUFFER_CREATE_INFO};
        ci.renderPass = render_pass_;
        ci.attachmentCount = 1;
        ci.pAttachments = &swap_views_[i];
        ci.width = extent_.width;
        ci.height = extent_.height;
        ci.layers = 1;
        if (!vk_ok(vkCreateFramebuffer(device_, &ci, nullptr, &framebuffers_[i]),
                   "vkCreateFramebuffer"))
            return false;
    }
    return true;
}

void Renderer::destroy_framebuffers() {
    for (auto f : framebuffers_) vkDestroyFramebuffer(device_, f, nullptr);
    framebuffers_.clear();
}

bool Renderer::recreate_swapchain() {
    vkDeviceWaitIdle(device_);
    destroy_framebuffers();
    destroy_swapchain();
    if (!create_swapchain()) return false;
    if (!create_framebuffers()) return false;
    return true;
}

// --------------------------------------------------------------------------
// Video pipeline resources
// --------------------------------------------------------------------------
VkShaderModule Renderer::load_shader(const uint32_t* code, size_t size_bytes) {
    VkShaderModuleCreateInfo ci{VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO};
    ci.codeSize = size_bytes;
    ci.pCode = code;
    VkShaderModule m = VK_NULL_HANDLE;
    vkCreateShaderModule(device_, &ci, nullptr, &m);
    return m;
}

bool Renderer::create_video_resources() {
    // Descriptor set layout: two combined image samplers (Y, UV).
    VkDescriptorSetLayoutBinding bindings[2]{};
    for (int i = 0; i < 2; ++i) {
        bindings[i].binding = i;
        bindings[i].descriptorType = VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER;
        bindings[i].descriptorCount = 1;
        bindings[i].stageFlags = VK_SHADER_STAGE_FRAGMENT_BIT;
    }
    VkDescriptorSetLayoutCreateInfo slci{
        VK_STRUCTURE_TYPE_DESCRIPTOR_SET_LAYOUT_CREATE_INFO};
    slci.bindingCount = 2;
    slci.pBindings = bindings;
    if (!vk_ok(vkCreateDescriptorSetLayout(device_, &slci, nullptr, &video_set_layout_),
               "descriptor set layout"))
        return false;

    VkPushConstantRange pcr{};
    pcr.stageFlags = VK_SHADER_STAGE_VERTEX_BIT | VK_SHADER_STAGE_FRAGMENT_BIT;
    pcr.offset = 0;
    pcr.size = sizeof(PushConstants);

    VkPipelineLayoutCreateInfo plci{VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO};
    plci.setLayoutCount = 1;
    plci.pSetLayouts = &video_set_layout_;
    plci.pushConstantRangeCount = 1;
    plci.pPushConstantRanges = &pcr;
    if (!vk_ok(vkCreatePipelineLayout(device_, &plci, nullptr, &video_pipeline_layout_),
               "pipeline layout"))
        return false;

    VkSamplerCreateInfo sci{VK_STRUCTURE_TYPE_SAMPLER_CREATE_INFO};
    sci.magFilter = VK_FILTER_LINEAR;
    sci.minFilter = VK_FILTER_LINEAR;
    sci.addressModeU = VK_SAMPLER_ADDRESS_MODE_CLAMP_TO_EDGE;
    sci.addressModeV = VK_SAMPLER_ADDRESS_MODE_CLAMP_TO_EDGE;
    sci.addressModeW = VK_SAMPLER_ADDRESS_MODE_CLAMP_TO_EDGE;
    if (!vk_ok(vkCreateSampler(device_, &sci, nullptr, &video_sampler_), "sampler"))
        return false;

    VkDescriptorPoolSize pool_size{VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER, 2};
    VkDescriptorPoolCreateInfo dpci{VK_STRUCTURE_TYPE_DESCRIPTOR_POOL_CREATE_INFO};
    dpci.maxSets = 1;
    dpci.poolSizeCount = 1;
    dpci.pPoolSizes = &pool_size;
    if (!vk_ok(vkCreateDescriptorPool(device_, &dpci, nullptr, &descriptor_pool_),
               "descriptor pool"))
        return false;

    VkDescriptorSetAllocateInfo dsai{VK_STRUCTURE_TYPE_DESCRIPTOR_SET_ALLOCATE_INFO};
    dsai.descriptorPool = descriptor_pool_;
    dsai.descriptorSetCount = 1;
    dsai.pSetLayouts = &video_set_layout_;
    if (!vk_ok(vkAllocateDescriptorSets(device_, &dsai, &video_descriptor_),
               "allocate descriptor set"))
        return false;

    vert_module_ = load_shader(kVideoVertSpv, sizeof(kVideoVertSpv));
    frag_module_ = load_shader(kVideoFragSpv, sizeof(kVideoFragSpv));
    if (!vert_module_ || !frag_module_) {
        CC_ERROR("failed to load video shaders");
        return false;
    }
    return true;
}

void Renderer::destroy_video_resources() {
    if (vert_module_) vkDestroyShaderModule(device_, vert_module_, nullptr);
    if (frag_module_) vkDestroyShaderModule(device_, frag_module_, nullptr);
    if (video_sampler_) vkDestroySampler(device_, video_sampler_, nullptr);
    if (descriptor_pool_) vkDestroyDescriptorPool(device_, descriptor_pool_, nullptr);
    if (video_pipeline_layout_)
        vkDestroyPipelineLayout(device_, video_pipeline_layout_, nullptr);
    if (video_set_layout_)
        vkDestroyDescriptorSetLayout(device_, video_set_layout_, nullptr);
    vert_module_ = frag_module_ = VK_NULL_HANDLE;
    video_sampler_ = VK_NULL_HANDLE;
    descriptor_pool_ = VK_NULL_HANDLE;
    video_pipeline_layout_ = VK_NULL_HANDLE;
    video_set_layout_ = VK_NULL_HANDLE;
}

bool Renderer::build_video_pipeline() {
    VkPipelineShaderStageCreateInfo stages[2]{};
    stages[0].sType = VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO;
    stages[0].stage = VK_SHADER_STAGE_VERTEX_BIT;
    stages[0].module = vert_module_;
    stages[0].pName = "main";
    stages[1].sType = VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO;
    stages[1].stage = VK_SHADER_STAGE_FRAGMENT_BIT;
    stages[1].module = frag_module_;
    stages[1].pName = "main";

    VkPipelineVertexInputStateCreateInfo vi{
        VK_STRUCTURE_TYPE_PIPELINE_VERTEX_INPUT_STATE_CREATE_INFO};
    VkPipelineInputAssemblyStateCreateInfo ia{
        VK_STRUCTURE_TYPE_PIPELINE_INPUT_ASSEMBLY_STATE_CREATE_INFO};
    ia.topology = VK_PRIMITIVE_TOPOLOGY_TRIANGLE_LIST;

    VkPipelineViewportStateCreateInfo vp{
        VK_STRUCTURE_TYPE_PIPELINE_VIEWPORT_STATE_CREATE_INFO};
    vp.viewportCount = 1;
    vp.scissorCount = 1;

    VkPipelineRasterizationStateCreateInfo rs{
        VK_STRUCTURE_TYPE_PIPELINE_RASTERIZATION_STATE_CREATE_INFO};
    rs.polygonMode = VK_POLYGON_MODE_FILL;
    rs.cullMode = VK_CULL_MODE_NONE;
    rs.frontFace = VK_FRONT_FACE_COUNTER_CLOCKWISE;
    rs.lineWidth = 1.0f;

    VkPipelineMultisampleStateCreateInfo ms{
        VK_STRUCTURE_TYPE_PIPELINE_MULTISAMPLE_STATE_CREATE_INFO};
    ms.rasterizationSamples = VK_SAMPLE_COUNT_1_BIT;

    VkPipelineColorBlendAttachmentState cba{};
    cba.colorWriteMask = VK_COLOR_COMPONENT_R_BIT | VK_COLOR_COMPONENT_G_BIT |
                         VK_COLOR_COMPONENT_B_BIT | VK_COLOR_COMPONENT_A_BIT;
    cba.blendEnable = VK_FALSE;
    VkPipelineColorBlendStateCreateInfo cb{
        VK_STRUCTURE_TYPE_PIPELINE_COLOR_BLEND_STATE_CREATE_INFO};
    cb.attachmentCount = 1;
    cb.pAttachments = &cba;

    VkDynamicState dyn_states[] = {VK_DYNAMIC_STATE_VIEWPORT, VK_DYNAMIC_STATE_SCISSOR};
    VkPipelineDynamicStateCreateInfo dyn{
        VK_STRUCTURE_TYPE_PIPELINE_DYNAMIC_STATE_CREATE_INFO};
    dyn.dynamicStateCount = 2;
    dyn.pDynamicStates = dyn_states;

    VkGraphicsPipelineCreateInfo pci{VK_STRUCTURE_TYPE_GRAPHICS_PIPELINE_CREATE_INFO};
    pci.stageCount = 2;
    pci.pStages = stages;
    pci.pVertexInputState = &vi;
    pci.pInputAssemblyState = &ia;
    pci.pViewportState = &vp;
    pci.pRasterizationState = &rs;
    pci.pMultisampleState = &ms;
    pci.pColorBlendState = &cb;
    pci.pDynamicState = &dyn;
    pci.layout = video_pipeline_layout_;
    pci.renderPass = render_pass_;
    pci.subpass = 0;

    return vk_ok(vkCreateGraphicsPipelines(device_, VK_NULL_HANDLE, 1, &pci, nullptr,
                                           &video_pipeline_),
                 "graphics pipeline");
}

void Renderer::destroy_video_pipeline() {
    if (video_pipeline_) {
        vkDestroyPipeline(device_, video_pipeline_, nullptr);
        video_pipeline_ = VK_NULL_HANDLE;
    }
}

// --------------------------------------------------------------------------
// Commands / sync
// --------------------------------------------------------------------------
bool Renderer::create_sync_and_commands() {
    VkCommandPoolCreateInfo pci{VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO};
    pci.flags = VK_COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER_BIT;
    pci.queueFamilyIndex = queue_family_;
    if (!vk_ok(vkCreateCommandPool(device_, &pci, nullptr, &command_pool_),
               "command pool"))
        return false;

    VkCommandBufferAllocateInfo ai{VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO};
    ai.commandPool = command_pool_;
    ai.level = VK_COMMAND_BUFFER_LEVEL_PRIMARY;
    ai.commandBufferCount = 1;
    if (!vk_ok(vkAllocateCommandBuffers(device_, &ai, &command_buffer_), "cmd buffer"))
        return false;
    if (!vk_ok(vkAllocateCommandBuffers(device_, &ai, &upload_cmd_), "upload cmd"))
        return false;

    VkSemaphoreCreateInfo sci{VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO};
    VkFenceCreateInfo fci{VK_STRUCTURE_TYPE_FENCE_CREATE_INFO};
    fci.flags = VK_FENCE_CREATE_SIGNALED_BIT;
    vkCreateSemaphore(device_, &sci, nullptr, &image_available_);
    vkCreateSemaphore(device_, &sci, nullptr, &render_finished_);
    vkCreateFence(device_, &fci, nullptr, &in_flight_);
    (void)MAX_FRAMES;
    return true;
}

// --------------------------------------------------------------------------
// Memory / image helpers
// --------------------------------------------------------------------------
uint32_t Renderer::find_memory_type(uint32_t type_bits,
                                    VkMemoryPropertyFlags props) const {
    VkPhysicalDeviceMemoryProperties mp;
    vkGetPhysicalDeviceMemoryProperties(physical_, &mp);
    for (uint32_t i = 0; i < mp.memoryTypeCount; ++i) {
        if ((type_bits & (1u << i)) &&
            (mp.memoryTypes[i].propertyFlags & props) == props)
            return i;
    }
    return 0;
}

bool Renderer::create_image(uint32_t w, uint32_t h, VkFormat format,
                            VkImageUsageFlags usage, VkImage& image,
                            VkDeviceMemory& memory) {
    VkImageCreateInfo ci{VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO};
    ci.imageType = VK_IMAGE_TYPE_2D;
    ci.format = format;
    ci.extent = {w < 1 ? 1 : w, h < 1 ? 1 : h, 1};
    ci.mipLevels = 1;
    ci.arrayLayers = 1;
    ci.samples = VK_SAMPLE_COUNT_1_BIT;
    ci.tiling = VK_IMAGE_TILING_OPTIMAL;
    ci.usage = usage;
    ci.sharingMode = VK_SHARING_MODE_EXCLUSIVE;
    ci.initialLayout = VK_IMAGE_LAYOUT_UNDEFINED;
    if (!vk_ok(vkCreateImage(device_, &ci, nullptr, &image), "create image"))
        return false;

    VkMemoryRequirements req;
    vkGetImageMemoryRequirements(device_, image, &req);
    VkMemoryAllocateInfo ai{VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO};
    ai.allocationSize = req.size;
    ai.memoryTypeIndex =
        find_memory_type(req.memoryTypeBits, VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT);
    if (!vk_ok(vkAllocateMemory(device_, &ai, nullptr, &memory), "image memory"))
        return false;
    vkBindImageMemory(device_, image, memory, 0);
    return true;
}

VkImageView Renderer::create_image_view(VkImage image, VkFormat format) {
    VkImageViewCreateInfo ci{VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO};
    ci.image = image;
    ci.viewType = VK_IMAGE_VIEW_TYPE_2D;
    ci.format = format;
    ci.subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1};
    VkImageView view = VK_NULL_HANDLE;
    vkCreateImageView(device_, &ci, nullptr, &view);
    return view;
}

bool Renderer::create_buffer(VkDeviceSize size, VkBufferUsageFlags usage,
                             VkMemoryPropertyFlags props, VkBuffer& buffer,
                             VkDeviceMemory& memory) {
    VkBufferCreateInfo ci{VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO};
    ci.size = size;
    ci.usage = usage;
    ci.sharingMode = VK_SHARING_MODE_EXCLUSIVE;
    if (!vk_ok(vkCreateBuffer(device_, &ci, nullptr, &buffer), "create buffer"))
        return false;
    VkMemoryRequirements req;
    vkGetBufferMemoryRequirements(device_, buffer, &req);
    VkMemoryAllocateInfo ai{VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO};
    ai.allocationSize = req.size;
    ai.memoryTypeIndex = find_memory_type(req.memoryTypeBits, props);
    if (!vk_ok(vkAllocateMemory(device_, &ai, nullptr, &memory), "buffer memory"))
        return false;
    vkBindBufferMemory(device_, buffer, memory, 0);
    return true;
}

void Renderer::ensure_staging(VkDeviceSize size) {
    if (staging_ && staging_size_ >= size) return;
    if (staging_) {
        vkDeviceWaitIdle(device_);
        if (staging_mapped_) vkUnmapMemory(device_, staging_memory_);
        vkDestroyBuffer(device_, staging_, nullptr);
        vkFreeMemory(device_, staging_memory_, nullptr);
        staging_ = VK_NULL_HANDLE;
        staging_mapped_ = nullptr;
    }
    create_buffer(size, VK_BUFFER_USAGE_TRANSFER_SRC_BIT,
                  VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT |
                      VK_MEMORY_PROPERTY_HOST_COHERENT_BIT,
                  staging_, staging_memory_);
    staging_size_ = size;
    vkMapMemory(device_, staging_memory_, 0, size, 0, &staging_mapped_);
}

// --------------------------------------------------------------------------
// Video textures
// --------------------------------------------------------------------------
void Renderer::destroy_video_textures() {
    if (!video_.valid) return;
    if (video_.y_view) vkDestroyImageView(device_, video_.y_view, nullptr);
    if (video_.uv_view) vkDestroyImageView(device_, video_.uv_view, nullptr);
    if (video_.y_image) vkDestroyImage(device_, video_.y_image, nullptr);
    if (video_.uv_image) vkDestroyImage(device_, video_.uv_image, nullptr);
    if (video_.y_memory) vkFreeMemory(device_, video_.y_memory, nullptr);
    if (video_.uv_memory) vkFreeMemory(device_, video_.uv_memory, nullptr);
    video_ = VideoTextures{};
}

void Renderer::ensure_video_textures(uint32_t w, uint32_t h, media::PixelFormat fmt) {
    if (video_.valid && video_.width == w && video_.height == h &&
        video_.format == static_cast<int>(fmt))
        return;

    vkDeviceWaitIdle(device_);
    destroy_video_textures();

    VkFormat y_format, uv_format;
    if (fmt == media::PixelFormat::Nv12) {
        y_format = VK_FORMAT_R8_UNORM;
        uv_format = VK_FORMAT_R8G8_UNORM;
    } else {
        y_format = VK_FORMAT_R16_UNORM;
        uv_format = VK_FORMAT_R16G16_UNORM;
    }

    VkImageUsageFlags usage =
        VK_IMAGE_USAGE_SAMPLED_BIT | VK_IMAGE_USAGE_TRANSFER_DST_BIT;
    create_image(w, h, y_format, usage, video_.y_image, video_.y_memory);
    create_image(w / 2, h / 2, uv_format, usage, video_.uv_image, video_.uv_memory);
    video_.y_view = create_image_view(video_.y_image, y_format);
    video_.uv_view = create_image_view(video_.uv_image, uv_format);
    video_.width = w;
    video_.height = h;
    video_.format = static_cast<int>(fmt);
    video_.valid = true;
    update_video_descriptor();
}

void Renderer::update_video_descriptor() {
    VkDescriptorImageInfo y_info{video_sampler_, video_.y_view,
                                 VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL};
    VkDescriptorImageInfo uv_info{video_sampler_, video_.uv_view,
                                  VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL};
    VkWriteDescriptorSet writes[2]{};
    for (int i = 0; i < 2; ++i) {
        writes[i].sType = VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET;
        writes[i].dstSet = video_descriptor_;
        writes[i].dstBinding = i;
        writes[i].descriptorCount = 1;
        writes[i].descriptorType = VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER;
    }
    writes[0].pImageInfo = &y_info;
    writes[1].pImageInfo = &uv_info;
    vkUpdateDescriptorSets(device_, 2, writes, 0, nullptr);
}

void Renderer::upload_video(const media::VideoFrame& frame) {
    uint32_t w = frame.width();
    uint32_t h = frame.height();
    if (w == 0 || h == 0) return;

    media::PixelFormat fmt = frame.format();
    ensure_video_textures(w, h, fmt);
    video_hdr_ = frame.is_hdr();

    auto y = frame.plane(0);
    auto uv = frame.plane(1);
    if (!y) return;

    // bytes per texel for the two plane formats.
    uint32_t y_texel = (fmt == media::PixelFormat::Nv12) ? 1 : 2;
    uint32_t uv_texel = (fmt == media::PixelFormat::Nv12) ? 2 : 4;

    VkDeviceSize y_size = y->size;
    VkDeviceSize uv_offset = (y_size + 3) & ~VkDeviceSize(3);  // align to 4
    VkDeviceSize uv_size = uv ? uv->size : 0;
    ensure_staging(uv_offset + uv_size);

    std::memcpy(staging_mapped_, y->data, y_size);
    if (uv)
        std::memcpy(static_cast<uint8_t*>(staging_mapped_) + uv_offset, uv->data,
                    uv_size);

    // Record the copy: transition to TRANSFER_DST (from UNDEFINED, contents
    // discarded since we overwrite fully), copy, transition to SHADER_READ_ONLY.
    vkResetCommandBuffer(upload_cmd_, 0);
    VkCommandBufferBeginInfo bi{VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO};
    bi.flags = VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT;
    vkBeginCommandBuffer(upload_cmd_, &bi);

    auto barrier = [&](VkImage img, VkImageLayout oldL, VkImageLayout newL,
                       VkAccessFlags src, VkAccessFlags dst, VkPipelineStageFlags srcS,
                       VkPipelineStageFlags dstS) {
        VkImageMemoryBarrier b{VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER};
        b.oldLayout = oldL;
        b.newLayout = newL;
        b.srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
        b.dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
        b.image = img;
        b.subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1};
        b.srcAccessMask = src;
        b.dstAccessMask = dst;
        vkCmdPipelineBarrier(upload_cmd_, srcS, dstS, 0, 0, nullptr, 0, nullptr, 1, &b);
    };

    auto copy = [&](VkImage img, VkDeviceSize offset, uint32_t stride, uint32_t texel,
                    uint32_t cw, uint32_t ch) {
        VkBufferImageCopy region{};
        region.bufferOffset = offset;
        region.bufferRowLength = stride / texel;  // in texels
        region.bufferImageHeight = 0;
        region.imageSubresource = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 0, 1};
        region.imageExtent = {cw, ch, 1};
        vkCmdCopyBufferToImage(upload_cmd_, staging_, img,
                               VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, 1, &region);
    };

    barrier(video_.y_image, VK_IMAGE_LAYOUT_UNDEFINED,
            VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, 0, VK_ACCESS_TRANSFER_WRITE_BIT,
            VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT, VK_PIPELINE_STAGE_TRANSFER_BIT);
    barrier(video_.uv_image, VK_IMAGE_LAYOUT_UNDEFINED,
            VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, 0, VK_ACCESS_TRANSFER_WRITE_BIT,
            VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT, VK_PIPELINE_STAGE_TRANSFER_BIT);

    copy(video_.y_image, 0, static_cast<uint32_t>(y->stride), y_texel, w, h);
    if (uv)
        copy(video_.uv_image, uv_offset, static_cast<uint32_t>(uv->stride), uv_texel,
             w / 2, h / 2);

    barrier(video_.y_image, VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
            VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL, VK_ACCESS_TRANSFER_WRITE_BIT,
            VK_ACCESS_SHADER_READ_BIT, VK_PIPELINE_STAGE_TRANSFER_BIT,
            VK_PIPELINE_STAGE_FRAGMENT_SHADER_BIT);
    barrier(video_.uv_image, VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
            VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL, VK_ACCESS_TRANSFER_WRITE_BIT,
            VK_ACCESS_SHADER_READ_BIT, VK_PIPELINE_STAGE_TRANSFER_BIT,
            VK_PIPELINE_STAGE_FRAGMENT_SHADER_BIT);

    vkEndCommandBuffer(upload_cmd_);

    VkSubmitInfo si{VK_STRUCTURE_TYPE_SUBMIT_INFO};
    si.commandBufferCount = 1;
    si.pCommandBuffers = &upload_cmd_;
    vkQueueSubmit(queue_, 1, &si, VK_NULL_HANDLE);
    vkQueueWaitIdle(queue_);
}

// --------------------------------------------------------------------------
// HDR toggle
// --------------------------------------------------------------------------
bool Renderer::set_hdr_output(bool on) {
    bool target = on && hdr_format_.has_value();
    if (target == hdr_output_) return false;
    hdr_output_ = target;

    vkDeviceWaitIdle(device_);
    destroy_video_pipeline();
    destroy_framebuffers();
    destroy_render_pass();
    destroy_swapchain();

    if (!create_swapchain() || !create_render_pass() || !create_framebuffers() ||
        !build_video_pipeline()) {
        CC_ERROR("failed to reconfigure swapchain for HDR toggle");
        return true;
    }

    // The ImGui Vulkan backend bakes in the render pass; re-init it.
    if (imgui_ready_) {
        ImGui_ImplVulkan_Shutdown();
        ImGui_ImplVulkan_InitInfo info{};
        info.ApiVersion = VK_API_VERSION_1_2;
        info.Instance = instance_;
        info.PhysicalDevice = physical_;
        info.Device = device_;
        info.QueueFamily = queue_family_;
        info.Queue = queue_;
        info.DescriptorPool = imgui_pool_;
        info.RenderPass = render_pass_;
        info.MinImageCount = 2;
        info.ImageCount = static_cast<uint32_t>(swap_images_.size());
        info.MSAASamples = VK_SAMPLE_COUNT_1_BIT;
        ImGui_ImplVulkan_Init(&info);
    }
    CC_INFO("reconfigured swapchain: hdr_output=%d", hdr_output_ ? 1 : 0);
    return true;
}

// --------------------------------------------------------------------------
// ImGui
// --------------------------------------------------------------------------
void Renderer::init_imgui() {
    // Descriptor pool for the ImGui backend.
    VkDescriptorPoolSize sizes[] = {
        {VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER, 64}};
    VkDescriptorPoolCreateInfo dpci{VK_STRUCTURE_TYPE_DESCRIPTOR_POOL_CREATE_INFO};
    dpci.flags = VK_DESCRIPTOR_POOL_CREATE_FREE_DESCRIPTOR_SET_BIT;
    dpci.maxSets = 64;
    dpci.poolSizeCount = 1;
    dpci.pPoolSizes = sizes;
    vkCreateDescriptorPool(device_, &dpci, nullptr, &imgui_pool_);

    // SDL3 defines VK_NO_PROTOTYPES, so the ImGui Vulkan backend uses a
    // function-pointer table that must be populated before init.
    ImGui_ImplVulkan_LoadFunctions(
        VK_API_VERSION_1_2,
        [](const char* fn, void* user) {
            return vkGetInstanceProcAddr(static_cast<VkInstance>(user), fn);
        },
        instance_);

    ImGui_ImplSDL3_InitForVulkan(window_);
    ImGui_ImplVulkan_InitInfo info{};
    info.ApiVersion = VK_API_VERSION_1_2;
    info.Instance = instance_;
    info.PhysicalDevice = physical_;
    info.Device = device_;
    info.QueueFamily = queue_family_;
    info.Queue = queue_;
    info.DescriptorPool = imgui_pool_;
    info.RenderPass = render_pass_;
    info.MinImageCount = 2;
    info.ImageCount = static_cast<uint32_t>(swap_images_.size());
    info.MSAASamples = VK_SAMPLE_COUNT_1_BIT;
    ImGui_ImplVulkan_Init(&info);
    imgui_ready_ = true;
}

void Renderer::new_frame() {
    ImGui_ImplVulkan_NewFrame();
    ImGui_ImplSDL3_NewFrame();
    ImGui::NewFrame();
}

// --------------------------------------------------------------------------
// Render
// --------------------------------------------------------------------------
void Renderer::render() {
    // Recreate on window pixel-size change (some drivers won't report OUT_OF_DATE).
    int pw = 0, ph = 0;
    SDL_GetWindowSizeInPixels(window_, &pw, &ph);
    if (pw > 0 && ph > 0 &&
        (static_cast<uint32_t>(pw) != extent_.width ||
         static_cast<uint32_t>(ph) != extent_.height)) {
        if (!recreate_swapchain()) return;
    }

    vkWaitForFences(device_, 1, &in_flight_, VK_TRUE, UINT64_MAX);

    uint32_t image_index = 0;
    VkResult acq = vkAcquireNextImageKHR(device_, swapchain_, UINT64_MAX,
                                         image_available_, VK_NULL_HANDLE,
                                         &image_index);
    if (acq == VK_ERROR_OUT_OF_DATE_KHR) {
        recreate_swapchain();
        return;
    }
    if (acq != VK_SUCCESS && acq != VK_SUBOPTIMAL_KHR) return;

    vkResetFences(device_, 1, &in_flight_);
    vkResetCommandBuffer(command_buffer_, 0);

    VkCommandBufferBeginInfo bi{VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO};
    bi.flags = VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT;
    vkBeginCommandBuffer(command_buffer_, &bi);

    VkClearValue clear{};
    clear.color = {{0.0f, 0.0f, 0.0f, 1.0f}};
    VkRenderPassBeginInfo rp{VK_STRUCTURE_TYPE_RENDER_PASS_BEGIN_INFO};
    rp.renderPass = render_pass_;
    rp.framebuffer = framebuffers_[image_index];
    rp.renderArea.extent = extent_;
    rp.clearValueCount = 1;
    rp.pClearValues = &clear;
    vkCmdBeginRenderPass(command_buffer_, &rp, VK_SUBPASS_CONTENTS_INLINE);

    VkViewport viewport{0.0f, 0.0f, static_cast<float>(extent_.width),
                        static_cast<float>(extent_.height), 0.0f, 1.0f};
    VkRect2D scissor{{0, 0}, extent_};
    vkCmdSetViewport(command_buffer_, 0, 1, &viewport);
    vkCmdSetScissor(command_buffer_, 0, 1, &scissor);

    if (video_.valid) {
        PushConstants pc{};
        float surf = static_cast<float>(extent_.width) / extent_.height;
        float vid = static_cast<float>(video_.width) / video_.height;
        if (vid > surf) {
            pc.scale[0] = 1.0f;
            pc.scale[1] = surf / vid;
        } else {
            pc.scale[0] = vid / surf;
            pc.scale[1] = 1.0f;
        }
        pc.hdr = video_hdr_ ? 1u : 0u;
        pc.hdr_output = hdr_output_ ? 1u : 0u;

        vkCmdBindPipeline(command_buffer_, VK_PIPELINE_BIND_POINT_GRAPHICS,
                          video_pipeline_);
        vkCmdBindDescriptorSets(command_buffer_, VK_PIPELINE_BIND_POINT_GRAPHICS,
                                video_pipeline_layout_, 0, 1, &video_descriptor_, 0,
                                nullptr);
        vkCmdPushConstants(command_buffer_, video_pipeline_layout_,
                           VK_SHADER_STAGE_VERTEX_BIT | VK_SHADER_STAGE_FRAGMENT_BIT, 0,
                           sizeof(PushConstants), &pc);
        vkCmdDraw(command_buffer_, 6, 1, 0, 0);
    }

    ImDrawData* draw_data = ImGui::GetDrawData();
    if (draw_data) ImGui_ImplVulkan_RenderDrawData(draw_data, command_buffer_);

    vkCmdEndRenderPass(command_buffer_);
    vkEndCommandBuffer(command_buffer_);

    VkPipelineStageFlags wait_stage = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;
    VkSubmitInfo si{VK_STRUCTURE_TYPE_SUBMIT_INFO};
    si.waitSemaphoreCount = 1;
    si.pWaitSemaphores = &image_available_;
    si.pWaitDstStageMask = &wait_stage;
    si.commandBufferCount = 1;
    si.pCommandBuffers = &command_buffer_;
    si.signalSemaphoreCount = 1;
    si.pSignalSemaphores = &render_finished_;
    vkQueueSubmit(queue_, 1, &si, in_flight_);

    VkPresentInfoKHR present{VK_STRUCTURE_TYPE_PRESENT_INFO_KHR};
    present.waitSemaphoreCount = 1;
    present.pWaitSemaphores = &render_finished_;
    present.swapchainCount = 1;
    present.pSwapchains = &swapchain_;
    present.pImageIndices = &image_index;
    VkResult pr = vkQueuePresentKHR(queue_, &present);
    if (pr == VK_ERROR_OUT_OF_DATE_KHR || pr == VK_SUBOPTIMAL_KHR) {
        recreate_swapchain();
    }
}

}  // namespace couchcast::render
