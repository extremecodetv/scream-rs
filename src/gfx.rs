//! wgpu renderer.
//!
//! The original draws with OpenGL 1.x immediate mode into a fixed 1024×768 layout. Here every
//! frame is a list of textured, coloured quads, each with one of the blend modes the engine uses
//! (`glBlendFunc` pairs). Vertices are transformed on the CPU straight to clip space, so a single
//! pipeline per blend mode serves both the 3D cube and the 2D overlay. Frames render into an
//! offscreen texture of window size; that texture doubles as the source for the engine's
//! cross-fade captures and is finally blitted to the swapchain (applying the F12 gamma boost).
//!
//! Layout: 2D coordinates are a virtual 1024×768 canvas (y down) scaled uniformly and centred
//! in the window. The 3D view is the horizontal band y = 64..704 of that canvas stretched to the
//! full window width, so wider screens see more of the panorama (Hor+).

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use glam::{Mat4, Vec3, Vec4};
use winit::window::Window;

pub const VIRTUAL_W: f32 = 1024.0;
pub const VIRTUAL_H: f32 = 768.0;
/// Height of the black bars above and below the 3D view, in virtual pixels.
pub const BAR: f32 = 64.0;

/// Texture slots, numbered like the original's `g_tex[]` table so code ports 1:1.
pub mod slot {
    pub const FACE: u32 = 0; // 0..5: node faces, SC_FRONT-1 .. SC_DOWN-1
    pub const MASK: u32 = 6; // 6..11: node masks
    pub const CURSOR_MENU: u32 = 0x12;
    pub const CURSOR_NORMAL: u32 = 0x13;
    pub const CURSOR_FORWARD: u32 = 0x14;
    pub const CURSOR_LOOK: u32 = 0x15;
    pub const CURSOR_USE: u32 = 0x16;
    pub const CURSOR_BACK: u32 = 0x17;
    pub const FONT: u32 = 0x18;
    pub const FONT_DEFAULT: u32 = 0x19;
    pub const VIDEO: u32 = 0x1a;
    pub const ZOOM: u32 = 0x1b;
    pub const OVERLAY: u32 = 0x1c;
    pub const LOGO: u32 = 0x1d;
    pub const CLOUDS1: u32 = 0x1e;
    pub const CLOUDS2: u32 = 0x1f;
    pub const WHITE: u32 = 0x20;
    pub const RAIN: u32 = 0x25;
    pub const MENU: u32 = 0x26; // 0x26..0x2c: logo, back, new, set, load, save, quit
    pub const ZOOM_MASK: u32 = 0x38;
    pub const CURSOR_WAIT: u32 = 0x39;
    pub const CURSOR_LEFT: u32 = 0x3a;
    pub const CURSOR_RIGHT: u32 = 0x3b;
    pub const CURSOR_UP: u32 = 0x3c;
    pub const CURSOR_DOWN: u32 = 0x3d;
    pub const GRADIENT: u32 = 0x3e;
    pub const ICON: u32 = 0x40; // 0x40 + item id
    /// Frame captured for cross-fades (the original's slot 0x22).
    pub const CAPTURE: u32 = 0x22;
    pub const COUNT: usize = 0x80;
}

/// `glBlendFunc` factors used by the engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Factor {
    Zero,
    One,
    SrcColor,
    SrcAlpha,
    OneMinusSrcAlpha,
    DstColor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Blend(pub Factor, pub Factor);

impl Blend {
    pub const ALPHA: Blend = Blend(Factor::SrcAlpha, Factor::OneMinusSrcAlpha);
    pub const ADD: Blend = Blend(Factor::One, Factor::One);
    pub const ALPHA_ADD: Blend = Blend(Factor::SrcAlpha, Factor::One);
    pub const MULTIPLY: Blend = Blend(Factor::DstColor, Factor::Zero);

    fn to_wgpu(self) -> wgpu::BlendState {
        let f = |f: Factor| match f {
            Factor::Zero => wgpu::BlendFactor::Zero,
            Factor::One => wgpu::BlendFactor::One,
            Factor::SrcColor => wgpu::BlendFactor::Src,
            Factor::SrcAlpha => wgpu::BlendFactor::SrcAlpha,
            Factor::OneMinusSrcAlpha => wgpu::BlendFactor::OneMinusSrcAlpha,
            Factor::DstColor => wgpu::BlendFactor::Dst,
        };
        let c = wgpu::BlendComponent { src_factor: f(self.0), dst_factor: f(self.1), operation: wgpu::BlendOperation::Add };
        wgpu::BlendState { color: c, alpha: c }
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    pos: [f32; 4],
    uv: [f32; 2],
    color: [f32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Viewport {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

struct Batch {
    viewport: Viewport,
    blend: Blend,
    easu: bool,
    tex: u32,
    first: u32,
    count: u32,
}

pub struct Texture {
    _texture: wgpu::Texture,
    bind: wgpu::BindGroup,
    pub width: u32,
    pub height: u32,
}

/// RGBA8 pixels ready for upload.
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    pub fn solid(width: u32, height: u32, rgba: [u8; 4]) -> Image {
        Image { width, height, rgba: rgba.repeat((width * height) as usize) }
    }
}

/// Window → canvas mapping, recomputed on resize.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    /// How the 3D view fills the window (port option).
    pub view: crate::options::NodeView,
    pub width: f32,
    pub height: f32,
    /// Physical pixels per virtual pixel.
    pub scale: f32,
    pub offset_x: f32,
    pub offset_y: f32,
}

impl Layout {
    pub fn new(width: f32, height: f32) -> Layout {
        let scale = (width / VIRTUAL_W).min(height / VIRTUAL_H).max(1e-3);
        Layout {
            view: Default::default(),
            width,
            height,
            scale,
            offset_x: (width - VIRTUAL_W * scale) * 0.5,
            offset_y: (height - VIRTUAL_H * scale) * 0.5,
        }
    }

    /// Window pixel → virtual canvas pixel.
    pub fn to_virtual(&self, x: f32, y: f32) -> (f32, f32) {
        ((x - self.offset_x) / self.scale, (y - self.offset_y) / self.scale)
    }

    pub fn to_window(&self, x: f32, y: f32) -> (f32, f32) {
        (x * self.scale + self.offset_x, y * self.scale + self.offset_y)
    }

    /// The 3D view in window pixels: canvas band 64..704 across the window (`Wide`), the
    /// 1024-wide canvas only (`Classic`), or the whole window (`Full`).
    pub fn scene_rect(&self) -> (f32, f32, f32, f32) {
        use crate::options::NodeView;
        let top = self.offset_y + BAR * self.scale;
        let h = (VIRTUAL_H - 2.0 * BAR) * self.scale;
        match self.view {
            NodeView::Wide => (0.0, top, self.width, h),
            NodeView::Classic => (self.offset_x, top, VIRTUAL_W * self.scale, h),
            NodeView::Full => (0.0, 0.0, self.width, self.height),
        }
    }

    /// Vertical field of view for the scene rect: `Full` keeps the picture scale of the band
    /// and shows more above and below it.
    pub fn scene_fov(&self, fov: f32) -> f32 {
        if self.view != crate::options::NodeView::Full {
            return fov;
        }
        let band = (VIRTUAL_H - 2.0 * BAR) * self.scale;
        let t = (fov.to_radians() / 2.0).tan() * self.height / band.max(1.0);
        (2.0 * t.atan()).to_degrees()
    }

    /// The plain canvas mapping as an affine.
    pub fn canvas(&self) -> Affine {
        Affine { sx: self.scale, sy: self.scale, ox: self.offset_x, oy: self.offset_y }
    }

    /// Placement of a flat picture covering canvas rect `src` (port options for close-ups and
    /// videos): None keeps the original layout.
    pub fn fit(&self, src: [f32; 4], mode: crate::options::FlatScale) -> Option<Affine> {
        use crate::options::FlatScale;
        let (sw, sh) = (src[2] - src[0], src[3] - src[1]);
        if sw <= 0.0 || sh <= 0.0 {
            return None;
        }
        let (kx, ky) = match mode {
            FlatScale::Original => return None,
            FlatScale::Fit => {
                let k = (self.width / sw).min(self.height / sh);
                (k, k)
            }
            FlatScale::Fill => {
                let k = (self.width / sw).max(self.height / sh);
                (k, k)
            }
            FlatScale::Stretch => (self.width / sw, self.height / sh),
        };
        let ox = (self.width - sw * kx) * 0.5 - src[0] * kx;
        let oy = (self.height - sh * ky) * 0.5 - src[1] * ky;
        Some(Affine { sx: kx, sy: ky, ox, oy })
    }
}

/// Canvas → window mapping that replaces the layout for scaled flat pictures.
#[derive(Clone, Copy, Debug)]
pub struct Affine {
    pub sx: f32,
    pub sy: f32,
    pub ox: f32,
    pub oy: f32,
}

impl Affine {
    pub fn map(&self, x: f32, y: f32) -> (f32, f32) {
        (x * self.sx + self.ox, y * self.sy + self.oy)
    }

    pub fn unmap(&self, x: f32, y: f32) -> (f32, f32) {
        ((x - self.ox) / self.sx, (y - self.oy) / self.sy)
    }
}

pub struct Gfx {
    pub window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    layout_bg: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    shader: wgpu::ShaderModule,
    pipelines: HashMap<(Blend, bool), wgpu::RenderPipeline>,
    /// Port option "texture filter: FSR 1": EASU for big pictures and RCAS on the frame.
    pub fsr: bool,
    blit: wgpu::RenderPipeline,
    blit_uniform: wgpu::Buffer,
    blit_uniform_bg: wgpu::BindGroup,
    sampler_clamp: wgpu::Sampler,
    sampler_repeat: wgpu::Sampler,
    frame: Texture,
    frame_view: wgpu::TextureView,
    frame_texture: wgpu::Texture,
    textures: Vec<Option<Texture>>,
    vertices: Vec<Vertex>,
    batches: Vec<Batch>,
    vbuf: wgpu::Buffer,
    vbuf_capacity: u64,
    /// Set while drawing a scaled flat picture: canvas quads go through it.
    pub content: Option<Affine>,
    pub layout: Layout,
    /// Brightness boost applied in the final blit (F12 gamma: 0, 0.33, 0.66, 1.0).
    pub gamma: f32,
    capture_pending: bool,
}

const FRAME_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

const SHADER: &str = r#"
struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs(@location(0) pos: vec4<f32>, @location(1) uv: vec2<f32>, @location(2) color: vec4<f32>) -> VOut {
    var o: VOut;
    o.pos = pos;
    o.uv = uv;
    o.color = color;
    return o;
}

@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var smp: sampler;

@fragment
fn fs(i: VOut) -> @location(0) vec4<f32> {
    return textureSample(tex, smp, i.uv) * i.color;
}

// ---- AMD FidelityFX FSR 1 (EASU), used as the magnification filter of big pictures ----

fn luma(c: vec3<f32>) -> f32 {
    return c.b * 0.5 + (c.r * 0.5 + c.g);
}

fn texel(p: vec2<i32>) -> vec3<f32> {
    let size = vec2<i32>(textureDimensions(tex));
    return textureLoad(tex, clamp(p, vec2(0), size - 1), 0).rgb;
}

fn easu_set(w: f32, la: f32, lb: f32, lc: f32, ld: f32, le: f32, acc: ptr<function, vec3<f32>>) {
    let dc = ld - lc;
    let cb = lc - lb;
    var len_x = 1.0 / max(max(abs(dc), abs(cb)), 1e-5);
    let dir_x = ld - lb;
    len_x = clamp(abs(dir_x) * len_x, 0.0, 1.0);
    len_x = len_x * len_x;
    let ec = le - lc;
    let ca = lc - la;
    var len_y = 1.0 / max(max(abs(ec), abs(ca)), 1e-5);
    let dir_y = le - la;
    len_y = clamp(abs(dir_y) * len_y, 0.0, 1.0);
    len_y = len_y * len_y;
    *acc = *acc + vec3(dir_x * w, dir_y * w, (len_x + len_y) * w);
}

fn easu_tap(off: vec2<f32>, dir: vec2<f32>, len: vec2<f32>, lob: f32, clp: f32, c: vec3<f32>, acc: ptr<function, vec4<f32>>) {
    let v = vec2(off.x * dir.x + off.y * dir.y, off.x * -dir.y + off.y * dir.x) * len;
    let d2 = min(dot(v, v), clp);
    var wb = 0.4 * d2 - 1.0;
    var wa = lob * d2 - 1.0;
    wb = wb * wb;
    wa = wa * wa;
    wb = 1.5625 * wb - 0.5625;
    let w = wb * wa;
    *acc = *acc + vec4(c * w, w);
}

fn easu(uv: vec2<f32>) -> vec3<f32> {
    let size = vec2<f32>(textureDimensions(tex));
    var pp = uv * size - 0.5;
    let fp = floor(pp);
    pp = pp - fp;
    let f0 = vec2<i32>(fp);
    let b = texel(f0 + vec2(0, -1));
    let c = texel(f0 + vec2(1, -1));
    let e = texel(f0 + vec2(-1, 0));
    let f = texel(f0 + vec2(0, 0));
    let g = texel(f0 + vec2(1, 0));
    let h = texel(f0 + vec2(2, 0));
    let i = texel(f0 + vec2(-1, 1));
    let j = texel(f0 + vec2(0, 1));
    let k = texel(f0 + vec2(1, 1));
    let l = texel(f0 + vec2(2, 1));
    let n = texel(f0 + vec2(0, 2));
    let o = texel(f0 + vec2(1, 2));
    var acc = vec3(0.0);
    easu_set((1.0 - pp.x) * (1.0 - pp.y), luma(b), luma(e), luma(f), luma(g), luma(j), &acc);
    easu_set(pp.x * (1.0 - pp.y), luma(c), luma(f), luma(g), luma(h), luma(k), &acc);
    easu_set((1.0 - pp.x) * pp.y, luma(f), luma(i), luma(j), luma(k), luma(n), &acc);
    easu_set(pp.x * pp.y, luma(g), luma(j), luma(k), luma(l), luma(o), &acc);
    var dir = acc.xy;
    let dir2 = dir * dir;
    var dir_r = dir2.x + dir2.y;
    let zero = dir_r < 1.0 / 32768.0;
    dir_r = select(inverseSqrt(max(dir_r, 1e-12)), 1.0, zero);
    dir.x = select(dir.x, 1.0, zero);
    dir = dir * dir_r;
    var len = acc.z * 0.5;
    len = len * len;
    let stretch = dot(dir, dir) / max(max(abs(dir.x), abs(dir.y)), 1e-5);
    let len2 = vec2(1.0 + (stretch - 1.0) * len, 1.0 - 0.5 * len);
    let lob = 0.5 + ((1.0 / 4.0 - 0.04) - 0.5) * len;
    let clp = 1.0 / lob;
    let mn = min(min(f, g), min(j, k));
    let mx = max(max(f, g), max(j, k));
    var a = vec4(0.0);
    easu_tap(vec2(0.0, -1.0) - pp, dir, len2, lob, clp, b, &a);
    easu_tap(vec2(1.0, -1.0) - pp, dir, len2, lob, clp, c, &a);
    easu_tap(vec2(-1.0, 1.0) - pp, dir, len2, lob, clp, i, &a);
    easu_tap(vec2(0.0, 1.0) - pp, dir, len2, lob, clp, j, &a);
    easu_tap(vec2(0.0, 0.0) - pp, dir, len2, lob, clp, f, &a);
    easu_tap(vec2(-1.0, 0.0) - pp, dir, len2, lob, clp, e, &a);
    easu_tap(vec2(1.0, 1.0) - pp, dir, len2, lob, clp, k, &a);
    easu_tap(vec2(2.0, 1.0) - pp, dir, len2, lob, clp, l, &a);
    easu_tap(vec2(2.0, 0.0) - pp, dir, len2, lob, clp, h, &a);
    easu_tap(vec2(1.0, 0.0) - pp, dir, len2, lob, clp, g, &a);
    easu_tap(vec2(1.0, 2.0) - pp, dir, len2, lob, clp, o, &a);
    easu_tap(vec2(0.0, 2.0) - pp, dir, len2, lob, clp, n, &a);
    return min(mx, max(mn, a.rgb / a.w));
}

@fragment
fn fs_easu(i: VOut) -> @location(0) vec4<f32> {
    let bilinear = textureSample(tex, smp, i.uv);
    // Texels per screen pixel: EASU only where the picture is magnified.
    let footprint = fwidth(i.uv * vec2<f32>(textureDimensions(tex)));
    if max(footprint.x, footprint.y) > 1.0 {
        return bilinear * i.color;
    }
    return vec4(easu(i.uv), bilinear.a) * i.color;
}

struct Blit { boost: f32, sharpness: f32, _p1: f32, _p2: f32 };
@group(1) @binding(0) var<uniform> blit: Blit;

@vertex
fn vs_blit(@builtin(vertex_index) i: u32) -> VOut {
    var p = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    var o: VOut;
    o.pos = vec4(p[i], 0.0, 1.0);
    o.uv = vec2((p[i].x + 1.0) * 0.5, (1.0 - p[i].y) * 0.5);
    o.color = vec4(1.0);
    return o;
}

// FSR 1 RCAS: contrast-adaptive sharpening of the finished frame (sharpness 0 = off).
fn rcas(p: vec2<i32>) -> vec3<f32> {
    let b = texel(p + vec2(0, -1));
    let d = texel(p + vec2(-1, 0));
    let e = texel(p);
    let f = texel(p + vec2(1, 0));
    let h = texel(p + vec2(0, 1));
    let bl = luma(b);
    let dl = luma(d);
    let el = luma(e);
    let fl = luma(f);
    let hl = luma(h);
    var nz = 0.25 * (bl + dl + fl + hl) - el;
    let range = max(max(max(bl, dl), max(el, fl)), hl) - min(min(min(bl, dl), min(el, fl)), hl);
    nz = clamp(abs(nz) / max(range, 1e-5), 0.0, 1.0);
    nz = -0.5 * nz + 1.0;
    let mn4 = min(min(b, d), min(f, h));
    let mx4 = max(max(b, d), max(f, h));
    let hit_min = min(mn4, e) / max(4.0 * mx4, vec3(1e-5));
    let hit_max = (1.0 - max(mx4, e)) / min(4.0 * mn4 - 4.0, vec3(-1e-5));
    let lobe3 = max(-hit_min, hit_max);
    var lobe = max(-(0.25 - 1.0 / 16.0), min(max(max(lobe3.r, lobe3.g), lobe3.b), 0.0)) * blit.sharpness;
    lobe = lobe * nz;
    return (lobe * (b + d + f + h) + e) / (4.0 * lobe + 1.0);
}

@fragment
fn fs_blit(i: VOut) -> @location(0) vec4<f32> {
    var c = textureSample(tex, smp, i.uv).rgb;
    if blit.sharpness > 0.0 {
        c = rcas(vec2<i32>(i.pos.xy));
    }
    return vec4(min(c * (1.0 + blit.boost), vec3(1.0)), 1.0);
}
"#;

impl Gfx {
    pub fn new(window: Arc<Window>, vsync: bool) -> Result<Gfx> {
        let size = window.inner_size();
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let surface = instance.create_surface(window.clone())?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .map_err(|e| anyhow!("no GPU adapter: {e}"))?;
        log::info!("GPU: {:?}", adapter.get_info());
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("scream"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
            memory_hints: wgpu::MemoryHints::default(),
            trace: wgpu::Trace::Off,
        }))?;

        let caps = surface.get_capabilities(&adapter);
        let format = caps.formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(caps.formats[0]);
        let present_mode = if vsync { wgpu::PresentMode::AutoVsync } else { wgpu::PresentMode::AutoNoVsync };
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&device, &config);

        let layout_bg = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("texture"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blit uniform"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("quads"),
            bind_group_layouts: &[&layout_bg],
            push_constant_ranges: &[],
        });
        let blit_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("blit"),
            bind_group_layouts: &[&layout_bg, &uniform_bgl],
            push_constant_ranges: &[],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scream"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let blit = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("blit"),
            layout: Some(&blit_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_blit"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_blit"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        let blit_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("blit uniform"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let blit_uniform_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("blit uniform"),
            layout: &uniform_bgl,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: blit_uniform.as_entire_binding() }],
        });
        let sampler = |mode| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                address_mode_u: mode,
                address_mode_v: mode,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            })
        };
        let sampler_clamp = sampler(wgpu::AddressMode::ClampToEdge);
        let sampler_repeat = sampler(wgpu::AddressMode::Repeat);
        let vbuf_capacity = 1 << 20;
        let vbuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vertices"),
            size: vbuf_capacity,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let (frame_texture, frame_view, frame) =
            Self::make_frame(&device, &layout_bg, &sampler_clamp, config.width, config.height);
        let mut gfx = Gfx {
            window,
            surface,
            device,
            queue,
            config,
            layout_bg,
            pipeline_layout,
            shader,
            pipelines: HashMap::new(),
            fsr: false,
            blit,
            blit_uniform,
            blit_uniform_bg,
            sampler_clamp,
            sampler_repeat,
            frame,
            frame_view,
            frame_texture,
            textures: (0..slot::COUNT).map(|_| None).collect(),
            vertices: Vec::new(),
            batches: Vec::new(),
            vbuf,
            vbuf_capacity,
            content: None,
            layout: Layout::new(size.width as f32, size.height as f32),
            gamma: 0.0,
            capture_pending: false,
        };
        gfx.upload(slot::WHITE, &Image::solid(4, 4, [255; 4]));
        gfx.upload(slot::CAPTURE, &Image::solid(4, 4, [0, 0, 0, 255]));
        Ok(gfx)
    }

    fn make_frame(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        w: u32,
        h: u32,
    ) -> (wgpu::Texture, wgpu::TextureView, Texture) {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("frame"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FRAME_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frame"),
            layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
            ],
        });
        let tex2 = texture.clone();
        (tex2, view, Texture { _texture: texture, bind, width: w, height: h })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        let (t, v, f) = Self::make_frame(&self.device, &self.layout_bg, &self.sampler_clamp, width, height);
        self.frame_texture = t;
        self.frame_view = v;
        self.frame = f;
        let view = self.layout.view;
        self.layout = Layout::new(width as f32, height as f32);
        self.layout.view = view;
    }

    fn pipeline(&mut self, blend: Blend, easu: bool) -> &wgpu::RenderPipeline {
        let (device, layout, shader) = (&self.device, &self.pipeline_layout, &self.shader);
        self.pipelines.entry((blend, easu)).or_insert_with(|| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("quads"),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x2, 2 => Float32x4],
                    }],
                },
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some(if easu { "fs_easu" } else { "fs" }),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: FRAME_FORMAT,
                        blend: Some(blend.to_wgpu()),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            })
        })
    }

    /// Uploads (or replaces) a texture slot.
    pub fn upload(&mut self, slot: u32, img: &Image) {
        let size = wgpu::Extent3d { width: img.width.max(1), height: img.height.max(1), depth_or_array_layers: 1 };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &img.rgba,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * size.width), rows_per_image: None },
            size,
        );
        let view = texture.create_view(&Default::default());
        let repeat = matches!(slot, slot::CLOUDS1 | slot::CLOUDS2 | slot::RAIN);
        let sampler = if repeat { &self.sampler_repeat } else { &self.sampler_clamp };
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layout_bg,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
            ],
        });
        self.textures[slot as usize] = Some(Texture { _texture: texture, bind, width: img.width, height: img.height });
    }

    pub fn clear_slot(&mut self, slot: u32) {
        self.textures[slot as usize] = None;
    }

    pub fn loaded(&self, slot: u32) -> bool {
        self.textures.get(slot as usize).is_some_and(|t| t.is_some())
    }

    pub fn size(&self, slot: u32) -> Option<(u32, u32)> {
        self.textures.get(slot as usize)?.as_ref().map(|t| (t.width, t.height))
    }

    // ---------------------------------------------------------------- drawing

    fn push(&mut self, viewport: Viewport, blend: Blend, tex: u32, quad: [Vertex; 4]) {
        let tex = if self.loaded(tex) { tex } else { slot::WHITE };
        // EASU suits the photographic pictures only (faces, close-ups, overlays, videos).
        let easu = self.fsr
            && (tex < slot::MASK || matches!(tex, slot::ZOOM | slot::OVERLAY | slot::LOGO | slot::VIDEO));
        let first = self.vertices.len() as u32;
        let [a, b, c, d] = quad;
        self.vertices.extend_from_slice(&[a, b, c, a, c, d]);
        if let Some(last) = self.batches.last_mut() {
            if last.viewport == viewport && last.blend == blend && last.tex == tex && last.easu == easu && last.first + last.count == first {
                last.count += 6;
                return;
            }
        }
        self.batches.push(Batch { viewport, blend, easu, tex, first, count: 6 });
    }

    fn full_viewport(&self) -> Viewport {
        Viewport { x: 0.0, y: 0.0, w: self.layout.width, h: self.layout.height }
    }

    /// Quad in window pixels, corners clockwise from top-left with their texture coordinates.
    pub fn quad_window(&mut self, tex: u32, blend: Blend, color: [f32; 4], pts: [(f32, f32); 4], uvs: [(f32, f32); 4]) {
        let (w, h) = (self.layout.width, self.layout.height);
        let v = |i: usize| Vertex {
            pos: [pts[i].0 / w * 2.0 - 1.0, 1.0 - pts[i].1 / h * 2.0, 0.0, 1.0],
            uv: [uvs[i].0, uvs[i].1],
            color,
        };
        let vp = self.full_viewport();
        self.push(vp, blend, tex, [v(0), v(1), v(2), v(3)]);
    }

    /// Axis-aligned quad on the virtual 1024×768 canvas.
    pub fn quad(&mut self, tex: u32, blend: Blend, color: [f32; 4], rect: [f32; 4], uv: [f32; 4]) {
        let l = self.layout;
        let content = self.content;
        let p = |x, y| match content {
            Some(a) => a.map(x, y),
            None => l.to_window(x, y),
        };
        let [x0, y0, x1, y1] = rect;
        let [u0, v0, u1, v1] = uv;
        self.quad_window(tex, blend, color, [p(x0, y0), p(x1, y0), p(x1, y1), p(x0, y1)], [
            (u0, v0),
            (u1, v0),
            (u1, v1),
            (u0, v1),
        ]);
    }

    /// Fills the whole window (letterbox areas included).
    pub fn quad_fullscreen(&mut self, tex: u32, blend: Blend, color: [f32; 4]) {
        let (w, h) = (self.layout.width, self.layout.height);
        self.quad_window(tex, blend, color, [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)], [
            (0.0, 0.0),
            (1.0, 0.0),
            (1.0, 1.0),
            (0.0, 1.0),
        ]);
    }

    /// Window rect of 3D drawing: the scene rect, or for a close-up the band of its picture
    /// (the sky behind the zoom mask follows the picture).
    pub fn scene_view(&self) -> (f32, f32, f32, f32) {
        match self.content {
            Some(a) => {
                let (x0, y0) = a.map(0.0, BAR);
                let (x1, y1) = a.map(VIRTUAL_W, VIRTUAL_H - BAR);
                (x0, y0, x1 - x0, y1 - y0)
            }
            None => self.layout.scene_rect(),
        }
    }

    /// 3D quad seen through `view_proj`, drawn into the scene band.
    pub fn quad_3d(&mut self, tex: u32, blend: Blend, color: [f32; 4], view_proj: &Mat4, pts: [Vec3; 4], uvs: [(f32, f32); 4]) {
        let (x, y, w, h) = self.scene_view();
        let (fw, fh) = (self.layout.width, self.layout.height);
        let inside = x >= -0.5 && y >= -0.5 && x + w <= fw + 0.5 && y + h <= fh + 0.5;
        // Viewports must lie inside the target; a rect reaching past the window (a cropped
        // close-up) is drawn through the full viewport with the mapping folded into clip space.
        let (vp, k) = if inside {
            (Viewport { x, y, w, h }, None)
        } else {
            let k = (w / fw, h / fh, (2.0 * x + w) / fw - 1.0, 1.0 - (2.0 * y + h) / fh);
            (self.full_viewport(), Some(k))
        };
        let v = |i: usize| {
            let mut p: Vec4 = *view_proj * pts[i].extend(1.0);
            if let Some((sx, sy, cx, cy)) = k {
                p.x = p.x * sx + cx * p.w;
                p.y = p.y * sy + cy * p.w;
            }
            Vertex { pos: p.to_array(), uv: [uvs[i].0, uvs[i].1], color }
        };
        self.push(vp, blend, tex, [v(0), v(1), v(2), v(3)]);
    }

    /// Requests a copy of the frame being built (before UI that should not fade is drawn the
    /// caller simply calls this at the right moment; the copy happens when the frame is flushed).
    pub fn request_capture(&mut self) {
        self.capture_pending = true;
    }

    /// Renders the queued quads into the frame texture (cleared to black first) and, when
    /// `present`, blits it to the window.
    pub fn flush(&mut self, present: bool) {
        // Ensure every batch's pipeline exists before borrowing for the pass.
        let keys: Vec<(Blend, bool)> = self.batches.iter().map(|b| (b.blend, b.easu)).collect();
        for (b, e) in keys {
            self.pipeline(b, e);
        }
        let bytes: &[u8] = bytemuck::cast_slice(&self.vertices);
        if bytes.len() as u64 > self.vbuf_capacity {
            self.vbuf_capacity = (bytes.len() as u64).next_power_of_two();
            self.vbuf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("vertices"),
                size: self.vbuf_capacity,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !bytes.is_empty() {
            self.queue.write_buffer(&self.vbuf, 0, bytes);
        }
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("frame"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.frame_view,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_vertex_buffer(0, self.vbuf.slice(..));
            let (fw, fh) = (self.config.width as f32, self.config.height as f32);
            for b in &self.batches {
                let vp = b.viewport;
                let (x, y) = (vp.x.clamp(0.0, fw - 1.0), vp.y.clamp(0.0, fh - 1.0));
                let (w, h) = (vp.w.min(fw - x).max(1.0), vp.h.min(fh - y).max(1.0));
                pass.set_viewport(x, y, w, h, 0.0, 1.0);
                pass.set_pipeline(&self.pipelines[&(b.blend, b.easu)]);
                let tex = self.textures[b.tex as usize].as_ref().unwrap();
                pass.set_bind_group(0, &tex.bind, &[]);
                pass.draw(b.first..b.first + b.count, 0..1);
            }
        }
        if self.capture_pending {
            self.capture_pending = false;
            self.copy_frame_to_capture(&mut encoder);
        }
        let surface_tex = if present {
            match self.surface.get_current_texture() {
                Ok(t) => Some(t),
                Err(e) => {
                    log::warn!("surface lost: {e}");
                    self.surface.configure(&self.device, &self.config);
                    None
                }
            }
        } else {
            None
        };
        if let Some(st) = &surface_tex {
            self.queue.write_buffer(&self.blit_uniform, 0, bytemuck::cast_slice(&[self.gamma, if self.fsr { 0.8 } else { 0.0 }, 0.0, 0.0]));
            let view = st.texture.create_view(&Default::default());
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("blit"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.blit);
            pass.set_bind_group(0, &self.frame.bind, &[]);
            pass.set_bind_group(1, &self.blit_uniform_bg, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit(Some(encoder.finish()));
        if let Some(st) = surface_tex {
            self.window.pre_present_notify();
            st.present();
        }
        self.vertices.clear();
        self.batches.clear();
    }

    fn copy_frame_to_capture(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let (w, h) = (self.config.width, self.config.height);
        let need_new = self.size(slot::CAPTURE) != Some((w, h));
        if need_new {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("capture"),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FRAME_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("capture"),
                layout: &self.layout_bg,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler_clamp) },
                ],
            });
            self.textures[slot::CAPTURE as usize] = Some(Texture { _texture: texture, bind, width: w, height: h });
        }
        let dst = &self.textures[slot::CAPTURE as usize].as_ref().unwrap()._texture;
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.frame_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo { texture: dst, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
    }

    /// Reads the last rendered frame back (for `--shot`).
    pub fn read_frame(&mut self) -> Image {
        let (w, h) = (self.config.width, self.config.height);
        let row = (4 * w).div_ceil(256) * 256;
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (row * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.frame_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: None },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.queue.submit(Some(encoder.finish()));
        buf.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        let _ = self.device.poll(wgpu::PollType::Wait);
        let data = buf.slice(..).get_mapped_range();
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            let start = (y * row) as usize;
            rgba.extend(data[start..start + (w * 4) as usize].chunks_exact(4).flat_map(|p| [p[0], p[1], p[2], 255]));
        }
        Image { width: w, height: h, rgba }
    }
}
