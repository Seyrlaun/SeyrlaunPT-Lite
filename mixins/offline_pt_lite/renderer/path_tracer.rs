use super::Frame;
use crate::{
    assets::{Assets, RenderAssets},
    chunk::Section, world::World,
};
use glam::{DVec3, Mat4, Vec3, Vec4};
use horizon_core::position::BlockPos;
use horizon_items::blocks::{BlockFacts, Fluid};
use horizon_world::registry::BlockStateId;
use std::{
    collections::{HashMap, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
    sync::Arc, time::{Duration, Instant},
};
use wgpu::util::DeviceExt;
const SIDE: u32 = 128;
const BRICKS: u32 = SIDE / 16;
const HISTORY: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;
const EMISSION_POWER: f32 = 36.;
const DISPLAY_BASELINE_EXPOSURE_EV: f32 = 1.0;
fn display_exposure_multiplier(exposure_ev: i32) -> f32 {
    2_f32.powf(DISPLAY_BASELINE_EXPOSURE_EV + exposure_ev as f32 / 10.)
}
fn sun_direction(angle_degrees: f32) -> Vec3 {
    let a = angle_degrees.to_radians();
    Vec3::new(-a.sin(), a.cos(), 0.)
}
fn scene_origin(camera: DVec3, current: Option<[i32; 3]>) -> [i32; 3] {
    let next = camera
        .floor()
        .as_ivec3()
        .to_array()
        .map(|v| v.div_euclid(16) * 16 - SIDE as i32 / 2);
    let Some(origin) = current else {
        return next;
    };
    let camera = camera.to_array();
    std::array::from_fn(|axis| {
        if camera[axis] >= f64::from(origin[axis] + 48)
            && camera[axis] < f64::from(origin[axis] + 80)
        {
            origin[axis]
        } else {
            next[axis]
        }
    })
}
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Material {
    range: [u32; 4],
    tint_emission: [f32; 4],
    surface: [f32; 4],
    absorption: [f32; 4],
}
fn surface_properties(name: &str) -> ([f32; 4], [f32; 4]) {
    let name = name.trim_start_matches("minecraft:");
    if matches!(name, "water" | "flowing_water") {
        return ([0.02, 0.0, 1.0, 1.3333], [0.180, 0.073, 0.045, 0.0]);
    }
    if matches!(name, "lava" | "flowing_lava") {
        return ([0.12, 0.0, 1.0, 1.30], [1.35, 0.24, 0.035, 0.0]);
    }
    if name == "glass" || name == "glass_pane" || name.ends_with("_stained_glass")
        || name.ends_with("_stained_glass_pane")
    {
        let colors = [
            ("white", [0.92_f32, 0.94, 0.96]),
            ("orange", [0.95, 0.45, 0.08]),
            ("magenta", [0.85, 0.16, 0.65]),
            ("light_blue", [0.35, 0.72, 0.95]),
            ("yellow", [0.95, 0.85, 0.12]),
            ("lime", [0.45, 0.85, 0.10]),
            ("pink", [0.95, 0.48, 0.65]),
            ("gray", [0.30, 0.34, 0.36]),
            ("light_gray", [0.65, 0.68, 0.70]),
            ("cyan", [0.10, 0.65, 0.72]),
            ("purple", [0.48, 0.15, 0.75]),
            ("blue", [0.12, 0.28, 0.85]),
            ("brown", [0.42, 0.24, 0.10]),
            ("green", [0.23, 0.45, 0.10]),
            ("red", [0.85, 0.10, 0.06]),
            ("black", [0.08, 0.09, 0.10]),
        ];
        let tint = colors
            .iter()
            .find_map(|(dye, color)| {
                name.strip_prefix(dye)
                    .filter(|suffix| suffix.starts_with("_stained_glass"))
                    .map(|_| *color)
            })
            .unwrap_or([0.995; 3]);
        let absorption = tint.map(|v| -v.ln());
        return (
            [0.0, 0.0, 1.0, 1.5],
            [
                absorption[0],
                absorption[1],
                absorption[2],
                f32::from(name.ends_with("_pane")),
            ],
        );
    }
    let bare = name.strip_prefix("waxed_").unwrap_or(name);
    let metallic = matches!(
        bare, "gold_block" | "iron_block" | "iron_bars" | "iron_door" | "iron_trapdoor"
    )
        || (bare.contains("copper") && !bare.contains("ore") && !bare.contains("raw_")
            && !bare.contains("oxidized"));
    let roughness = if metallic {
        if bare == "gold_block" {
            0.16
        } else if bare.contains("weathered") {
            0.60
        } else if bare.contains("exposed") {
            0.42
        } else {
            0.24
        }
    } else if bare.starts_with("polished_") || bare.ends_with("_glazed_terracotta")
        || bare == "quartz_block" || bare == "smooth_quartz"
    {
        0.26
    } else {
        0.85
    };
    ([roughness, f32::from(metallic), 0.0, 1.5], [0.0; 4])
}
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Triangle {
    p0: [f32; 4],
    e1: [f32; 4],
    e2: [f32; 4],
    uv0_uv1: [f32; 4],
    uv2_layer: [f32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Light {
    cell: [u32; 4],
    sampling: [f32; 4],
}
fn light_distribution(
    cells: &[[u32; 4]],
    materials: &[Material],
    triangles: &[Triangle],
    eye: Vec3,
) -> Vec<Light> {
    let weights: Vec<f64> = cells
        .iter()
        .map(|cell| {
            let mat = materials[cell[3] as usize];
            let area_power: f32 = triangles[mat.range[0]
                    as usize..(mat.range[0] + mat.range[1]) as usize]
                .iter()
                .map(|tri| {
                    let area = Vec3::from_slice(&tri.e1)
                        .cross(Vec3::from_slice(&tri.e2))
                        .length() * 0.5;
                    area * mat.tint_emission[3].max(tri.e1[3])
                })
                .sum();
            let center = Vec3::new(cell[0] as f32, cell[1] as f32, cell[2] as f32)
                + Vec3::splat(0.5);
            f64::from(area_power.max(1e-6) / (center.distance_squared(eye) + 4.))
        })
        .collect();
    let total: f64 = weights.iter().sum();
    let mut cumulative = 0_f64;
    let mut previous = 0_f32;
    cells
        .iter()
        .zip(weights)
        .enumerate()
        .map(|(i, (&cell, weight))| {
            cumulative += 0.1 / cells.len() as f64 + 0.9 * weight / total;
            let cdf = if i + 1 == cells.len() { 1. } else { cumulative as f32 };
            let probability = cdf - previous;
            previous = cdf;
            Light {
                cell,
                sampling: [cdf, probability, 0., 0.],
            }
        })
        .collect()
}
fn emission_strength(level: u8) -> f32 {
    EMISSION_POWER * (f32::from(level) / 15.).powi(2)
}
/// Original authored emission rules. Model-provided emissive layers take
/// precedence over block-wide gameplay light levels.
fn emission_properties(
    name: &str,
    sprite: &str,
    block_light: u8,
    face_light: u8,
    explicit_surfaces: bool,
) -> (f32, f32) {
    let strength = emission_strength;
    if explicit_surfaces {
        return (strength(face_light), 0.);
    }
    if block_light == 0 {
        return (0., 0.);
    }
    let name = name.trim_start_matches("minecraft:");
    if name.ends_with("torch") {
        return (strength(block_light) * 4., 1.);
    }
    if name == "nether_portal" {
        return (strength(15) * 3., 0.);
    }
    if name.ends_with("lantern") && name != "sea_lantern" {
        return (strength(block_light) * 3., 2.);
    }
    if name.ends_with("campfire") {
        return if sprite.ends_with("_fire") {
            (strength(block_light), 0.)
        } else {
            (0., 0.)
        };
    }
    if name == "firefly_bush" {
        return if sprite.ends_with("_emissive") { (strength(15), 0.) } else { (0., 0.) };
    }
    (strength(block_light), 0.)
}
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    inverse: [[f32; 4]; 4],
    camera: [f32; 4],
    sun: [f32; 4],
    sky: [f32; 4],
    image: [u32; 4],
    options: [u32; 4],
    optical: [f32; 4],
    origin: [i32; 4],
}
struct CachedSection {
    source: Option<Arc<Section>>,
    cells: Box<[u32; 4096]>,
    lights: Vec<[u32; 4]>,
}
#[derive(Clone, Copy)]
struct RayCamera {
    eye: DVec3,
    inverse: [[f32; 4]; 4],
}
impl RayCamera {
    fn new(camera: DVec3, projection: Mat4, mut view: Mat4) -> Self {
        let eye = camera + view.inverse().transform_point3(Vec3::ZERO).as_dvec3();
        view.w_axis = Vec4::W;
        Self {
            eye,
            inverse: (projection * view).inverse().to_cols_array_2d(),
        }
    }
    fn delta(self, other: Self) -> (f64, f32) {
        let direction = |camera: Self, x, y| {
            let p = Mat4::from_cols_array_2d(&camera.inverse) * Vec4::new(x, y, 1., 1.);
            (p.truncate() / p.w).normalize()
        };
        let angle = [(0., 0.), (-1., -1.), (-1., 1.), (1., -1.), (1., 1.)]
            .into_iter()
            .map(|(x, y)| direction(self, x, y).distance(direction(other, x, y)))
            .fold(0_f32, f32::max);
        (self.eye.distance(other.eye), angle)
    }
    fn differs(self, other: Self) -> bool {
        let (translation, angle) = self.delta(other);
        translation > 0.001 || angle > 0.0001
    }
}
#[derive(Clone, Copy)]
struct Lighting {
    sun: Vec3,
    sky: Vec3,
    power: f32,
}
#[derive(Default)]
struct Accumulation {
    camera: Option<RayCamera>,
    lighting: Option<Lighting>,
    live_lighting: Option<Lighting>,
    samples: u32,
    peak: u32,
    resets: [u32; 8],
    last_reset: Option<ResetReason>,
    last_discarded: u32,
    camera_delta: (f64, f32),
    scene_note: String,
    report_at: Option<Instant>,
    animation_tick: Option<u64>,
}
#[derive(Clone, Copy, Debug)]
enum ResetReason {
    Camera,
    Lighting,
    Scene,
    CacheOrigin,
    Assets,
    Resolution,
    Settings,
    Toggle,
}
impl Accumulation {
    fn holds_scene(&self, camera: RayCamera, freeze: bool) -> bool {
        freeze && self.samples > 0 && self.camera.is_some_and(|old| !old.differs(camera))
    }
    fn reset(&mut self, reason: ResetReason) {
        self.resets[reason as usize] += 1;
        self.last_reset = Some(reason);
        self.last_discarded = self.samples;
        self.samples = 0;
    }
    fn animation_tick(
        &mut self,
        live_tick: u64,
        camera: RayCamera,
        freeze: bool,
    ) -> u64 {
        if !freeze || self.samples == 0
            || self.camera.is_none_or(|old| old.differs(camera))
        {
            self.animation_tick = Some(live_tick);
        }
        *self.animation_tick.get_or_insert(live_tick)
    }
    fn prepare(
        &mut self,
        camera: RayCamera,
        lighting: Lighting,
        freeze: bool,
    ) -> Lighting {
        let moved = self.camera.is_none_or(|old| old.differs(camera));
        if moved {
            self.camera_delta = self.camera.map_or((0., 0.), |old| old.delta(camera));
            self.reset(ResetReason::Camera);
            self.camera = Some(camera);
        }
        if let Some(old) = self.live_lighting {
            let angle = old.sun.distance_squared(lighting.sun);
            let sky = (old.sky - lighting.sky).abs().max_element();
            let power = (old.power - lighting.power).abs();
            if angle > 0.0025 || sky > 0.15 || power > 1.0 {
                self.reset(ResetReason::Lighting);
            } else if !freeze && (angle > 1e-12 || sky > 1e-5 || power > 1e-5) {
                self.samples = self.samples.min(127);
            }
        }
        self.live_lighting = Some(lighting);
        if freeze && self.samples > 0 && let Some(held) = self.lighting {
            return held;
        }
        self.lighting = Some(lighting);
        lighting
    }
    fn advance(&mut self) {
        self.samples = (self.samples + 1).min(4095);
        self.peak = self.peak.max(self.samples);
    }
    fn report(&mut self) {
        let now = Instant::now();
        let previous = self.report_at.get_or_insert(now);
        if now.duration_since(*previous) < Duration::from_secs(10) {
            return;
        }
        *previous = now;
        log::info!(
            "Path tracer history: samples={} peak={} resets(camera/light/scene/origin/assets/size/quality/toggle)={:?}; last={:?} discarded={}; camera_delta={:.4}mm/{:.7}rad; last_scene={}",
            self.samples, self.peak, self.resets, self.last_reset, self.last_discarded,
            self.camera_delta.0 * 1000., self.camera_delta.1, self.scene_note
        );
    }
}
struct Targets {
    size: [u32; 2],
    trace: [wgpu::BindGroup; 2],
    show: [wgpu::BindGroup; 2],
    history: [wgpu::Texture; 2],
}
pub(super) struct PathTracer {
    voxel: wgpu::Texture,
    occupancy: wgpu::Texture,
    scene_layout: wgpu::BindGroupLayout,
    scene_group: Option<wgpu::BindGroup>,
    sprite_names: Vec<String>,
    materials: Vec<Material>,
    known: Vec<bool>,
    visual_states: Vec<u32>,
    visual_keys: HashMap<u64, Vec<u32>>,
    triangles: Vec<Triangle>,
    material_buffer: wgpu::Buffer,
    triangle_buffer: wgpu::Buffer,
    light_buffer: wgpu::Buffer,
    pbr_buffer: wgpu::Buffer,
    light_count: u32,
    light_eye: Option<DVec3>,
    uniform: wgpu::Buffer,
    uniform_group: wgpu::BindGroup,
    trace_layout: wgpu::BindGroupLayout,
    show_layout: wgpu::BindGroupLayout,
    trace_pipeline: wgpu::ComputePipeline,
    show_pipeline: wgpu::RenderPipeline,
    targets: Option<Targets>,
    cache: HashMap<[i32; 3], CachedSection>,
    origin: Option<[i32; 3]>,
    assets_key: Option<Arc<RenderAssets>>,
    accumulation: Accumulation,
    frame_index: u32,
    scale: f32,
    bounces: u32,
    debug: u32,
    denoise: u32,
    exposure: f32,
    exposure_ev: i32,
    spectral: f32,
    freeze: bool,
    hold_scene: bool,
    interaction_until: u64,
    native_capture: NativeCapturePipeline,
    native_capture_uniform: Option<Uniform>,
}
fn storage(device: &wgpu::Device, label: &str, bytes: &[u8]) -> wgpu::Buffer {
    device
        .create_buffer_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: if bytes.is_empty() { &[0; 16] } else { bytes },
                usage: wgpu::BufferUsages::STORAGE,
            },
        )
}
#[cfg(test)]
pub(super) fn verify_material_handoff(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    render: &RenderAssets,
    sprites: &[wgpu::Texture],
) {
    let native_extra = render.model_catalog.native_pt_extra();
    let mut mapped = native_extra.pbr.words.clone();
    mapped.resize(mapped.len().max(4), 0);
    mapped[3] = mapped.len() as u32;
    mapped.extend_from_slice(&native_extra.ctm.words);
    mapped.push(0);
    let bytes = mapped.len() as u64 * 4;
    assert!(bytes <= device.limits().max_buffer_size);
    assert!(bytes <= device.limits().max_storage_buffer_binding_size);
    let mut tracer = PathTracer::new(
            device,
            sprites,
            super::sprite_split(),
            wgpu::TextureFormat::Rgba8Unorm,
        )
        .unwrap();
    tracer.pbr_buffer = storage(
        device,
        "Full-resolution material handoff",
        bytemuck::cast_slice(&mapped),
    );
    tracer.bind_scene(device, sprites);
    let layers: Vec<_> = (0..render.texture_layers as usize)
        .filter(|&layer| mapped[layer * 4 + 2] != 0)
        .collect();
    assert!(! layers.is_empty(), "the pack must contain material maps");
    let mut offsets = vec![0, 3, mapped.len() as u32 / 2, mapped.len() as u32 - 1];
    for &layer in [layers[0], layers[layers.len() / 2], *layers.last().unwrap()].iter() {
        let header = layer * 4;
        let side = mapped[header + 1];
        assert_eq!(side, render.sprite_size, "material layer {layer} lost resolution");
        offsets
            .extend(
                (header as u32..header as u32 + 4)
                    .chain([
                        mapped[header],
                        mapped[header] + side * side / 2,
                        mapped[header] + side * side - 1,
                    ]),
            );
    }
    let module = device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Material storage readback"),
            source: wgpu::ShaderSource::Wgsl(
                "
            @group(0) @binding(0) var<storage, read> materials: array<u32>;
            @group(0) @binding(1) var<storage, read> offsets: array<u32>;
            @group(0) @binding(2) var<storage, read_write> output: array<u32>;
            @compute @workgroup_size(1)
            fn main(@builtin(global_invocation_id) id: vec3<u32>) {
                output[id.x] = materials[offsets[id.x]];
            }"
                    .into(),
            ),
        });
    let pipeline = device
        .create_compute_pipeline(
            &wgpu::ComputePipelineDescriptor {
                label: Some("Material storage readback"),
                layout: None,
                module: &module,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                cache: None,
            },
        );
    let indices = storage(
        device,
        "Material read offsets",
        bytemuck::cast_slice(&offsets),
    );
    let output = device
        .create_buffer(
            &wgpu::BufferDescriptor {
                label: Some("Material sampled words"),
                size: offsets.len() as u64 * 4,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            },
        );
    let read = device
        .create_buffer(
            &wgpu::BufferDescriptor {
                label: Some("Material readback"),
                size: output.size(),
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
        );
    let group = device
        .create_bind_group(
            &wgpu::BindGroupDescriptor {
                label: Some("Material readback bindings"),
                layout: &pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: tracer.pbr_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: indices.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: output.as_entire_binding(),
                    },
                ],
            },
        );
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups(offsets.len() as u32, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&output, 0, &read, 0, output.size());
    queue.submit([encoder.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    read.slice(..)
        .map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    rx.recv().unwrap().unwrap();
    let expected: Vec<_> = offsets
        .iter()
        .map(|&offset| mapped[offset as usize])
        .collect();
    {
        let data = read.slice(..).get_mapped_range().unwrap();
        assert_eq!(bytemuck::cast_slice::< u8, u32 > (& data), expected);
    }
    read.unmap();
    eprintln!(
        "Material handoff: {bytes} bytes bound to native PT, {} GPU words verified; storage limit {} bytes",
        offsets.len(), device.limits().max_storage_buffer_binding_size
    );
}
fn texture(
    device: &wgpu::Device,
    label: &str,
    size: [u32; 3],
    format: wgpu::TextureFormat,
    volume: bool,
) -> wgpu::Texture {
    device
        .create_texture(
            &wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: size[0],
                    height: size[1],
                    depth_or_array_layers: size[2],
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: if volume {
                    wgpu::TextureDimension::D3
                } else {
                    wgpu::TextureDimension::D2
                },
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | if volume {
                        wgpu::TextureUsages::COPY_DST
                    } else {
                        wgpu::TextureUsages::STORAGE_BINDING
                    }
                    | if cfg!(test) {
                        wgpu::TextureUsages::COPY_SRC
                    } else {
                        wgpu::TextureUsages::empty()
                    },
                view_formats: &[],
            },
        )
}
fn texture_entry(
    binding: u32,
    dimension: wgpu::TextureViewDimension,
    sample_type: wgpu::TextureSampleType,
    visibility: wgpu::ShaderStages,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Texture {
            sample_type,
            view_dimension: dimension,
            multisampled: false,
        },
        count: None,
    }
}
fn buffer_entry(
    binding: u32,
    ty: wgpu::BufferBindingType,
    visibility: wgpu::ShaderStages,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}
fn number(name: &str, default: f32, min: f32, max: f32) -> f32 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .filter(|v| v.is_finite())
        .unwrap_or(default)
        .clamp(min, max)
}
fn shader_source(arrays: usize, split: u32) -> String {
    let mut bindings = String::new();
    let mut cases = String::new();
    for i in 0..arrays {
        bindings
            += &format!(
                "@group(1) @binding({}) var sprites_{i}: texture_2d_array<f32>;\n", i + 5
            );
        cases
            += &format!(
                "case {i}u: {{ return textureSampleLevel(sprites_{i}, sprite_sampler, safe_uv, i32(layer % {split}u), 0.0); }}\n"
            );
    }
    let sampler = arrays + 5;
    let sample = format!(
        "{bindings}\n@group(1) @binding({sampler}) var sprite_sampler: sampler;\n\
        @group(1) @binding({}) var<storage, read> pbr_data: array<u32>;\n\
        fn texel(layer: u32, uv: vec2<f32>) -> vec4<f32> {{
            let safe_uv=clamp(uv,vec2<f32>(0.000001),vec2<f32>(0.999999));
            switch layer / {split}u {{ {cases} default: {{ return vec4<f32>(1.0, 0.0, 1.0, 1.0); }} }}
        }}\n",
        arrays + 6
    );
    offline_pt_lite_shaders::PATH_TRACER
        .replace("// SPRITE_BINDINGS", &sample)
        .replace("// CTM_CODE", offline_pt_lite_shaders::CTM)
}
impl PathTracer {
    pub fn invalidate(&mut self) {
        self.accumulation.reset(ResetReason::Toggle);
    }
    pub fn configure(
        &mut self,
        percent: u32,
        bounces: u32,
        freeze: bool,
        exposure_ev: i32,
    ) {
        let scale = percent.clamp(25, 100) as f32 / 100.;
        let bounces = bounces.clamp(1, 8);
        if self.scale != scale || self.bounces != bounces || self.freeze != freeze {
            self.accumulation.reset(ResetReason::Settings);
            self.scale = scale;
            self.bounces = bounces;
            self.freeze = freeze;
        }
        self.exposure_ev = exposure_ev.clamp(-40, 40);
    }
    pub fn prepare_scene(&mut self, frame: &Frame, size: [u32; 2], interacting: bool) {
        let size = size.map(|v| ((v as f32 * self.scale).round() as u32).max(1));
        self.capture_scene(Self::ray_camera(frame, size), frame.ticks, interacting);
    }
    fn capture_scene(&mut self, camera: RayCamera, tick: u64, interacting: bool) {
        if interacting {
            self.interaction_until = tick.saturating_add(20);
        }
        self.hold_scene = tick > self.interaction_until
            && self.accumulation.holds_scene(camera, self.freeze);
    }
    pub fn new(
        device: &wgpu::Device,
        sprites: &[wgpu::Texture],
        split: u32,
        format: wgpu::TextureFormat,
    ) -> Result<Self, String> {
        use wgpu::{ShaderStages as S, TextureViewDimension as D};
        if device.limits().max_storage_textures_per_shader_stage < 2 {
            return Err("path tracing needs two compute storage textures".into());
        }
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Experimental world path tracer"),
                source: wgpu::ShaderSource::Wgsl(
                    shader_source(sprites.len(), split).into(),
                ),
            });
        let uniform_layout = device
            .create_bind_group_layout(
                &wgpu::BindGroupLayoutDescriptor {
                    label: Some("Path tracer frame"),
                    entries: &[
                        buffer_entry(
                            0,
                            wgpu::BufferBindingType::Uniform,
                            S::COMPUTE | S::FRAGMENT,
                        ),
                    ],
                },
            );
        let mut entries = vec![
            texture_entry(0, D::D3, wgpu::TextureSampleType::Uint, S::COMPUTE),
            texture_entry(1, D::D3, wgpu::TextureSampleType::Uint, S::COMPUTE),
            buffer_entry(2, wgpu::BufferBindingType::Storage { read_only : true },
            S::COMPUTE,), buffer_entry(3, wgpu::BufferBindingType::Storage { read_only :
            true }, S::COMPUTE,), buffer_entry(4, wgpu::BufferBindingType::Storage {
            read_only : true }, S::COMPUTE,),
        ];
        for i in 0..sprites.len() {
            entries
                .push(
                    texture_entry(
                        i as u32 + 5,
                        D::D2Array,
                        wgpu::TextureSampleType::Float {
                            filterable: true,
                        },
                        S::COMPUTE,
                    ),
                );
        }
        entries
            .push(wgpu::BindGroupLayoutEntry {
                binding: sprites.len() as u32 + 5,
                visibility: S::COMPUTE,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            });
        entries
            .push(
                buffer_entry(
                    sprites.len() as u32 + 6,
                    wgpu::BufferBindingType::Storage {
                        read_only: true,
                    },
                    S::COMPUTE,
                ),
            );
        let scene_layout = device
            .create_bind_group_layout(
                &wgpu::BindGroupLayoutDescriptor {
                    label: Some("Path tracer scene"),
                    entries: &entries,
                },
            );
        let float = wgpu::TextureSampleType::Float {
            filterable: false,
        };
        let write = |binding, format| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: S::COMPUTE,
            ty: wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format,
                view_dimension: D::D2,
            },
            count: None,
        };
        let trace_layout = device
            .create_bind_group_layout(
                &wgpu::BindGroupLayoutDescriptor {
                    label: Some("Path tracer history"),
                    entries: &[
                        texture_entry(0, D::D2, float, S::COMPUTE),
                        write(1, HISTORY),
                        write(2, wgpu::TextureFormat::Rgba16Float),
                    ],
                },
            );
        let layout = device
            .create_pipeline_layout(
                &wgpu::PipelineLayoutDescriptor {
                    label: Some("Path tracer compute"),
                    bind_group_layouts: &[
                        Some(&uniform_layout),
                        Some(&scene_layout),
                        Some(&trace_layout),
                    ],
                    ..Default::default()
                },
            );
        let trace_pipeline = device
            .create_compute_pipeline(
                &wgpu::ComputePipelineDescriptor {
                    label: Some("Path trace terrain"),
                    layout: Some(&layout),
                    module: &module,
                    entry_point: Some("trace_frame"),
                    compilation_options: Default::default(),
                    cache: None,
                },
            );
        let show_layout = device
            .create_bind_group_layout(
                &wgpu::BindGroupLayoutDescriptor {
                    label: Some("Path tracer resolve"),
                    entries: &[
                        texture_entry(0, D::D2, float, S::FRAGMENT),
                        texture_entry(1, D::D2, float, S::FRAGMENT),
                    ],
                },
            );
        let layout = device
            .create_pipeline_layout(
                &wgpu::PipelineLayoutDescriptor {
                    label: Some("Path tracer display"),
                    bind_group_layouts: &[Some(&uniform_layout), Some(&show_layout)],
                    ..Default::default()
                },
            );
        let show_pipeline = device
            .create_render_pipeline(
                &wgpu::RenderPipelineDescriptor {
                    label: Some("Path tracer resolve"),
                    layout: Some(&layout),
                    vertex: wgpu::VertexState {
                        module: &module,
                        entry_point: Some("vs_show"),
                        compilation_options: Default::default(),
                        buffers: &[],
                    },
                    primitive: Default::default(),
                    depth_stencil: None,
                    multisample: Default::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &module,
                        entry_point: Some("fs_show"),
                        compilation_options: Default::default(),
                        targets: &[
                            Some(wgpu::ColorTargetState {
                                format,
                                blend: None,
                                write_mask: wgpu::ColorWrites::ALL,
                            }),
                        ],
                    }),
                    multiview_mask: None,
                    cache: None,
                },
            );
        let native_capture = NativeCapturePipeline::new(
            device,
            &module,
            &uniform_layout,
            &scene_layout,
        );
        if let Some(error) = pollster::block_on(scope.pop()) {
            return Err(error.to_string());
        }
        let uniform = device
            .create_buffer(
                &wgpu::BufferDescriptor {
                    label: Some("Path tracer frame"),
                    size: size_of::<Uniform>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                },
            );
        let uniform_group = device
            .create_bind_group(
                &wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &uniform_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: uniform.as_entire_binding(),
                        },
                    ],
                },
            );
        Ok(Self {
            voxel: texture(
                device,
                "Path tracer blocks",
                [SIDE; 3],
                wgpu::TextureFormat::R32Uint,
                true,
            ),
            occupancy: texture(
                device,
                "Path tracer section occupancy",
                [BRICKS; 3],
                wgpu::TextureFormat::R32Uint,
                true,
            ),
            scene_layout,
            scene_group: None,
            sprite_names: Vec::new(),
            materials: Vec::new(),
            triangles: Vec::new(),
            known: Vec::new(),
            visual_states: Vec::new(),
            visual_keys: HashMap::new(),
            material_buffer: storage(device, "Path tracer materials", &[0; 64]),
            triangle_buffer: storage(device, "Path tracer model triangles", &[0; 80]),
            light_buffer: storage(device, "Path tracer emitters", &[0; 32]),
            pbr_buffer: storage(device, "Path tracer empty PBR", &[0; 16]),
            light_count: 0,
            light_eye: None,
            uniform,
            uniform_group,
            trace_layout,
            show_layout,
            trace_pipeline,
            show_pipeline,
            targets: None,
            cache: HashMap::new(),
            origin: None,
            assets_key: None,
            accumulation: Accumulation::default(),
            frame_index: 0,
            scale: number("HORIZON_PT_SCALE", 1.0, 0.2, 1.0),
            bounces: number("HORIZON_PT_BOUNCES", 4.0, 1.0, 8.0) as u32,
            debug: number("HORIZON_PT_DEBUG", 0.0, 0.0, 5.0) as u32,
            denoise: number("HORIZON_PT_DENOISE", 1.0, 0.0, 1.0) as u32,
            exposure: number("HORIZON_PT_EXPOSURE", 1.0, 0.05, 8.0),
            exposure_ev: 0,
            spectral: number("HORIZON_PT_SPECTRAL", 0.0, 0.0, 1.0),
            freeze: true,
            hold_scene: false,
            interaction_until: 0,
            native_capture: native_capture,
            native_capture_uniform: None,
        })
    }
    fn material(
        &mut self,
        id: u32,
        assets: &Assets,
        render: &RenderAssets,
        facts: &BlockFacts,
    ) {
        let native_extra = render.model_catalog.native_pt_extra();
        let i = id as usize;
        if self.known[i] {
            return;
        }
        self.known[i] = true;
        let Some(state) = assets.registry.state(BlockStateId(id)) else {
            return;
        };
        let fact = &facts.states[i];
        let mut tint = [1.0; 3];
        let name = state.name.as_str();
        if name.contains("leaves") || name.contains("vine") {
            tint = [0.32, 0.58, 0.13];
        }
        if name.contains("grass") || name.contains("fern") {
            tint = [0.40, 0.65, 0.18];
        }
        let mut emission = 0_f32;
        let start = self.triangles.len() as u32;
        let mut models = Vec::new();
        let _ = render
            .model_catalog
            .models_at(BlockStateId(id), BlockPos::new(0, 0, 0), &mut models);
        let explicit_surfaces = models
            .iter()
            .any(|model| model.faces.iter().any(|face| face.quad.light_emission > 0));
        for model in models {
            for face in &model.faces {
                let sprite = self
                    .sprite_names
                    .get(face.layer as usize)
                    .map_or("", String::as_str);
                let (authored_emission, mask) = emission_properties(
                    name,
                    sprite,
                    fact.emission,
                    face.quad.light_emission,
                    explicit_surfaces,
                );
                let program = native_extra
                    .ctm
                    .program(BlockStateId(id), face.layer, face.quad.facing);
                let mapped_emission = native_extra
                    .ctm
                    .possible_layers(program, face.layer)
                    .into_iter()
                    .map(|layer| native_extra.pbr.emission_peak(layer) * EMISSION_POWER)
                    .fold(0_f32, f32::max);
                let surface_emission = authored_emission.max(mapped_emission);
                emission = emission.max(surface_emission);
                for indices in [[0, 1, 2], [0, 2, 3]] {
                    let [a, b, c] = indices.map(|j| Vec3::from(face.quad.positions[j]));
                    if (b - a).cross(c - a).length_squared() < 1e-12 {
                        continue;
                    }
                    let [u, v, w] = indices.map(|j| face.quad.uv[j]);
                    self.triangles
                        .push(Triangle {
                            p0: [a.x, a.y, a.z, f32::from(face.quad.tint_index >= 0)],
                            e1: [(b - a).x, (b - a).y, (b - a).z, authored_emission],
                            e2: [(c - a).x, (c - a).y, (c - a).z, program as f32],
                            uv0_uv1: [u[0], u[1], v[0], v[1]],
                            uv2_layer: [w[0], w[1], face.layer as f32, mask],
                        });
                }
            }
        }
        if self.triangles.len() as u32 == start && fact.fluid != Fluid::None {
            let flowing = fact.fluid_level < 8 || fact.fluid_falling;
            let (still, flow) = if fact.fluid == Fluid::Lava {
                ("minecraft:block/lava_still", "minecraft:block/lava_flow")
            } else {
                ("minecraft:block/water_still", "minecraft:block/water_flow")
            };
            let top_layer = render
                .sprite_layers
                .get(if flowing { flow } else { still })
                .copied()
                .or_else(|| render.sprite_layers.get(still).copied())
                .unwrap_or(0);
            let side_layer = render
                .sprite_layers
                .get(flow)
                .copied()
                .unwrap_or(top_layer);
            emission = emission_strength(fact.emission);
            let height = if fact.fluid_falling {
                1.0
            } else {
                (f32::from(fact.fluid_level.max(1)) / 9.0).clamp(1.0 / 9.0, 1.0)
            };
            let p = [
                [0., height, 0.],
                [0., height, 1.],
                [1., height, 1.],
                [1., height, 0.],
            ];
            for k in [[0, 1, 2], [0, 2, 3]] {
                let [a, b, c] = k.map(|j| Vec3::from(p[j]));
                self.triangles
                    .push(Triangle {
                        p0: [a.x, a.y, a.z, 1.],
                        e1: [(b - a).x, (b - a).y, (b - a).z, emission],
                        e2: [(c - a).x, (c - a).y, (c - a).z, 0.],
                        uv0_uv1: [a.x, a.z, b.x, b.z],
                        uv2_layer: [c.x, c.z, top_layer as f32, 0.],
                    });
            }
            let sides = [
                [[0., 0., 0.], [0., height, 0.], [1., height, 0.], [1., 0., 0.]],
                [[1., 0., 1.], [1., height, 1.], [0., height, 1.], [0., 0., 1.]],
                [[0., 0., 1.], [0., height, 1.], [0., height, 0.], [0., 0., 0.]],
                [[1., 0., 0.], [1., height, 0.], [1., height, 1.], [1., 0., 1.]],
            ];
            for face in sides {
                for k in [[0, 1, 2], [0, 2, 3]] {
                    let [a, b, c] = k.map(|j| Vec3::from(face[j]));
                    self.triangles
                        .push(Triangle {
                            p0: [a.x, a.y, a.z, 1.],
                            e1: [(b - a).x, (b - a).y, (b - a).z, emission],
                            e2: [(c - a).x, (c - a).y, (c - a).z, 0.],
                            uv0_uv1: [a.x, a.y, b.x, b.y],
                            uv2_layer: [c.x, c.y, side_layer as f32, 0.],
                        });
                }
            }
            if fact.fluid == Fluid::Water {
                tint = [0.18, 0.46, 0.82];
            }
        }
        let (surface, absorption) = surface_properties(name);
        self.materials[i] = Material {
            range: [
                start,
                self.triangles.len() as u32 - start,
                1,
                native_extra.ctm.neighbor_masks.get(i).copied().unwrap_or(0),
            ],
            tint_emission: [tint[0], tint[1], tint[2], emission],
            surface,
            absorption,
        };
        let material = &self.materials[i];
        let bytes = &bytemuck::bytes_of(material)[4..];
        let triangles = &self.triangles[start as usize..];
        let mut hash = DefaultHasher::new();
        bytes.hash(&mut hash);
        bytemuck::cast_slice::<_, u8>(triangles).hash(&mut hash);
        let key = hash.finish();
        let same = self
            .visual_keys
            .get(&key)
            .and_then(|ids| {
                ids.iter()
                    .copied()
                    .find(|&id| {
                        let old = &self.materials[id as usize];
                        bytes == &bytemuck::bytes_of(old)[4..]
                            && bytemuck::cast_slice::<_, u8>(triangles)
                                == bytemuck::cast_slice::<
                                    _,
                                    u8,
                                >(
                                    &self
                                        .triangles[old.range[0]
                                        as usize..(old.range[0] + old.range[1]) as usize],
                                )
                    })
            });
        if let Some(id) = same {
            self.visual_states[i] = id;
            self.materials[i] = self.materials[id as usize];
            self.triangles.truncate(start as usize);
        } else {
            self.visual_keys.entry(key).or_default().push(id);
        }
    }
    pub fn sync(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        world: &World,
        camera: DVec3,
        assets: &Assets,
        render: &Arc<RenderAssets>,
        facts: &BlockFacts,
        sprites: &[wgpu::Texture],
    ) {
        let native_extra = render.model_catalog.native_pt_extra();
        let started = Instant::now();
        let origin = scene_origin(camera, self.origin);
        let assets_changed = self
            .assets_key
            .as_ref()
            .is_none_or(|a| !Arc::ptr_eq(a, render));
        if self.hold_scene && !assets_changed && self.scene_group.is_some() {
            return;
        }
        if assets_changed {
            let mut mapped = native_extra.pbr.words.clone();
            mapped.resize(mapped.len().max(4), 0);
            mapped[3] = mapped.len() as u32;
            mapped.extend_from_slice(&native_extra.ctm.words);
            mapped.push(0);
            self.pbr_buffer = storage(
                device,
                "Path tracer linear labPBR",
                bytemuck::cast_slice(&mapped),
            );
            self.assets_key = Some(render.clone());
            self.sprite_names = vec![String::new(); render.texture_layers as usize];
            for (name, &layer) in &render.sprite_layers {
                if let Some(slot) = self.sprite_names.get_mut(layer as usize) {
                    slot.clone_from(name);
                }
            }
            self.materials = vec![Material::zeroed(); assets.registry.len()];
            self.known = vec![false; assets.registry.len()];
            self.visual_states = (0..assets.registry.len() as u32).collect();
            self.visual_keys.clear();
            self.triangles.clear();
            self.cache.clear();
        }
        let origin_changed = self.origin != Some(origin);
        if origin_changed {
            self.cache.clear();
            self.origin = Some(origin);
        }
        let mut full_voxel = (assets_changed || origin_changed)
            .then(|| vec![0_u32; (SIDE * SIDE * SIDE) as usize]);
        let mut full_occupancy = (assets_changed || origin_changed)
            .then(|| vec![0_u32; (BRICKS * BRICKS * BRICKS) as usize]);
        let triangle_count = self.triangles.len();
        let mut changed = false;
        let mut changed_sections = 0;
        let mut changed_cells = 0;
        let mut first_change = None;
        for by in 0..BRICKS as i32 {
            for bz in 0..BRICKS as i32 {
                for bx in 0..BRICKS as i32 {
                    let p = [
                        origin[0] + bx * 16,
                        origin[1] + by * 16,
                        origin[2] + bz * 16,
                    ];
                    let section = world
                        .chunks
                        .get(&(p[0] >> 4, p[2] >> 4))
                        .and_then(|c| {
                            c.section_index(p[1]).and_then(|sy| c.sections.get(sy))
                        });
                    let same = self
                        .cache
                        .get(&p)
                        .is_some_and(|old| match (&old.source, section) {
                            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                            (None, None) => true,
                            _ => false,
                        });
                    if same {
                        continue;
                    }
                    let mut cells = Box::new([world.air.0; 4096]);
                    if let Some(section) = section {
                        section.unpack(&mut cells);
                    }
                    if let Some(old) = self.cache.get_mut(&p) && old.cells == cells {
                        old.source = section.cloned();
                        continue;
                    }
                    for &id in cells.iter() {
                        self.material(id, assets, render, facts);
                    }
                    if let Some(old) = self.cache.get_mut(&p) {
                        let mut visible_changes = 0;
                        for (j, (&a, &b)) in old
                            .cells
                            .iter()
                            .zip(cells.iter())
                            .enumerate()
                        {
                            if self.visual_states[a as usize]
                                != self.visual_states[b as usize]
                            {
                                visible_changes += 1;
                                first_change.get_or_insert((p, j, a, b));
                            }
                        }
                        if visible_changes == 0 {
                            old.source = section.cloned();
                            old.cells = cells;
                            continue;
                        }
                        changed_cells += visible_changes;
                    }
                    changed = true;
                    changed_sections += 1;
                    let mut lights = Vec::new();
                    let mut occupied = false;
                    for (j, &id) in cells.iter().enumerate() {
                        let m = self.materials[id as usize];
                        occupied |= m.range[1] > 0;
                        if m.range[1] > 0 && m.tint_emission[3] > 0.0 {
                            lights
                                .push([
                                    (bx * 16) as u32 + (j as u32 & 15),
                                    (by * 16) as u32 + (j as u32 >> 8),
                                    (bz * 16) as u32 + ((j as u32 >> 4) & 15),
                                    self.visual_states[id as usize],
                                ]);
                        }
                    }
                    let mut xyz = [0u32; 4096];
                    for y in 0..16 {
                        for z in 0..16 {
                            for x in 0..16 {
                                xyz[(z * 16 + y) * 16 + x] = self
                                    .visual_states[cells[(y * 16 + z) * 16 + x] as usize];
                            }
                        }
                    }
                    if let Some(volume) = &mut full_voxel {
                        for z in 0..16 {
                            for y in 0..16 {
                                let dst = (((bz as usize * 16 + z) * SIDE as usize
                                    + by as usize * 16 + y) * SIDE as usize) + bx as usize * 16;
                                let src = (z * 16 + y) * 16;
                                volume[dst..dst + 16].copy_from_slice(&xyz[src..src + 16]);
                            }
                        }
                        full_occupancy
                            .as_mut()
                            .unwrap()[((bz * BRICKS as i32 + by) * BRICKS as i32 + bx)
                            as usize] = u32::from(occupied);
                    } else {
                        queue
                            .write_texture(
                                wgpu::TexelCopyTextureInfo {
                                    texture: &self.voxel,
                                    mip_level: 0,
                                    origin: wgpu::Origin3d {
                                        x: bx as u32 * 16,
                                        y: by as u32 * 16,
                                        z: bz as u32 * 16,
                                    },
                                    aspect: wgpu::TextureAspect::All,
                                },
                                bytemuck::cast_slice(&xyz),
                                wgpu::TexelCopyBufferLayout {
                                    offset: 0,
                                    bytes_per_row: Some(64),
                                    rows_per_image: Some(16),
                                },
                                wgpu::Extent3d {
                                    width: 16,
                                    height: 16,
                                    depth_or_array_layers: 16,
                                },
                            );
                        queue
                            .write_texture(
                                wgpu::TexelCopyTextureInfo {
                                    texture: &self.occupancy,
                                    mip_level: 0,
                                    origin: wgpu::Origin3d {
                                        x: bx as u32,
                                        y: by as u32,
                                        z: bz as u32,
                                    },
                                    aspect: wgpu::TextureAspect::All,
                                },
                                bytemuck::bytes_of(&u32::from(occupied)),
                                wgpu::TexelCopyBufferLayout {
                                    offset: 0,
                                    bytes_per_row: None,
                                    rows_per_image: None,
                                },
                                wgpu::Extent3d {
                                    width: 1,
                                    height: 1,
                                    depth_or_array_layers: 1,
                                },
                            );
                    }
                    self.cache
                        .insert(
                            p,
                            CachedSection {
                                source: section.cloned(),
                                cells,
                                lights,
                            },
                        );
                }
            }
        }
        if let Some(volume) = full_voxel {
            queue
                .write_texture(
                    self.voxel.as_image_copy(),
                    bytemuck::cast_slice(&volume),
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(SIDE * 4),
                        rows_per_image: Some(SIDE),
                    },
                    self.voxel.size(),
                );
            queue
                .write_texture(
                    self.occupancy.as_image_copy(),
                    bytemuck::cast_slice(full_occupancy.as_ref().unwrap()),
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(BRICKS * 4),
                        rows_per_image: Some(BRICKS),
                    },
                    self.occupancy.size(),
                );
        }
        let sampling_changed = self
            .light_eye
            .is_none_or(|eye| eye.distance_squared(camera) > 16.);
        if !changed && !sampling_changed {
            return;
        }
        if changed {
            self.accumulation.scene_note = if let Some((p, j, a, b)) = first_change {
                let label = |id| {
                    assets
                        .registry
                        .state(BlockStateId(id))
                        .map_or_else(
                            || id.to_string(),
                            |s| format!("{}{:?}", s.name, s.properties),
                        )
                };
                format!(
                    "{changed_sections} sections/{changed_cells} cells; first at {:?}: {} -> {}",
                    [p[0] + (j as i32 & 15), p[1] + (j as i32 >> 8), p[2] + ((j as i32 >>
                    4) & 15)], label(a), label(b)
                )
            } else {
                format!("{changed_sections} populated cache sections at {origin:?}")
            };
            self.accumulation
                .reset(
                    if assets_changed {
                        ResetReason::Assets
                    } else if origin_changed {
                        ResetReason::CacheOrigin
                    } else {
                        ResetReason::Scene
                    },
                );
        }
        if assets_changed || triangle_count != self.triangles.len()
            || self.scene_group.is_none()
        {
            self.material_buffer = storage(
                device,
                "Path tracer materials",
                bytemuck::cast_slice(&self.materials),
            );
            self.triangle_buffer = storage(
                device,
                "Path tracer triangles",
                bytemuck::cast_slice(&self.triangles),
            );
        }
        let mut lights: Vec<_> = self
            .cache
            .values()
            .flat_map(|s| s.lights.iter().copied())
            .collect();
        lights.sort_unstable();
        self.light_count = lights.len() as u32;
        let distribution = light_distribution(
            &lights,
            &self.materials,
            &self.triangles,
            (camera - DVec3::from_array(origin.map(f64::from))).as_vec3(),
        );
        self.light_eye = Some(camera);
        self.light_buffer = storage(
            device,
            "Path tracer emitter cells",
            if distribution.is_empty() {
                &[0; 32]
            } else {
                bytemuck::cast_slice(&distribution)
            },
        );
        self.bind_scene(device, sprites);
        let elapsed = started.elapsed().as_secs_f64() * 1000.;
        if assets_changed || origin_changed || elapsed > 50. {
            log::info!(
                "Path tracer scene sync: {elapsed:.1} ms, {changed_sections} sections, {} triangles, {} emitters; {}",
                self.triangles.len(), self.light_count, self.accumulation.scene_note
            );
        }
    }
    fn bind_scene(&mut self, device: &wgpu::Device, sprites: &[wgpu::Texture]) {
        let voxel = self.voxel.create_view(&Default::default());
        let occupancy = self.occupancy.create_view(&Default::default());
        let views: Vec<_> = sprites
            .iter()
            .map(|t| {
                t.create_view(
                    &wgpu::TextureViewDescriptor {
                        format: Some(wgpu::TextureFormat::Rgba8UnormSrgb),
                        dimension: Some(wgpu::TextureViewDimension::D2Array),
                        usage: Some(wgpu::TextureUsages::TEXTURE_BINDING),
                        ..Default::default()
                    },
                )
            })
            .collect();
        let sampler = device
            .create_sampler(
                &wgpu::SamplerDescriptor {
                    label: Some("Path tracer nearest sprites"),
                    address_mode_u: wgpu::AddressMode::ClampToEdge,
                    address_mode_v: wgpu::AddressMode::ClampToEdge,
                    ..Default::default()
                },
            );
        let mut entries = vec![
            wgpu::BindGroupEntry { binding : 0, resource :
            wgpu::BindingResource::TextureView(& voxel), }, wgpu::BindGroupEntry {
            binding : 1, resource : wgpu::BindingResource::TextureView(& occupancy), },
            wgpu::BindGroupEntry { binding : 2, resource : self.material_buffer
            .as_entire_binding(), }, wgpu::BindGroupEntry { binding : 3, resource : self
            .triangle_buffer.as_entire_binding(), }, wgpu::BindGroupEntry { binding : 4,
            resource : self.light_buffer.as_entire_binding(), },
        ];
        for (i, view) in views.iter().enumerate() {
            entries
                .push(wgpu::BindGroupEntry {
                    binding: i as u32 + 5,
                    resource: wgpu::BindingResource::TextureView(view),
                });
        }
        entries
            .push(wgpu::BindGroupEntry {
                binding: views.len() as u32 + 5,
                resource: wgpu::BindingResource::Sampler(&sampler),
            });
        entries
            .push(wgpu::BindGroupEntry {
                binding: views.len() as u32 + 6,
                resource: self.pbr_buffer.as_entire_binding(),
            });
        self.scene_group = Some(
            device
                .create_bind_group(
                    &wgpu::BindGroupDescriptor {
                        label: Some("Path tracer scene"),
                        layout: &self.scene_layout,
                        entries: &entries,
                    },
                ),
        );
    }
    fn resize(&mut self, device: &wgpu::Device, size: [u32; 2]) {
        if self.targets.as_ref().is_some_and(|t| t.size == size) {
            return;
        }
        let make = |label, format| {
            texture(device, label, [size[0], size[1], 1], format, false)
                .create_view(&Default::default())
        };
        let history_textures: [wgpu::Texture; 2] = std::array::from_fn(|_| {
            native_capture_texture(
                device,
                "Path tracer HDR history",
                [size[0], size[1], 1],
                HISTORY,
                false,
            )
        });
        let history = history_textures
            .each_ref()
            .map(|t| t.create_view(&Default::default()));
        let guides = make(
            "Path tracer normal and depth",
            wgpu::TextureFormat::Rgba16Float,
        );
        let trace = std::array::from_fn(|i| {
            device
                .create_bind_group(
                    &wgpu::BindGroupDescriptor {
                        label: Some("Path tracer history"),
                        layout: &self.trace_layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::TextureView(
                                    &history[1 - i],
                                ),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::TextureView(&history[i]),
                            },
                            wgpu::BindGroupEntry {
                                binding: 2,
                                resource: wgpu::BindingResource::TextureView(&guides),
                            },
                        ],
                    },
                )
        });
        let show = std::array::from_fn(|i| {
            device
                .create_bind_group(
                    &wgpu::BindGroupDescriptor {
                        label: Some("Path tracer display"),
                        layout: &self.show_layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::TextureView(&history[i]),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::TextureView(&guides),
                            },
                        ],
                    },
                )
        });
        self.targets = Some(Targets {
            size,
            trace,
            show,
            history: history_textures,
        });
        log::info!("Path tracer internal resolution: {}x{}", size[0], size[1]);
        self.accumulation.reset(ResetReason::Resolution);
    }
    fn ray_camera(frame: &Frame, size: [u32; 2]) -> RayCamera {
        RayCamera::new(
            frame.camera,
            super::world_projection(
                frame.fov_degrees,
                size[0] as f32 / size[1] as f32,
                frame.depth_far,
            ),
            frame.bob * frame.spin * crate::camera::view_matrix(frame.yaw, frame.pitch),
        )
    }
    pub fn animation_tick(&mut self, frame: &Frame, size: [u32; 2]) -> u64 {
        let size = size.map(|v| ((v as f32 * self.scale).round() as u32).max(1));
        self.accumulation
            .animation_tick(frame.ticks, Self::ray_camera(frame, size), self.freeze)
    }
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        frame: &Frame,
        size: [u32; 2],
    ) {
        let size = size.map(|v| ((v as f32 * self.scale).round() as u32).max(1));
        self.resize(device, size);
        let live_camera = Self::ray_camera(frame, size);
        let sun = sun_direction(frame.sky.sun_angle);
        let origin = DVec3::from_array(self.origin.unwrap_or([0; 3]).map(f64::from));
        let sun_power = if frame.skybox {
            3.0 * (sun.y * 8.0).clamp(0.0, 1.0) * (1.0 - frame.pack.rain * 0.85)
        } else {
            0.0
        };
        let sky = if frame.skybox {
            frame.sky.sky_color.map(|v| v.max(0.0).powf(2.2))
        } else {
            [0.002; 3]
        };
        let lighting = self
            .accumulation
            .prepare(
                live_camera,
                Lighting {
                    sun,
                    sky: Vec3::from(sky),
                    power: sun_power,
                },
                self.freeze,
            );
        let ray_camera = self.accumulation.camera.unwrap_or(live_camera);
        let camera = (ray_camera.eye - origin).as_vec3();
        let uniform = Uniform {
            origin: [origin.x as i32, origin.y as i32, origin.z as i32, 0],
            inverse: ray_camera.inverse,
            camera: [camera.x, camera.y, camera.z, 0.],
            sun: [lighting.sun.x, lighting.sun.y, lighting.sun.z, lighting.power],
            sky: [lighting.sky.x, lighting.sky.y, lighting.sky.z, 0.],
            image: [size[0], size[1], self.frame_index, self.accumulation.samples],
            options: [self.bounces, self.light_count, self.debug, self.denoise],
            optical: [
                self.exposure * display_exposure_multiplier(self.exposure_ev),
                0.00465,
                EMISSION_POWER,
                self.spectral,
            ],
        };
        self.encode(device, queue, encoder, target, &uniform);
        self.frame_index = self.frame_index.wrapping_add(1);
        self.accumulation.advance();
        self.accumulation.report();
    }
    fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        uniform: &Uniform,
    ) {
        let size = [uniform.image[0], uniform.image[1]];
        self.resize(device, size);
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(uniform));
        let targets = self.targets.as_ref().unwrap();
        let i = (uniform.image[2] % 2) as usize;
        {
            let mut pass = encoder
                .begin_compute_pass(
                    &wgpu::ComputePassDescriptor {
                        label: Some("Path trace terrain"),
                        timestamp_writes: None,
                    },
                );
            pass.set_pipeline(&self.trace_pipeline);
            pass.set_bind_group(0, &self.uniform_group, &[]);
            pass.set_bind_group(1, self.scene_group.as_ref().unwrap(), &[]);
            pass.set_bind_group(2, &targets.trace[i], &[]);
            pass.dispatch_workgroups(size[0].div_ceil(8), size[1].div_ceil(8), 1);
        }
        let mut pass = encoder
            .begin_render_pass(
                &wgpu::RenderPassDescriptor {
                    label: Some("Path tracer filmic resolve"),
                    color_attachments: &[
                        Some(wgpu::RenderPassColorAttachment {
                            view: target,
                            resolve_target: None,
                            depth_slice: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                store: wgpu::StoreOp::Store,
                            },
                        }),
                    ],
                    ..Default::default()
                },
            );
        pass.set_pipeline(&self.show_pipeline);
        pass.set_bind_group(0, &self.uniform_group, &[]);
        pass.set_bind_group(1, &targets.show[i], &[]);
        pass.draw(0..3, 0..1);
        self.native_capture_uniform = Some(*uniform);
    }
}
use bytemuck::Zeroable;
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cache_origin_has_hysteresis_at_section_boundaries() {
        for edge in [-32., -16., 0., 16., 32.] {
            let initial = DVec3::new(edge - 0.00001, 68.62, 8.5);
            let origin = scene_origin(initial, None);
            let crossed = scene_origin(initial + DVec3::X * 0.00002, Some(origin));
            for i in 0..1000 {
                let jitter = if i % 2 == 0 { 0.00002 } else { 0. };
                assert_eq!(
                    scene_origin(initial + DVec3::X * jitter, Some(crossed)), crossed,
                    "tiny idle drift at a boundary must not rebuild the scene"
                );
            }
            let moved = initial + DVec3::X * 40.;
            assert_ne!(scene_origin(moved, Some(crossed)), crossed);
        }
        let mut origin = scene_origin(DVec3::splat(15.99999), None);
        for mask in 0..8 {
            let camera = DVec3::from_array(
                std::array::from_fn(|i| {
                    if mask & (1 << i) == 0 { 15.99999 } else { 16.00001 }
                }),
            );
            origin = scene_origin(camera, Some(origin));
        }
        let settled = origin;
        for mask in (0..8).cycle().take(1000) {
            let camera = DVec3::from_array(
                std::array::from_fn(|i| {
                    if mask & (1 << i) == 0 { 15.99999 } else { 16.00001 }
                }),
            );
            origin = scene_origin(camera, Some(origin));
            assert_eq!(origin, settled, "axes must retain their own hysteresis");
        }
    }
    #[test]
    fn camera_history_ignores_projection_depth_but_tracks_actual_rays() {
        let camera = DVec3::new(-37.16, 68.62, -14.08);
        let make = |far, fov, yaw| {
            RayCamera::new(
                camera,
                super::super::world_projection(fov, 16. / 9., far),
                crate::camera::view_matrix(yaw, 18.),
            )
        };
        let original = make(128., 70., 37.);
        for far in [128., 256., 512., 1024., 2048.] {
            assert!(! original.differs(make(far, 70., 37.)));
        }
        assert!(original.differs(make(128., 70.1, 37.)));
        assert!(original.differs(make(128., 70., 37.1)));
    }
    #[test]
    fn idle_camera_capture_ignores_network_jitter_but_not_slow_movement() {
        let camera = RayCamera::new(
            DVec3::ZERO,
            super::super::world_projection(70., 16. / 9., 512.),
            Mat4::IDENTITY,
        );
        let lighting = Lighting {
            sun: Vec3::Y,
            sky: Vec3::splat(0.1),
            power: 3.,
        };
        let mut history = Accumulation::default();
        history.prepare(camera, lighting, true);
        for tick in 0..2000 {
            let mut jitter = camera;
            jitter.eye.x += (f64::from(tick) * 0.37).sin() * 0.0008;
            jitter.eye.y += (f64::from(tick) * 0.23).cos() * 0.0003;
            assert!(! camera.differs(jitter));
            history.prepare(jitter, lighting, true);
            history.advance();
            assert!(history.holds_scene(jitter, true));
            assert_eq!(history.samples, tick as u32 + 1);
            assert_eq!(history.camera.unwrap().eye, camera.eye);
        }
        assert!(! history.holds_scene(camera, false));
        let mut moved = camera;
        moved.eye.x += 0.002;
        assert!(! history.holds_scene(moved, true));
        history.prepare(moved, lighting, true);
        assert_eq!(
            history.samples, 0, "slow drift must be measured from the capture anchor"
        );
    }
    #[test]
    fn emission_rules_separate_torch_wood_and_firefly_foliage() {
        let torch = emission_properties(
            "minecraft:torch",
            "minecraft:block/torch",
            14,
            0,
            false,
        );
        assert!(torch.0 > 0.);
        assert_eq!(torch.1, 1.);
        assert_eq!(
            emission_properties("minecraft:redstone_torch", "", 0, 0, false), (0., 0.)
        );
        assert_eq!(
            emission_properties("minecraft:firefly_bush", "minecraft:block/firefly_bush",
            2, 0, true), (0., 0.)
        );
        assert_eq!(
            emission_properties("minecraft:firefly_bush",
            "minecraft:block/firefly_bush_emissive", 2, 15, true), (EMISSION_POWER, 0.)
        );
        assert_eq!(
            emission_properties("minecraft:campfire", "minecraft:block/campfire_log_lit",
            15, 0, false), (0., 0.)
        );
        assert_eq!(
            emission_properties("minecraft:campfire", "minecraft:block/campfire_fire",
            15, 0, false), (EMISSION_POWER, 0.)
        );
    }
    #[test]
    fn light_distribution_favours_nearby_power_without_discarding_distant_lights() {
        let material = Material {
            range: [0, 1, 1, 0],
            ..Material::zeroed()
        };
        let triangle = Triangle {
            e1: [1., 0., 0., 12.],
            e2: [0., 1., 0., 0.],
            ..Triangle::zeroed()
        };
        let cells = [[1, 1, 1, 0], [40, 40, 40, 0], [100, 100, 100, 0]];
        let lights = light_distribution(
            &cells,
            &[material],
            &[triangle],
            Vec3::splat(1.5),
        );
        assert!(lights[0].sampling[1] > 0.8);
        assert!(lights.iter().all(| l | l.sampling[1] > 0.03));
        assert_eq!(lights.last().unwrap().sampling[0], 1.);
        assert!(
            (lights.iter().map(| l | l.sampling[1]).sum::< f32 > () - 1.).abs() < 1e-6
        );
        assert!(lights.windows(2).all(| w | w[0].sampling[0] < w[1].sampling[0]));
        let moved = light_distribution(
            &cells,
            &[material],
            &[triangle],
            Vec3::splat(100.5),
        );
        assert!(moved[2].sampling[1] > 0.8);
        assert!(
            light_distribution(& [], & [material], & [triangle], Vec3::ZERO).is_empty()
        );
    }
    #[test]
    fn bobbed_primary_rays_match_the_raster_projection() {
        let camera = DVec3::new(10_000.5, 70.0, -20_000.5);
        for far in [128.0, 512.0, 1024.0] {
            let projection = super::super::world_projection(70.0, 16.0 / 9.0, far);
            let rotation = crate::camera::view_matrix(37.0, 18.0);
            for bob in [
                Mat4::IDENTITY,
                Mat4::from_translation(Vec3::new(0.03, -0.08, 0.0)),
                Mat4::from_rotation_z(0.02)
                    * Mat4::from_translation(Vec3::new(0.03, -0.08, 0.0)),
            ] {
                let view = bob * rotation;
                let rays = RayCamera::new(camera, projection, view);
                let eye = (rays.eye - camera).as_vec3();
                assert!(eye.length() < 0.09, "bob must move the eye, not tilt the ray");
                for screen in [Vec3::new(0., 0., -8.), Vec3::new(1., -0.7, -8.)] {
                    let point = view.inverse().transform_point3(screen);
                    let projected = projection * view * point.extend(1.0);
                    let ndc = projected / projected.w;
                    let unprojected = Mat4::from_cols_array_2d(&rays.inverse)
                        * Vec4::new(ndc.x, ndc.y, 1., 1.);
                    let direction = (unprojected.truncate() / unprojected.w).normalize();
                    assert!(
                        direction.distance((point - eye).normalize()) < 2e-5,
                        "traced bob must agree with terrain and hand projection"
                    );
                }
            }
        }
    }
    #[test]
    fn sun_tracks_world_space_celestial_geometry_and_horizon() {
        for (angle, expected) in [
            (-90., Vec3::X),
            (-45., Vec3::new(0.5_f32.sqrt(), 0.5_f32.sqrt(), 0.)),
            (0., Vec3::Y),
            (90., Vec3::NEG_X),
            (180., Vec3::NEG_Y),
            (270., Vec3::X),
        ] {
            let sun = sun_direction(angle);
            assert!(sun.distance(expected) < 1e-6, "{angle}: {sun:?}");
            let celestial = Mat4::from_rotation_y(-90_f32.to_radians())
                * Mat4::from_rotation_x(angle.to_radians());
            assert!(sun.distance(celestial.transform_vector3(Vec3::Y)) < 1e-6);
            let power = (sun.y * 8.).clamp(0., 1.);
            if angle == 180. {
                assert_eq!(power, 0., "the sun must not light midnight from overhead");
            }
        }
    }
    #[test]
    fn display_exposure_keeps_relative_slider_and_restores_one_stop() {
        assert!((display_exposure_multiplier(0) - 2.0).abs() < 1e-6);
        assert!((display_exposure_multiplier(10) - 4.0).abs() < 1e-6);
        assert!((display_exposure_multiplier(- 10) - 1.0).abs() < 1e-6);
        assert!(
            (display_exposure_multiplier(20) / display_exposure_multiplier(0) - 4.0)
            .abs() < 1e-6
        );
    }
    #[test]
    fn frozen_capture_observes_time_commands_without_periodic_daylight_resets() {
        let camera = RayCamera::new(DVec3::ZERO, Mat4::IDENTITY, Mat4::IDENTITY);
        let lighting = |angle| Lighting {
            sun: sun_direction(angle),
            sky: Vec3::splat(0.1),
            power: 3.,
        };
        let mut history = Accumulation::default();
        for i in 0..2000 {
            let held = history.prepare(camera, lighting(i as f32 * 0.01), true);
            assert_eq!(held.sun, Vec3::Y);
            assert_eq!(history.samples, i);
            history.advance();
        }
        let sunset = lighting(90.);
        assert_eq!(history.prepare(camera, sunset, true).sun, sunset.sun);
        assert_eq!(history.samples, 0, "a time command must refresh a held shutter");
        history.advance();
        assert_eq!(history.prepare(camera, sunset, true).sun, sunset.sun);
        assert_eq!(history.samples, 1);
    }
    #[test]
    fn daylight_accumulates_continuously_and_frozen_lighting_converges() {
        let camera = RayCamera::new(
            DVec3::ZERO,
            super::super::world_projection(70., 1., 512.),
            Mat4::IDENTITY,
        );
        let lighting = |angle: f32| Lighting {
            sun: Vec3::new(-angle.sin(), angle.cos(), 0.),
            sky: Vec3::new(0.1, 0.2, 0.3),
            power: 5.,
        };
        let mut history = Accumulation::default();
        for i in 0..1200 {
            history.prepare(camera, lighting(i as f32 * 0.0001), false);
            assert_eq!(
                history.samples, i.min(127),
                "normal sun movement must never periodically reset convergence"
            );
            history.advance();
        }
        for _ in 0..500 {
            history.prepare(camera, lighting(1199. * 0.0001), false);
            history.advance();
        }
        assert!(history.samples > 500, "static lighting keeps gathering samples");
        let before = history.samples;
        let mut noise = camera;
        noise.eye.x += 1e-7;
        history.prepare(noise, lighting(1199. * 0.0001), false);
        assert_eq!(history.samples, before, "subpixel idle drift is harmless");
        let mut moved = camera;
        moved.eye.x += 0.01;
        history.prepare(moved, lighting(1199. * 0.0001), false);
        assert_eq!(history.samples, 0, "real movement must discard stale pixels");
        history.samples = 100;
        history.prepare(moved, lighting(1.0), false);
        assert_eq!(
            history.samples, 0, "a time command must change the image immediately"
        );
    }
    #[test]
    fn static_shutter_freezes_animation_and_daylight_until_the_camera_moves() {
        let camera = RayCamera::new(
            DVec3::ZERO,
            super::super::world_projection(70., 1., 512.),
            Mat4::IDENTITY,
        );
        let lighting = |i: u64| Lighting {
            sun: Vec3::new(-(i as f32 * 0.001).sin(), (i as f32 * 0.001).cos(), 0.),
            sky: Vec3::splat(0.1 + i as f32 * 0.0001),
            power: 3.,
        };
        let mut history = Accumulation::default();
        for i in 0..1200 {
            assert_eq!(history.animation_tick(100 + i, camera, true), 100);
            let held = history.prepare(camera, lighting(i), true);
            assert_eq!(held.sun, lighting(0).sun);
            assert_eq!(held.sky, lighting(0).sky);
            assert_eq!(history.samples, i as u32);
            history.advance();
        }
        let mut moved = camera;
        moved.eye.x += 0.01;
        assert_eq!(history.animation_tick(1300, moved, true), 1300);
        assert_eq!(history.prepare(moved, lighting(1200), true).sun, lighting(1200).sun);
        assert_eq!(history.samples, 0);
        history.advance();
        assert_eq!(history.animation_tick(1301, moved, false), 1301);
        assert_eq!(
            history.prepare(moved, lighting(1201), false).sun, lighting(1201).sun
        );
        history.reset(ResetReason::Scene);
        assert_eq!(history.animation_tick(1302, moved, true), 1302);
    }
    #[test]
    fn shader_validates_with_one_and_four_sprite_arrays() {
        for n in [1, 4] {
            let source = shader_source(n, 2048);
            let module = naga::front::wgsl::parse_str(&source)
                .unwrap_or_else(|e| panic!("{}", e.emit_to_string(& source)));
            naga::valid::Validator::new(
                    naga::valid::ValidationFlags::all(),
                    naga::valid::Capabilities::all(),
                )
                .validate(&module)
                .unwrap();
        }
        assert_eq!(size_of::< Triangle > (), 80);
        assert_eq!(size_of::< Material > (), 64);
        assert_eq!(size_of::< Uniform > (), 176);
        assert_eq!(size_of::< Light > (), 32);
    }
    #[test]
    fn gpu_visible_normal_sampling_preserves_energy_and_reduces_bright_outliers() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
            .unwrap();
        let (device, queue) = pollster::block_on(
                adapter
                    .request_device(
                        &wgpu::DeviceDescriptor {
                            required_limits: adapter.limits(),
                            ..Default::default()
                        },
                    ),
            )
            .unwrap();
        let source = shader_source(1, 2048)
            + r#"
@group(0) @binding(1) var<storage,read_write> statistics: array<vec4<f32>>;
@compute @workgroup_size(64)
fn verify_sampling(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x>=256u { return; }
    let scenario=id.x/64u;
    let n=vec3<f32>(0.,1.,0.);
    let no=select(0.05,1.,scenario>=2u);
    let wo=vec3<f32>(sqrt(1.-no*no),no,0.);
    let roughness=select(0.4,0.08,(scenario%2u)==1u);
    let surface=vec4<f32>(roughness,1.,0.04,255.);
    let albedo=vec3<f32>(0.8,0.3,0.1);
    for(var proposal=0u;proposal<2u;proposal++) {
        rng=id.x*9781u+3812015801u;
        var sum=0.;
        var square=0.;
        var peak=0.;
        for(var i=0u;i<2048u;i++) {
            var wi: vec3<f32>;
            if proposal==0u {
                let alpha=roughness*roughness;
                let u=random();
                let ct=sqrt((1.-u)/(1.+(alpha*alpha-1.)*u));
                let st=sqrt(max(0.,1.-ct*ct));
                let phi=2.*PI*random();
                wi=reflect(-wo,basis(n,vec3<f32>(st*cos(phi),st*sin(phi),ct)));
            } else { wi=sample_bsdf(wo,n,surface); }
            var contribution=0.;
            if dot(n,wi)>0. {
                let bsdf=eval_bsdf(wo,wi,n,albedo,surface);
                var pdf=bsdf.pdf;
                if proposal==0u {
                    let h=normalize(wo+wi);
                    let nh=max(0.,dot(n,h));
                    let alpha=roughness*roughness;
                    let c=cross(n,h);
                    let den=dot(c,c)+alpha*alpha*nh*nh;
                    let distribution=alpha*alpha/(PI*den*den);
                    pdf=distribution*nh/max(4.*dot(wo,h),1e-7);
                }
                contribution=bsdf.value.r*max(0.,dot(n,wi))/max(pdf,1e-20);
            }
            sum+=contribution;
            square+=contribution*contribution;
            peak=max(peak,contribution);
        }
        statistics[proposal*256u+id.x]=vec4<f32>(sum,square,peak,2048.);
    }
}
"#;
        let module = device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("BSDF energy and variance regression"),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            });
        let pipeline = device
            .create_compute_pipeline(
                &wgpu::ComputePipelineDescriptor {
                    label: Some("BSDF sampling regression"),
                    layout: None,
                    module: &module,
                    entry_point: Some("verify_sampling"),
                    compilation_options: Default::default(),
                    cache: None,
                },
            );
        let output = device
            .create_buffer(
                &wgpu::BufferDescriptor {
                    label: None,
                    size: 512 * 16,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                },
            );
        let readback = device
            .create_buffer(
                &wgpu::BufferDescriptor {
                    label: None,
                    size: output.size(),
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                },
            );
        let group = device
            .create_bind_group(
                &wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &pipeline.get_bind_group_layout(0),
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: output.as_entire_binding(),
                        },
                    ],
                },
            );
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(4, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, output.size());
        queue.submit([encoder.finish()]);
        readback.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let data = readback.slice(..).get_mapped_range().unwrap();
        let values: &[[f32; 4]] = bytemuck::cast_slice(&data);
        for scenario in 0_usize..4 {
            let moments = |proposal: usize| {
                let rows = &values[proposal * 256
                    + scenario * 64..proposal * 256 + (scenario + 1) * 64];
                let count: f64 = rows.iter().map(|v| f64::from(v[3])).sum();
                let mean = rows.iter().map(|v| f64::from(v[0])).sum::<f64>() / count;
                let variance = rows.iter().map(|v| f64::from(v[1])).sum::<f64>() / count
                    - mean * mean;
                let peak = rows.iter().map(|v| v[2]).fold(0_f32, f32::max);
                (mean, variance, peak)
            };
            let old = moments(0);
            let visible = moments(1);
            eprintln!(
                "GGX scenario {scenario}: NDF {old:?}, visible normals {visible:?}"
            );
            assert!(
                (old.0 - visible.0).abs() < 0.025, "sampling must retain mean energy"
            );
            assert!(
                visible.2 <= 1.001, "visible-normal metal paths cannot amplify energy"
            );
            if scenario == 0 {
                assert!(visible.1 < old.1 * 0.5, "grazing-highlight variance must fall");
                assert!(
                    visible.2 < old.2 * 0.25, "rare bright grazing paths must be reduced"
                );
            }
        }
    }
    #[test]
    fn vanilla_material_defaults_distinguish_glass_metal_and_ore() {
        let (glass, clear) = surface_properties("minecraft:glass");
        assert_eq!(glass, [0., 0., 1., 1.5]);
        assert!(clear[0] < 0.01 && clear[3] == 0.);
        let (pane, red) = surface_properties("minecraft:red_stained_glass_pane");
        assert_eq!(pane[2], 1.);
        assert_eq!(red[3], 1.);
        assert!(red[1] > red[0] && red[2] > red[0]);
        for name in ["gold_block", "iron_block", "waxed_cut_copper_stairs"] {
            assert_eq!(surface_properties(name).0[1], 1.);
        }
        for name in ["gold_ore", "raw_copper_block", "oxidized_copper", "stone"] {
            assert_eq!(surface_properties(name).0[1], 0.);
        }
    }
    #[test]
    #[ignore = "requires a scratch copy of prepared Minecraft resources"]
    fn scene_sync_reads_real_assets_and_tracks_block_edits() {
        let root = std::path::PathBuf::from(
            std::env::var("HORIZON_PT_TEST_RESOURCES")
                .expect("scratch prepared resources"),
        );
        let assets = Assets::load(&root).unwrap();
        let facts = BlockFacts::load(&root, assets.registry).unwrap();
        let render = Arc::new(RenderAssets::load(&root, &assets).unwrap());
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
            .unwrap();
        let (device, queue) = pollster::block_on(
                adapter
                    .request_device(
                        &wgpu::DeviceDescriptor {
                            required_limits: adapter.limits(),
                            ..Default::default()
                        },
                    ),
            )
            .unwrap();
        let sprite = device
            .create_texture(
                &wgpu::TextureDescriptor {
                    label: None,
                    size: wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::STORAGE_BINDING,
                    view_formats: &[wgpu::TextureFormat::Rgba8UnormSrgb],
                },
            );
        let sprites = [sprite];
        let mut tracer = PathTracer::new(
                &device,
                &sprites,
                2048,
                wgpu::TextureFormat::Rgba8Unorm,
            )
            .unwrap();
        let state = |name| assets.registry.default_state(name).unwrap();
        let mut world = World::new(state("minecraft:air"), 0);
        world.chunks.insert((0, 0), Arc::new(crate::chunk::Chunk::empty(0, 0)));
        world.set(8, 0, 8, state("minecraft:stone")).unwrap();
        let camera = DVec3::new(8.5, 6., 8.5);
        tracer.sync(&device, &queue, &world, camera, &assets, &render, &facts, &sprites);
        let read_cells = |texture: &wgpu::Texture, positions: &[[u32; 3]]| {
            let buffer = device
                .create_buffer(
                    &wgpu::BufferDescriptor {
                        label: Some("Scene sync readback"),
                        size: positions.len() as u64 * 256,
                        usage: wgpu::BufferUsages::COPY_DST
                            | wgpu::BufferUsages::MAP_READ,
                        mapped_at_creation: false,
                    },
                );
            let mut encoder = device.create_command_encoder(&Default::default());
            for (i, &[x, y, z]) in positions.iter().enumerate() {
                encoder
                    .copy_texture_to_buffer(
                        wgpu::TexelCopyTextureInfo {
                            texture,
                            mip_level: 0,
                            origin: wgpu::Origin3d { x, y, z },
                            aspect: wgpu::TextureAspect::All,
                        },
                        wgpu::TexelCopyBufferInfo {
                            buffer: &buffer,
                            layout: wgpu::TexelCopyBufferLayout {
                                offset: i as u64 * 256,
                                bytes_per_row: Some(256),
                                rows_per_image: Some(1),
                            },
                        },
                        wgpu::Extent3d {
                            width: 1,
                            height: 1,
                            depth_or_array_layers: 1,
                        },
                    );
            }
            queue.submit([encoder.finish()]);
            buffer.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            let values = buffer
                .slice(..)
                .get_mapped_range()
                .unwrap()
                .as_chunks::<256>()
                .0
                .iter()
                .map(|row| u32::from_le_bytes(row[..4].try_into().unwrap()))
                .collect::<Vec<_>>();
            buffer.unmap();
            values
        };
        let stone_cell = [72, 64, 72];
        assert_eq!(
            read_cells(& tracer.voxel, & [stone_cell, [72, 65, 72], [0, 0, 0]]),
            [state("minecraft:stone").0, world.air.0, world.air.0],
            "bulk scene upload must preserve x/y/z layout and clear empty sections"
        );
        assert_eq!(read_cells(& tracer.occupancy, & [[4, 4, 4], [0, 0, 0]]), [1, 0]);
        assert_eq!(tracer.materials[state("minecraft:stone").0 as usize].range[1], 12);
        assert_eq!(tracer.light_count, 0);
        tracer.accumulation.samples = 11;
        tracer.configure(100, 4, true, 0);
        assert_eq!(
            tracer.accumulation.samples, 11, "unchanged quality preserves convergence"
        );
        tracer.configure(100, 4, true, 10);
        assert_eq!(
            tracer.accumulation.samples, 11, "display exposure preserves HDR history"
        );
        tracer.configure(50, 6, true, 10);
        assert_eq!(
            tracer.accumulation.samples, 0,
            "quality changes discard incompatible history"
        );
        tracer.accumulation.samples = 11;
        tracer.sync(&device, &queue, &world, camera, &assets, &render, &facts, &sprites);
        assert_eq!(
            tracer.accumulation.samples, 11,
            "unchanged chunk snapshots must preserve convergence"
        );
        let chunk = Arc::make_mut(world.chunks.get_mut(&(0, 0)).unwrap());
        let sy = chunk.section_index(0).unwrap();
        let section = Arc::make_mut(&mut chunk.sections[sy]);
        section.light = crate::chunk::Light::Uniform(0x20);
        section.biomes.set(0, 1);
        tracer.sync(&device, &queue, &world, camera, &assets, &render, &facts, &sprites);
        assert_eq!(
            tracer.accumulation.samples, 11,
            "light/biome-only snapshot updates must not discard traced lighting"
        );
        let leaves = state("minecraft:oak_leaves");
        world.set(10, 0, 8, leaves).unwrap();
        tracer.sync(&device, &queue, &world, camera, &assets, &render, &facts, &sprites);
        let triangles = tracer.triangles.len();
        let resets = tracer.accumulation.resets;
        for tick in 0..600 {
            let mut variant = assets.registry.state(leaves).unwrap().clone();
            variant.properties.insert("distance".into(), (tick % 6 + 1).to_string());
            let variant = assets.registry.id(&variant).unwrap();
            world.set(10, 0, 8, variant).unwrap();
            tracer
                .sync(
                    &device,
                    &queue,
                    &world,
                    camera,
                    &assets,
                    &render,
                    &facts,
                    &sprites,
                );
            assert_eq!(tracer.accumulation.samples, tick);
            tracer.accumulation.advance();
        }
        assert_eq!(tracer.accumulation.resets, resets);
        assert_eq!(
            tracer.triangles.len(), triangles,
            "simulation-only states must share exact baked triangles and history"
        );
        world.set(8, 0, 8, state("minecraft:glowstone")).unwrap();
        tracer.sync(&device, &queue, &world, camera, &assets, &render, &facts, &sprites);
        assert_eq!(
            tracer.accumulation.samples, 0,
            "a block edit must invalidate the old traced image"
        );
        assert_eq!(
            tracer.light_count, 1, "emissive blocks must enter the direct-light sampler"
        );
        assert_eq!(
            read_cells(& tracer.voxel, & [stone_cell]), [tracer
            .visual_states[state("minecraft:glowstone").0 as usize]],
            "incremental scene uploads must replace the same voxel as a bulk upload"
        );
        let anchor = RayCamera::new(camera, Mat4::IDENTITY, Mat4::IDENTITY);
        tracer.accumulation.camera = Some(anchor);
        tracer.accumulation.samples = 20;
        let resets = tracer.accumulation.resets;
        for tick in 100..700 {
            world.set(11, 0, 8, if tick % 2 == 0 { leaves } else { world.air }).unwrap();
            tracer.capture_scene(anchor, tick, false);
            assert!(tracer.hold_scene);
            tracer
                .sync(
                    &device,
                    &queue,
                    &world,
                    camera,
                    &assets,
                    &render,
                    &facts,
                    &sprites,
                );
            tracer.accumulation.advance();
        }
        assert_eq!(
            tracer.accumulation.resets, resets,
            "real background growth must not reopen a stationary shutter"
        );
        assert_eq!(read_cells(& tracer.voxel, & [[75, 64, 72]]), [world.air.0]);
        world.set(11, 0, 8, leaves).unwrap();
        tracer.capture_scene(anchor, 700, true);
        assert!(
            ! tracer.hold_scene, "block interaction must accept server scene updates"
        );
        tracer.sync(&device, &queue, &world, camera, &assets, &render, &facts, &sprites);
        assert_eq!(tracer.accumulation.samples, 0);
        assert_ne!(read_cells(& tracer.voxel, & [[75, 64, 72]]), [world.air.0]);
        tracer.accumulation.samples = 20;
        tracer.capture_scene(anchor, 719, false);
        assert!(! tracer.hold_scene, "late server acknowledgements remain accepted");
        tracer.capture_scene(anchor, 721, false);
        assert!(tracer.hold_scene);
        let mut moved = anchor;
        moved.eye.x += 0.01;
        tracer.capture_scene(moved, 722, false);
        assert!(! tracer.hold_scene);
        world.set(8, 0, 8, state("minecraft:stone_stairs")).unwrap();
        tracer.sync(&device, &queue, &world, camera, &assets, &render, &facts, &sprites);
        assert!(
            tracer.materials[state("minecraft:stone_stairs").0 as usize].range[1] > 12
        );
        assert_eq!(tracer.light_count, 0, "removed emitters must leave the sampler");
        tracer.material(state("minecraft:glass").0, &assets, &render, &facts);
        let glass = tracer.materials[state("minecraft:glass").0 as usize];
        assert_eq!(glass.surface[2], 1.);
        assert_eq!(glass.range[1], 12);
        let bush = state("minecraft:firefly_bush");
        tracer.material(bush.0, &assets, &render, &facts);
        let bush = tracer.materials[bush.0 as usize];
        let bush_faces = &tracer
            .triangles[bush.range[0] as usize..(bush.range[0] + bush.range[1]) as usize];
        assert!(
            bush_faces.iter().any(| tri | tri.e1[3] > 0.),
            "the model's glowing dots must emit"
        );
        assert!(
            bush_faces.iter().any(| tri | tri.e1[3] == 0.),
            "ordinary foliage must remain non-emissive"
        );
        let torch = state("minecraft:torch");
        tracer.material(torch.0, &assets, &render, &facts);
        let torch = tracer.materials[torch.0 as usize];
        assert!(
            tracer.triangles[torch.range[0] as usize.. (torch.range[0] + torch.range[1])
            as usize].iter().all(| tri | tri.uv2_layer[3] == 1.)
        );
        for triangle in &tracer
            .triangles[glass.range[0]
            as usize..(glass.range[0] + glass.range[1]) as usize]
        {
            let p = Vec3::from_slice(&triangle.p0);
            let e1 = Vec3::from_slice(&triangle.e1);
            let e2 = Vec3::from_slice(&triangle.e2);
            assert!(
                e1.cross(e2).dot(p + (e1 + e2) / 3. - Vec3::splat(0.5)) > 0.,
                "closed glass model winding must identify entering and exiting rays"
            );
        }
        world.chunks.remove(&(0, 0));
        tracer.accumulation.samples = 15;
        tracer.sync(&device, &queue, &world, camera, &assets, &render, &facts, &sprites);
        assert_eq!(
            tracer.accumulation.samples, 0,
            "unloading terrain must invalidate history too"
        );
        assert_eq!(read_cells(& tracer.voxel, & [stone_cell]), [world.air.0]);
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    }
    #[test]
    fn gpu_traces_world_geometry_accumulates_and_observes_edits() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
            .expect("Vulkan adapter for the renderer test");
        eprintln!("Path tracer GPU test: {:?}", adapter.get_info());
        let (device, queue) = pollster::block_on(
                adapter
                    .request_device(
                        &wgpu::DeviceDescriptor {
                            required_limits: adapter.limits(),
                            ..Default::default()
                        },
                    ),
            )
            .unwrap();
        let errors = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let sprite = device
            .create_texture(
                &wgpu::TextureDescriptor {
                    label: None,
                    size: wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 3,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::COPY_DST
                        | wgpu::TextureUsages::STORAGE_BINDING,
                    view_formats: &[wgpu::TextureFormat::Rgba8UnormSrgb],
                },
            );
        queue
            .write_texture(
                sprite.as_image_copy(),
                &[220, 180, 120, 255, 255, 255, 255, 0, 255, 255, 255, 255],
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4),
                    rows_per_image: Some(1),
                },
                sprite.size(),
            );
        let sprites = [sprite];
        let mut tracer = PathTracer::new(
                &device,
                &sprites,
                2048,
                wgpu::TextureFormat::Rgba8Unorm,
            )
            .unwrap();
        tracer.materials = vec![
            Material::zeroed(), Material { range : [0, 2, 0, 0], tint_emission : [1., 1.,
            1., 0.], surface : [0.85, 0., 0., 1.5], absorption : [0.; 4], },
        ];
        tracer.triangles = vec![
            Triangle { p0 : [0., 1., 0., 0.], e1 : [0., 0., 1., 0.], e2 : [1., 0., 1.,
            0.], uv0_uv1 : [0., 0., 0., 1.], uv2_layer : [1., 1., 0., 0.], }, Triangle {
            p0 : [0., 1., 0., 0.], e1 : [1., 0., 1., 0.], e2 : [1., 0., 0., 0.], uv0_uv1
            : [0., 0., 1., 1.], uv2_layer : [1., 0., 0., 0.], },
        ];
        tracer.material_buffer = storage(
            &device,
            "Test materials",
            bytemuck::cast_slice(&tracer.materials),
        );
        tracer.triangle_buffer = storage(
            &device,
            "Test triangles",
            bytemuck::cast_slice(&tracer.triangles),
        );
        let mut cells = vec![0u32; (SIDE * SIDE * SIDE) as usize];
        for z in 16..112 {
            for x in 16..112 {
                cells[((z * SIDE + 60) * SIDE + x) as usize] = 1;
            }
        }
        let upload = |tracer: &PathTracer, cells: &[u32]| {
            queue
                .write_texture(
                    tracer.voxel.as_image_copy(),
                    bytemuck::cast_slice(cells),
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(SIDE * 4),
                        rows_per_image: Some(SIDE),
                    },
                    tracer.voxel.size(),
                )
        };
        upload(&tracer, &cells);
        queue
            .write_texture(
                tracer.occupancy.as_image_copy(),
                bytemuck::cast_slice(&[1u32; 512]),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(BRICKS * 4),
                    rows_per_image: Some(BRICKS),
                },
                tracer.occupancy.size(),
            );
        tracer.bind_scene(&device, &sprites);
        let output = device
            .create_texture(
                &wgpu::TextureDescriptor {
                    label: None,
                    size: wgpu::Extent3d {
                        width: 32,
                        height: 32,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                },
            );
        let view = output.create_view(&Default::default());
        let matrix = glam::Mat4::perspective_infinite_reverse_rh(
            70f32.to_radians(),
            1.,
            0.05,
        ) * glam::Mat4::look_to_rh(Vec3::ZERO, Vec3::new(0., -0.4, -1.), Vec3::Y);
        let sun = Vec3::new(0.3, 1., 0.2).normalize();
        let mut uniform = Uniform {
            origin: [0; 4],
            inverse: matrix.inverse().to_cols_array_2d(),
            camera: [64.5, 64., 70.5, 0.],
            sun: [sun.x, sun.y, sun.z, 5.],
            sky: [0.08, 0.15, 0.3, 0.],
            image: [32, 32, 0, 0],
            options: [3, 0, 1, 0],
            optical: [1., 0.00465, EMISSION_POWER, 0.],
        };
        let draw = |tracer: &mut PathTracer, uniform: &Uniform| {
            let mut encoder = device.create_command_encoder(&Default::default());
            tracer.encode(&device, &queue, &mut encoder, &view, uniform);
            let buffer = device
                .create_buffer(
                    &wgpu::BufferDescriptor {
                        label: None,
                        size: 256 * 32,
                        usage: wgpu::BufferUsages::COPY_DST
                            | wgpu::BufferUsages::MAP_READ,
                        mapped_at_creation: false,
                    },
                );
            encoder
                .copy_texture_to_buffer(
                    output.as_image_copy(),
                    wgpu::TexelCopyBufferInfo {
                        buffer: &buffer,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(256),
                            rows_per_image: Some(32),
                        },
                    },
                    output.size(),
                );
            queue.submit([encoder.finish()]);
            buffer.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            let data = buffer.slice(..).get_mapped_range().unwrap().to_vec();
            buffer.unmap();
            data
        };
        let normals = draw(&mut tracer, &uniform);
        let center = 16 * 256 + 16 * 4;
        assert_eq!(
            & normals[center..center + 4], & [128, 255, 128, 255],
            "primary ray must hit the upward-facing model triangle"
        );
        uniform.options[2] = 5;
        let albedo = draw(&mut tracer, &uniform);
        let srgb_to_linear = |channel: u8| {
            let channel = f32::from(channel) / 255.;
            if channel <= 0.04045 {
                channel / 12.92
            } else {
                ((channel + 0.055) / 1.055).powf(2.4)
            }
        };
        let expected_albedo = [220u8, 180, 120]
            .map(|channel| (srgb_to_linear(channel) * 255.).round() as u8);
        for (actual, expected) in albedo[center..center + 3].iter().zip(expected_albedo)
        {
            assert!(
                actual.abs_diff(expected) <= 1,
                "base-colour texture must decode from sRGB to linear albedo: {:?}, expected {expected_albedo:?}",
                & albedo[center..center + 3]
            );
        }
        uniform.options[2] = 1;
        let original_triangles = tracer.triangles.clone();
        let original_inverse = uniform.inverse;
        uniform.inverse = (glam::Mat4::perspective_infinite_reverse_rh(
            70f32.to_radians(),
            1.,
            0.05,
        ) * glam::Mat4::look_to_rh(Vec3::ZERO, Vec3::NEG_Y, Vec3::Z))
            .inverse()
            .to_cols_array_2d();
        let set_pbr = |tracer: &mut PathTracer, flags: u32, channels: [u8; 4]| {
            tracer.pbr_buffer = storage(
                &device,
                "Test linear PBR",
                bytemuck::cast_slice(&[4u32, 1, flags, 0, u32::from_le_bytes(channels)]),
            );
            tracer.bind_scene(&device, &sprites);
        };
        let mut pbr_normals = Vec::new();
        for (transform, expected) in [
            (0, [200u8, 218, 180]),
            (1, [55, 218, 180]),
            (2, [200, 218, 75]),
            (3, [180, 218, 200]),
        ] {
            tracer.triangles.clone_from(&original_triangles);
            for tri in &mut tracer.triangles {
                let mut uv = [
                    [tri.uv0_uv1[0], tri.uv0_uv1[1]],
                    [tri.uv0_uv1[2], tri.uv0_uv1[3]],
                    [tri.uv2_layer[0], tri.uv2_layer[1]],
                ];
                for p in &mut uv {
                    match transform {
                        1 => p[0] = 1. - p[0],
                        2 => p[1] = 1. - p[1],
                        3 => p.swap(0, 1),
                        _ => {}
                    }
                }
                tri.uv0_uv1 = [uv[0][0], uv[0][1], uv[1][0], uv[1][1]];
                tri.uv2_layer[0..2].copy_from_slice(&uv[2]);
            }
            tracer.triangle_buffer = storage(
                &device,
                "Test UV handedness",
                bytemuck::cast_slice(&tracer.triangles),
            );
            set_pbr(&mut tracer, 3, [200, 180, 128, 10]);
            let image = draw(&mut tracer, &uniform);
            for channel in 0..3 {
                assert!(
                    (i16::from(image[center + channel]) - i16::from(expected[channel]))
                    .abs() <= 1, "PBR normal UV transform {transform}: {:?}", &
                    image[center..center + 3]
                );
            }
            pbr_normals.push(image[center..center + 3].to_vec());
        }
        tracer.triangles.clone_from(&original_triangles);
        tracer.triangle_buffer = storage(
            &device,
            "Test two-sided PBR",
            bytemuck::cast_slice(&tracer.triangles),
        );
        set_pbr(&mut tracer, 3, [200, 180, 128, 10]);
        uniform.camera[1] = 59.5;
        uniform.inverse = (glam::Mat4::perspective_infinite_reverse_rh(
            70f32.to_radians(),
            1.,
            0.05,
        ) * glam::Mat4::look_to_rh(Vec3::ZERO, Vec3::Y, Vec3::Z))
            .inverse()
            .to_cols_array_2d();
        let back = draw(&mut tracer, &uniform);
        assert_eq!(& back[center..center + 3], & [200, 37, 180], "back-face PBR normal");
        uniform.camera[1] = 64.;
        uniform.inverse = (glam::Mat4::perspective_infinite_reverse_rh(
            70f32.to_radians(),
            1.,
            0.05,
        ) * glam::Mat4::look_to_rh(Vec3::ZERO, Vec3::NEG_Y, Vec3::Z))
            .inverse()
            .to_cols_array_2d();
        uniform.options[2] = 4;
        for (channels, expected) in [
            ([128, 128, 128, 10], [127u8, 0, 10]),
            ([128, 128, 250, 230], [5, 255, 230]),
            ([128, 128, 0, 229], [255, 0, 229]),
        ] {
            set_pbr(&mut tracer, 3, channels);
            let image = draw(&mut tracer, &uniform);
            assert_eq!(& image[center..center + 3], & expected, "linear labPBR decode");
        }
        eprintln!(
            "labPBR GPU normal UV transforms: {pbr_normals:?}; smoothness/F0/metal decode passed"
        );
        tracer.triangles = original_triangles;
        tracer.triangle_buffer = storage(
            &device,
            "Restore test triangles",
            bytemuck::cast_slice(&tracer.triangles),
        );
        set_pbr(&mut tracer, 0, [0; 4]);
        uniform.inverse = original_inverse;
        uniform.options[2] = 0;
        uniform.image = [32, 32, 1, 0];
        let first = draw(&mut tracer, &uniform);
        uniform.image = [32, 32, 2, 1];
        let second = draw(&mut tracer, &uniform);
        assert!(
            first[center] > 100 && second[center] > 100,
            "diffuse lit surface must be nonblack"
        );
        assert_ne!(first, second, "stochastic samples must change and accumulate");
        let opaque = tracer.materials[1];
        tracer
            .materials
            .push(Material {
                tint_emission: [1., 1., 1., 8.],
                ..opaque
            });
        let rebind = |tracer: &mut PathTracer| {
            tracer.material_buffer = storage(
                &device,
                "Test materials",
                bytemuck::cast_slice(&tracer.materials),
            );
            tracer.triangle_buffer = storage(
                &device,
                "Test triangles",
                bytemuck::cast_slice(&tracer.triangles),
            );
            tracer.bind_scene(&device, &sprites);
        };
        let accumulate_count = |
            tracer: &mut PathTracer,
            uniform: &mut Uniform,
            start: u32,
            count: u32|
        {
            let mut data = Vec::new();
            for sample in 0..count {
                uniform.image = [32, 32, start + sample, sample];
                data = draw(tracer, uniform);
            }
            data
        };
        let accumulate = |tracer: &mut PathTracer, uniform: &mut Uniform, start: u32| {
            accumulate_count(tracer, uniform, start, 32)
        };
        let read_linear = |tracer: &PathTracer, uniform: &Uniform| {
            let texture = &tracer
                .targets
                .as_ref()
                .unwrap()
                .history[(uniform.image[2] % 2) as usize];
            let buffer = device
                .create_buffer(
                    &wgpu::BufferDescriptor {
                        label: Some("Linear transport regression readback"),
                        size: 512 * 32,
                        usage: wgpu::BufferUsages::COPY_DST
                            | wgpu::BufferUsages::MAP_READ,
                        mapped_at_creation: false,
                    },
                );
            let mut encoder = device.create_command_encoder(&Default::default());
            encoder
                .copy_texture_to_buffer(
                    texture.as_image_copy(),
                    wgpu::TexelCopyBufferInfo {
                        buffer: &buffer,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(512),
                            rows_per_image: Some(32),
                        },
                    },
                    texture.size(),
                );
            queue.submit([encoder.finish()]);
            buffer.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            let data = buffer.slice(..).get_mapped_range().unwrap();
            let values: &[f32] = bytemuck::cast_slice(&data);
            let mut mean = [0_f32; 3];
            for y in 12..20 {
                for x in 12..20 {
                    for channel in 0..3 {
                        mean[channel] += values[(y * 32 + x) * 4 + channel] / 64.;
                    }
                }
            }
            drop(data);
            buffer.unmap();
            mean
        };
        let mut direct_energy = Vec::new();
        for bounces in [1, 4, 8] {
            uniform.options = [bounces, 0, 0, 0];
            accumulate_count(&mut tracer, &mut uniform, 3600, 256);
            direct_energy.push(read_linear(&tracer, &uniform));
        }
        eprintln!("Direct sun/sky energy at 1/4/8 bounces: {direct_energy:?}");
        for energy in &direct_energy[1..] {
            for channel in 0..3 {
                assert!(
                    (energy[channel] / direct_energy[0] [channel] - 1.).abs() < 0.05,
                    "last-vertex MIS must preserve direct sun/sky energy across bounce limits"
                );
            }
        }
        cells.fill(0);
        for z in 16..112 {
            for x in 16..112 {
                cells[((z * SIDE + 60) * SIDE + x) as usize] = 1;
                cells[((z * SIDE + 58) * SIDE + x) as usize] = 2;
            }
        }
        upload(&tracer, &cells);
        uniform.sun[3] = 0.;
        uniform.sky = [0.; 4];
        uniform.options[0] = 4;
        tracer.materials[1].surface = [0., 0., 1., 1.5];
        tracer.materials[1].absorption = [0., 0., 0., 1.];
        rebind(&mut tracer);
        let through_pane = accumulate(&mut tracer, &mut uniform, 100);
        assert!(
            through_pane[center] > 200,
            "a thin pane must reveal the emissive surface behind it"
        );
        let pane_cells = cells.clone();
        for y in 61..64 {
            for z in 16..112 {
                for x in 16..112 {
                    cells[((z * SIDE + y) * SIDE + x) as usize] = 1;
                }
            }
        }
        upload(&tracer, &cells);
        uniform.options[0] = 1;
        let through_windows = accumulate(&mut tracer, &mut uniform, 900);
        eprintln!("Four thin windows, one diffuse bounce: {}", through_windows[center]);
        assert!(
            through_windows[center] > 180,
            "glass interfaces must not consume the diffuse bounce budget"
        );
        cells = pane_cells;
        upload(&tracer, &cells);
        tracer
            .triangles
            .extend([
                Triangle {
                    p0: [0., 0., 0., 0.],
                    e1: [1., 0., 1., 0.],
                    e2: [0., 0., 1., 0.],
                    ..tracer.triangles[0]
                },
                Triangle {
                    p0: [0., 0., 0., 0.],
                    e1: [1., 0., 0., 0.],
                    e2: [1., 0., 1., 0.],
                    ..tracer.triangles[1]
                },
            ]);
        tracer.materials[1].range[1] = 4;
        tracer.materials[1].absorption = [0.005, 0.005, 0.005, 0.];
        for z in 16..112 {
            for x in 16..112 {
                cells[((z * SIDE + 61) * SIDE + x) as usize] = 1;
            }
        }
        upload(&tracer, &cells);
        uniform.options[0] = 1;
        rebind(&mut tracer);
        let through_solid = accumulate(&mut tracer, &mut uniform, 140);
        assert!(
            through_solid[center] > 180,
            "adjacent glass must suppress internal faces and transmit through both external interfaces"
        );
        tracer.materials[1] = opaque;
        rebind(&mut tracer);
        let blocked = accumulate(&mut tracer, &mut uniform, 180);
        assert!(blocked[center] < 90, "an opaque surface must block the same emitter");
        cells.fill(0);
        for z in 16..112 {
            for x in 16..112 {
                cells[((z * SIDE + 60) * SIDE + x) as usize] = 1;
                cells[((z * SIDE + 66) * SIDE + x) as usize] = 2;
            }
        }
        upload(&tracer, &cells);
        tracer.materials[1].surface = [0.08, 1., 0., 1.5];
        uniform.options[0] = 3;
        rebind(&mut tracer);
        let reflected = accumulate(&mut tracer, &mut uniform, 220);
        assert!(
            reflected[center] > 180,
            "metal reflections must include secondary-hit emissive radiance"
        );
        eprintln!(
            "Material GPU readback (red): pane {}, solid glass {}, opaque {}, metal reflection {}",
            through_pane[center], through_solid[center], blocked[center],
            reflected[center]
        );
        cells.fill(0);
        for z in 16..112 {
            for x in 16..112 {
                cells[((z * SIDE + 60) * SIDE + x) as usize] = 1;
            }
        }
        let mut emitters = [
            [64_u32, 66, 62, 2],
            [65, 66, 62, 2],
            [64, 66, 63, 2],
            [65, 66, 63, 2],
        ];
        emitters.sort_unstable();
        for &[x, y, z, id] in &emitters {
            cells[((z * SIDE + y) * SIDE + x) as usize] = id;
        }
        upload(&tracer, &cells);
        tracer.light_buffer = storage(
            &device,
            "Test area emitters",
            bytemuck::cast_slice(
                &light_distribution(
                    &emitters,
                    &tracer.materials,
                    &tracer.triangles,
                    Vec3::from_slice(&uniform.camera),
                ),
            ),
        );
        tracer.materials[1] = opaque;
        uniform.options = [1, emitters.len() as u32, 0, 0];
        rebind(&mut tracer);
        let lit_floor = accumulate(&mut tracer, &mut uniform, 260);
        let forward_triangles = tracer.triangles.clone();
        for tri in &mut tracer.triangles[..2] {
            std::mem::swap(&mut tri.e1, &mut tri.e2);
            let uv1 = [tri.uv0_uv1[2], tri.uv0_uv1[3]];
            tri.uv0_uv1[2..4].copy_from_slice(&tri.uv2_layer[..2]);
            tri.uv2_layer[..2].copy_from_slice(&uv1);
        }
        rebind(&mut tracer);
        let reversed_floor = accumulate(&mut tracer, &mut uniform, 260);
        assert!(
            (i16::from(lit_floor[center]) - i16::from(reversed_floor[center])).abs() < 8,
            "two-sided emitter winding must preserve floor lighting: {}/{}",
            lit_floor[center], reversed_floor[center]
        );
        eprintln!(
            "Two-sided emitter winding GPU readback (red): {}/{}", lit_floor[center],
            reversed_floor[center]
        );
        tracer.triangles = forward_triangles;
        rebind(&mut tracer);
        tracer.materials[2].tint_emission[3] = 0.;
        rebind(&mut tracer);
        let unlit_floor = accumulate(&mut tracer, &mut uniform, 300);
        let unlit_energy = read_linear(&tracer, &uniform);
        eprintln!(
            "Diffuse area-light GPU readback (red): lit {}, unlit {}", lit_floor[center],
            unlit_floor[center]
        );
        assert!(
            lit_floor[center] > 80,
            "emitter sampling must illuminate nearby diffuse surfaces"
        );
        assert_eq!(
            unlit_floor[center], 0, "no fake light or display lift on unlit black"
        );
        tracer.materials[2].tint_emission[3] = 3.;
        rebind(&mut tracer);
        let old_floor = accumulate(&mut tracer, &mut uniform, 300);
        let old_energy = read_linear(&tracer, &uniform);
        tracer.materials[2].tint_emission[3] = emission_strength(15);
        rebind(&mut tracer);
        let new_floor = accumulate(&mut tracer, &mut uniform, 300);
        let new_energy = read_linear(&tracer, &uniform);
        for channel in 0..3 {
            let expected_ratio = emission_strength(15) / 3.;
            assert!(
                ((new_energy[channel] - unlit_energy[channel]) / (old_energy[channel] -
                unlit_energy[channel]) - expected_ratio).abs() < 0.001,
                "source power must scale scene-linear transport: {old_energy:?}/{new_energy:?}"
            );
        }
        assert!(new_floor[center] > old_floor[center] + 20);
        eprintln!(
            "Emitter power 3 -> {}: floor display {}/{}, linear {:?}/{:?}",
            emission_strength(15), old_floor[center], new_floor[center], old_energy,
            new_energy
        );
        let importance = light_distribution(
            &emitters,
            &tracer.materials,
            &tracer.triangles,
            Vec3::from_slice(&uniform.camera),
        );
        let mut estimates = Vec::new();
        for skewed in [false, true] {
            let mut proposal = importance.clone();
            let masses = if skewed { [0.7, 0.1, 0.1, 0.1] } else { [0.25; 4] };
            let mut cdf = 0.;
            for (light, mass) in proposal.iter_mut().zip(masses) {
                cdf += mass;
                light.sampling = [cdf, mass, 0., 0.];
            }
            proposal.last_mut().unwrap().sampling[0] = 1.;
            tracer.light_buffer = storage(
                &device,
                "Proposal energy regression",
                bytemuck::cast_slice(&proposal),
            );
            rebind(&mut tracer);
            uniform.options[0] = 4;
            accumulate_count(&mut tracer, &mut uniform, 2000, 256);
            estimates.push(read_linear(&tracer, &uniform));
        }
        assert!(
            (estimates[1] [0] / estimates[0] [0] - 1.).abs() < 0.1,
            "nonuniform selection and emitter-hit MIS must preserve energy: {estimates:?}"
        );
        eprintln!("Uniform/skewed light proposal linear energy: {estimates:?}");
        tracer.light_buffer = storage(
            &device,
            "Test area emitters",
            bytemuck::cast_slice(&importance),
        );
        uniform.options[0] = 1;
        let camera = uniform.camera;
        uniform.camera[1] = 63.5;
        tracer.materials[2].tint_emission[3] = 8.;
        tracer
            .materials
            .push(Material {
                surface: [0., 0., 1., 1.5],
                absorption: [0.005, 0.005, 0.005, 0.],
                range: [0, 4, 0, 0],
                ..opaque
            });
        rebind(&mut tracer);
        let open_floor = accumulate(&mut tracer, &mut uniform, 1000);
        for z in 16..112 {
            for x in 16..112 {
                cells[((z * SIDE + 65) * SIDE + x) as usize] = 3;
            }
        }
        upload(&tracer, &cells);
        let glass_floor = accumulate(&mut tracer, &mut uniform, 1000);
        accumulate_count(&mut tracer, &mut uniform, 2400, 256);
        let glass_direct_energy = read_linear(&tracer, &uniform);
        uniform.options[0] = 4;
        accumulate_count(&mut tracer, &mut uniform, 2400, 256);
        let glass_bounce_energy = read_linear(&tracer, &uniform);
        assert!(
            (glass_bounce_energy[0] / glass_direct_energy[0] - 1.).abs() < 0.15,
            "glass transmission must not count direct NEE and BSDF hits twice: {glass_direct_energy:?}/{glass_bounce_energy:?}"
        );
        eprintln!(
            "Glass direct-only / four-bounce linear energy: {glass_direct_energy:?}/{glass_bounce_energy:?}"
        );
        uniform.options[0] = 1;
        tracer.materials[3].absorption = [0.05, 2., 3., 0.];
        rebind(&mut tracer);
        let red_glass_floor = accumulate(&mut tracer, &mut uniform, 1000);
        tracer.materials[3] = opaque;
        rebind(&mut tracer);
        let blocked_floor = accumulate(&mut tracer, &mut uniform, 1000);
        eprintln!(
            "Glass shadow GPU readback: open {}, clear {}, red {:?}, opaque {}",
            open_floor[center], glass_floor[center], & red_glass_floor[center..center +
            3], blocked_floor[center]
        );
        assert!(glass_floor[center] > 60 && glass_floor[center] < open_floor[center]);
        assert!(
            red_glass_floor[center] > 50 && red_glass_floor[center + 1] <
            glass_floor[center + 1] / 2
        );
        assert_eq!(blocked_floor[center], 0, "opaque shadow must remain black");
        for z in 16..112 {
            for x in 16..112 {
                cells[((z * SIDE + 65) * SIDE + x) as usize] = 0;
            }
        }
        upload(&tracer, &cells);
        uniform.camera = camera;
        tracer.materials[1] = opaque;
        uniform.options = [1, 0, 0, 0];
        let mut ramp = Vec::new();
        for (i, emission) in [0., 0.01, 0.1, 0.5, 1., 3., 8., 32., 128.]
            .into_iter()
            .enumerate()
        {
            tracer.materials[1].tint_emission[3] = emission;
            rebind(&mut tracer);
            uniform.image = [32, 32, 300 + i as u32, 0];
            let pixels = draw(&mut tracer, &uniform);
            let rgb = [pixels[center], pixels[center + 1], pixels[center + 2]];
            ramp.push(rgb);
        }
        eprintln!("Display radiance ramp RGB: {ramp:?}");
        assert_eq!(ramp[0], [0; 3], "unlit black must remain black");
        assert!(
            ramp[1] [0] > 4 && ramp[1] [0] < 30,
            "the display toe must retain low-radiance detail"
        );
        assert!(
            ramp.windows(2).all(| w | { w[1] != w[0] && w[1].iter().zip(w[0]).all(| (&
            next, previous) | next >= previous) })
        );
        assert!(
            ramp[6] [0] < 253 && ramp[8] [0] < 255, "highlights must retain headroom"
        );
        assert!(
            ramp[6] [0] < 240 && ramp[8] [0] - ramp[6] [0] >= 8,
            "extended highlight shoulder must retain emitter texture headroom"
        );
        for (index, expected) in [[0, 0, 0], [8, 5, 2], [77, 61, 38]]
            .into_iter()
            .enumerate()
        {
            assert_eq!(ramp[index], expected, "M4 black/toe must remain unchanged");
        }
        assert!(
            ramp[5] [0] > ramp[5] [1] && ramp[5] [1] > ramp[5] [2],
            "bright warm emission must retain its hue"
        );
        let linear = |value: u8| {
            let v = f32::from(value) / 255.;
            if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        let reference = ramp[4].map(linear);
        for rgb in &ramp[5..] {
            let color = rgb.map(linear);
            for channel in 1..3 {
                assert!(
                    (color[channel] / color[0] - reference[channel] / reference[0]).abs()
                    < 0.015,
                    "increasing emitter radiance must preserve linear RGB ratios: {rgb:?}"
                );
            }
        }
        let mut exposure_ramp = Vec::new();
        tracer.materials[1].tint_emission[3] = 8.;
        rebind(&mut tracer);
        for exposure in [0.25, 1., 4., 16.] {
            uniform.optical[0] = exposure;
            uniform.image[3] = 0;
            let pixels = draw(&mut tracer, &uniform);
            exposure_ramp.push([pixels[center], pixels[center + 1], pixels[center + 2]]);
        }
        uniform.optical[0] = 1.;
        assert!(
            exposure_ramp.windows(2).all(| w | { w[1] != w[0] && w[1].iter().zip(w[0])
            .all(| (& next, previous) | next >= previous) }),
            "higher display exposure must retain distinct highlight levels"
        );
        for rgb in &exposure_ramp {
            let color = rgb.map(linear);
            assert!(rgb[0] < 255, "exposure must not hard-clip highlights");
            assert!(
                (color[1] / color[0] - reference[1] / reference[0]).abs() < 0.015 &&
                (color[2] / color[0] - reference[2] / reference[0]).abs() < 0.015,
                "display exposure must not bleach warm emission: {rgb:?}"
            );
        }
        eprintln!("Display exposure ramp RGB: {exposure_ramp:?}");
        tracer.materials[1] = Material {
            range: [0, 2, 1, 0],
            ..opaque
        };
        tracer.materials[2].range[2] = 1;
        tracer.materials[2].tint_emission[3] = 8.;
        let mut visible = Vec::new();
        let mut indirect = Vec::new();
        for (index, v) in [0.42, 0.75].into_iter().enumerate() {
            for tri in &mut tracer.triangles {
                tri.e1[3] = 8.;
                tri.uv0_uv1 = [0.5, v, 0.5, v];
                tri.uv2_layer = [0.5, v, 0., 1.];
            }
            uniform.options = [1, 0, 0, 0];
            rebind(&mut tracer);
            visible
                .push(
                    accumulate(
                        &mut tracer,
                        &mut uniform,
                        400 + index as u32 * 64,
                    )[center],
                );
            tracer.materials[1] = opaque;
            uniform.options = [1, emitters.len() as u32, 0, 0];
            rebind(&mut tracer);
            indirect
                .push(
                    accumulate(
                        &mut tracer,
                        &mut uniform,
                        432 + index as u32 * 64,
                    )[center],
                );
            tracer.materials[1].range[2] = 1;
        }
        eprintln!(
            "Torch emission GPU readback (head/wood): visible {visible:?}, sampled {indirect:?}"
        );
        assert!(
            visible[0] > 200 && visible[1] == 0,
            "only the torch head may glow in a camera ray"
        );
        assert!(
            indirect[0] > 80 && indirect[1] == 0,
            "direct lighting must use the identical emission mask"
        );
        let masked_triangles = tracer.triangles.clone();
        let masked_materials = [tracer.materials[1], tracer.materials[2]];
        let emitter_triangles = tracer.triangles[..2].to_vec();
        tracer.triangles.truncate(2);
        for tri in &mut tracer.triangles {
            tri.uv2_layer[2] = 2.;
        }
        tracer.triangles.extend(emitter_triangles);
        tracer.materials[2].range = [2, 2, 1, 0];
        let mut mapped = Vec::new();
        for value in [0_u32, 64, 254] {
            tracer.pbr_buffer = storage(
                &device,
                "Packed emission mask regression",
                bytemuck::cast_slice(
                    &[
                        12_u32,
                        1,
                        6 | (value << 8),
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        u32::from_le_bytes([128, 128, 0, 10]),
                        value,
                    ],
                ),
            );
            tracer.materials[1] = opaque;
            uniform.options = [1, emitters.len() as u32, 0, 0];
            rebind(&mut tracer);
            accumulate_count(&mut tracer, &mut uniform, 4200, 64);
            let indirect_energy = read_linear(&tracer, &uniform);
            tracer.materials[1].range = [2, 2, 1, 0];
            uniform.options[1] = 0;
            rebind(&mut tracer);
            accumulate(&mut tracer, &mut uniform, 4400);
            let visible_energy = read_linear(&tracer, &uniform);
            mapped.push((visible_energy, indirect_energy));
        }
        eprintln!("labPBR emission alpha 0/64/254 visible/sampled energy: {mapped:?}");
        assert_eq!(mapped[0], ([0.; 3], [0.; 3]));
        for channel in 0..3 {
            assert!(
                (mapped[2].0[channel] / mapped[1].0[channel] - 254. / 64.).abs() < 0.001
            );
            assert!(
                (mapped[2].1[channel] / mapped[1].1[channel] - 254. / 64.).abs() < 0.001
            );
        }
        tracer.triangles = masked_triangles;
        tracer.materials[1] = masked_materials[0];
        tracer.materials[2] = masked_materials[1];
        set_pbr(&mut tracer, 0, [0; 4]);
        let base = tracer.triangles[..2].to_vec();
        tracer.triangles = base
            .iter()
            .map(|tri| Triangle {
                e1: [tri.e1[0], tri.e1[1], tri.e1[2], 8.],
                uv2_layer: [0.5, 0.5, 2., 0.],
                ..*tri
            })
            .chain(
                base
                    .iter()
                    .map(|tri| Triangle {
                        e1: [tri.e1[0], tri.e1[1], tri.e1[2], 0.],
                        uv2_layer: [0.5, 0.5, 0., 0.],
                        ..*tri
                    }),
            )
            .collect();
        tracer.materials[1].range = [0, 4, 1, 0];
        uniform.options = [1, 0, 0, 0];
        rebind(&mut tracer);
        let dots = accumulate(&mut tracer, &mut uniform, 600)[center];
        for tri in &mut tracer.triangles[..2] {
            tri.uv2_layer[2] = 1.;
        }
        rebind(&mut tracer);
        let foliage = accumulate(&mut tracer, &mut uniform, 640)[center];
        eprintln!("Emissive overlay GPU readback (dot/foliage): {dots}/{foliage}");
        assert!(dots > 200 && foliage == 0);
        let faces = [
            [[0., 0., 0.], [0., 0., 1.], [0., 1., 1.], [0., 1., 0.]],
            [[1., 0., 1.], [1., 0., 0.], [1., 1., 0.], [1., 1., 1.]],
            [[0., 0., 1.], [0., 0., 0.], [1., 0., 0.], [1., 0., 1.]],
            [[0., 1., 0.], [0., 1., 1.], [1., 1., 1.], [1., 1., 0.]],
            [[1., 0., 0.], [0., 0., 0.], [0., 1., 0.], [1., 1., 0.]],
            [[0., 0., 1.], [1., 0., 1.], [1., 1., 1.], [0., 1., 1.]],
        ];
        tracer.triangles.clear();
        for face in faces {
            for indices in [[0, 1, 2], [0, 2, 3]] {
                let [a, b, c] = indices.map(|i| Vec3::from(face[i]));
                tracer
                    .triangles
                    .push(Triangle {
                        p0: [a.x, a.y, a.z, 0.],
                        e1: [(b - a).x, (b - a).y, (b - a).z, 0.],
                        e2: [(c - a).x, (c - a).y, (c - a).z, 0.],
                        uv0_uv1: [0.5; 4],
                        uv2_layer: [0.5, 0.5, 0., 0.],
                    });
            }
        }
        tracer.materials[1] = Material {
            range: [0, 12, 0, 0],
            surface: [0., 0., 1., 1.5],
            absorption: [0.005, 0.005, 0.005, 0.],
            ..opaque
        };
        tracer.materials[2] = Material {
            range: [0, 12, 0, 0],
            tint_emission: [1., 1., 1., 8.],
            ..opaque
        };
        rebind(&mut tracer);
        uniform.options = [1, 0, 0, 0];
        let mut cube_views = Vec::new();
        for normal in [Vec3::X, -Vec3::X, Vec3::Y, -Vec3::Y, Vec3::Z, -Vec3::Z] {
            let up = if normal.y.abs() > 0.9 { Vec3::Z } else { Vec3::Y };
            let right = normal.cross(up);
            cells.fill(0);
            cells[((64 * SIDE + 64) * SIDE + 64) as usize] = 1;
            for v in -2..=2 {
                for u in -2..=2 {
                    let cell = glam::IVec3::splat(64) - normal.as_ivec3() * 2
                        + right.as_ivec3() * u + up.as_ivec3() * v;
                    cells[((cell.z as u32 * SIDE + cell.y as u32) * SIDE + cell.x as u32)
                        as usize] = 2;
                }
            }
            upload(&tracer, &cells);
            for offset in [-0.45, 0., 0.45] {
                let direction = (normal + right * offset).normalize();
                let camera = Vec3::splat(64.5) + direction * 3.;
                uniform.camera = [camera.x, camera.y, camera.z, 0.];
                uniform.inverse = (glam::Mat4::perspective_infinite_reverse_rh(
                    35_f32.to_radians(),
                    1.,
                    0.05,
                ) * glam::Mat4::look_to_rh(Vec3::ZERO, -direction, up))
                    .inverse()
                    .to_cols_array_2d();
                uniform.options[2] = 2;
                uniform.image = [32, 32, 1200, 0];
                let depth = draw(&mut tracer, &uniform)[center];
                assert!(
                    (3..= 7).contains(& depth),
                    "the first hit must be the glass, not a missed face/backdrop: {normal:?}/{offset}/{depth}"
                );
                uniform.options[2] = 0;
                let pixels = accumulate(&mut tracer, &mut uniform, 1200);
                cube_views.push(pixels[center]);
                assert!(
                    pixels[center] > 180,
                    "glass must transmit from {normal:?}, oblique offset {offset}: {}",
                    pixels[center]
                );
            }
        }
        eprintln!("Glass cube six faces x three viewing angles: {cube_views:?}");
        let emitter_start = tracer.triangles.len() as u32;
        let emitter_faces: Vec<_> = tracer
            .triangles[..12]
            .iter()
            .enumerate()
            .map(|(i, tri)| Triangle {
                uv2_layer: [0.5, 0.5, if (6..8).contains(&i) { 0. } else { 2. }, 0.],
                ..*tri
            })
            .collect();
        tracer.triangles.extend(emitter_faces);
        tracer.materials[2].range = [emitter_start, 12, 0, 0];
        rebind(&mut tracer);
        uniform.camera = [63.3, 67., 64.5, 0.];
        uniform.inverse = (Mat4::perspective_infinite_reverse_rh(
            5_f32.to_radians(),
            1.,
            0.05,
        ) * Mat4::look_to_rh(Vec3::ZERO, Vec3::new(0.6, -1., 0.), Vec3::Z))
            .inverse()
            .to_cols_array_2d();
        let mut contact_energy = Vec::new();
        for lamp_y in [62, 63] {
            cells.fill(0);
            for z in 62..67 {
                for x in 62..68 {
                    cells[((z * SIDE + 64) * SIDE + x) as usize] = 1;
                    cells[((z * SIDE + lamp_y) * SIDE + x) as usize] = 2;
                }
            }
            upload(&tracer, &cells);
            accumulate(&mut tracer, &mut uniform, 1600);
            let energy = read_linear(&tracer, &uniform);
            contact_energy.push(energy);
            assert!(
                energy[0] > energy[2] * 2.5,
                "glass/lamp gap {} must show the copper top, not the white far shell: {energy:?}",
                63 - lamp_y
            );
        }
        eprintln!("Glass/lamp near-face regression (gap/contact): {contact_energy:?}");
        let white_start = tracer.triangles.len() as u32;
        let white_faces: Vec<_> = tracer
            .triangles[emitter_start as usize..]
            .iter()
            .map(|tri| Triangle {
                uv2_layer: [0.5, 0.5, 2., 0.],
                ..*tri
            })
            .collect();
        tracer.triangles.extend(white_faces);
        tracer.materials[3] = Material {
            range: [white_start, 12, 0, 0],
            ..tracer.materials[2]
        };
        rebind(&mut tracer);
        let mut stripe_energy = Vec::new();
        for lamp_y in [63, 62] {
            cells.fill(0);
            for z in 62..67 {
                for x in 62..68 {
                    cells[((z * SIDE + 64) * SIDE + x) as usize] = 1;
                    cells[((z * SIDE + lamp_y) * SIDE + x) as usize] = if x < 65 {
                        2
                    } else {
                        3
                    };
                }
            }
            upload(&tracer, &cells);
            accumulate(&mut tracer, &mut uniform, 1600);
            stripe_energy.push(read_linear(&tracer, &uniform));
        }
        eprintln!("Refracted lamp stripes (touching/gap): {stripe_energy:?}");
        assert!(stripe_energy[0] [0] > stripe_energy[0] [2] * 2.5);
        assert!((stripe_energy[1] [0] / stripe_energy[1] [2] - 1.).abs() < 0.01);
        let layered_glass = Material {
            range: [0, 12, 0, 0],
            surface: [0., 0., 1., 1.5],
            absorption: [0.05, 1.2, 0.35, 0.],
            ..opaque
        };
        tracer.materials.push(layered_glass);
        rebind(&mut tracer);
        let normal = Vec3::new(0.62, -0.16, 0.77).normalize();
        let camera = Vec3::splat(64.5) + normal * 4.;
        uniform.camera = [camera.x, camera.y, camera.z, 0.];
        uniform.inverse = (Mat4::perspective_infinite_reverse_rh(
            7_f32.to_radians(),
            1.,
            0.05,
        ) * Mat4::look_to_rh(Vec3::ZERO, -normal, Vec3::Y))
            .inverse()
            .to_cols_array_2d();
        uniform.options = [4, 0, 0, 0];
        let mut layered_scene = |inner_state: u32| {
            cells.fill(0);
            for z in 60..69 {
                for y in 60..69 {
                    cells[((z * SIDE + y) * SIDE + 64) as usize] = 1;
                    cells[((z * SIDE + y) * SIDE + 63) as usize] = inner_state;
                    cells[((z * SIDE + y) * SIDE + 62) as usize] = 2;
                }
            }
            upload(&tracer, &cells);
            accumulate(&mut tracer, &mut uniform, 2100);
            read_linear(&tracer, &uniform)
        };
        let clear_layers = layered_scene(1);
        let tinted_layers = layered_scene(4);
        eprintln!(
            "Layered oblique glass transport: clear {clear_layers:?}, tinted {tinted_layers:?}"
        );
        assert!(
            clear_layers[0] > 0.01 && clear_layers[1] > 0.005,
            "oblique rays must retain the emitter behind two glass layers: {clear_layers:?}"
        );
        assert!(
            tinted_layers[0] > tinted_layers[1] * 1.5 && tinted_layers[0] >
            clear_layers[0] * 0.05,
            "adjacent stained glass must tint, not erase, the lower layer: {tinted_layers:?}/{clear_layers:?}"
        );
        uniform.options = [4, 0, 0, 0];
        uniform.optical[3] = 1.;
        accumulate(&mut tracer, &mut uniform, 3000);
        let spectral_layers = read_linear(&tracer, &uniform);
        eprintln!("Hero-wavelength layered glass transport: {spectral_layers:?}");
        assert!(
            spectral_layers.iter().all(| value | value.is_finite()) && spectral_layers
            .iter().any(| value | * value > 0.001),
            "opt-in spectral transport must produce finite radiance: {spectral_layers:?}"
        );
        uniform.optical[3] = 0.;
        queue
            .write_texture(
                tracer.voxel.as_image_copy(),
                bytemuck::cast_slice(&vec![0u32; cells.len()]),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(SIDE * 4),
                    rows_per_image: Some(SIDE),
                },
                tracer.voxel.size(),
            );
        uniform.image = [32, 32, 3, 0];
        uniform.options[2] = 1;
        let empty = draw(&mut tracer, &uniform);
        assert_eq!(
            & empty[center..center + 4], & [128, 128, 128, 255],
            "edited geometry must disappear"
        );
        assert!(
            pollster::block_on(errors.pop()).is_none(), "GPU validation must succeed"
        );
    }
}
trait NativePtChunkSections {
    fn section_index(&self, y: i32) -> Option<usize>;
}
impl NativePtChunkSections for crate::chunk::Chunk {
    fn section_index(&self, y: i32) -> Option<usize> {
        usize::try_from((i64::from(y) - i64::from(crate::chunk::MIN_Y)).div_euclid(16))
            .ok()
            .filter(|&index| index < self.sections.len())
    }
}
#[cfg(test)]
mod native_chunk_layout_tests {
    use super::NativePtChunkSections;
    #[test]
    fn native_pt_fixed_column_index_respects_negative_and_upper_bounds() {
        let chunk = crate::chunk::Chunk::empty(0, 0);
        assert_eq!(chunk.section_index(i32::MIN), None);
        assert_eq!(chunk.section_index(crate ::chunk::MIN_Y - 1), None);
        assert_eq!(chunk.section_index(crate ::chunk::MIN_Y), Some(0));
        assert_eq!(chunk.section_index(0), Some(4));
        let end = crate::chunk::MIN_Y + chunk.sections.len() as i32 * 16;
        assert_eq!(chunk.section_index(end - 1), Some(chunk.sections.len() - 1));
        assert_eq!(chunk.section_index(end), None);
        assert_eq!(chunk.section_index(i32::MAX), None);
    }
}
pub(super) struct Capture {
    pub(super) beauty: wgpu::Buffer,
    pub(super) albedo: wgpu::Buffer,
    pub(super) normal: wgpu::Buffer,
    pub(super) size: [u32; 2],
    pub(super) exposure: f32,
}
struct NativeCapturePipeline {
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
}
impl NativeCapturePipeline {
    fn new(
        device: &wgpu::Device,
        module: &wgpu::ShaderModule,
        uniform: &wgpu::BindGroupLayout,
        scene: &wgpu::BindGroupLayout,
    ) -> Self {
        let output = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format: wgpu::TextureFormat::Rgba32Float,
                view_dimension: wgpu::TextureViewDimension::D2,
            },
            count: None,
        };
        let layout = device
            .create_bind_group_layout(
                &wgpu::BindGroupLayoutDescriptor {
                    label: Some("Offline PT Lite capture guides"),
                    entries: &[output(3), output(4)],
                },
            );
        let pipeline_layout = device
            .create_pipeline_layout(
                &wgpu::PipelineLayoutDescriptor {
                    label: Some("Offline PT Lite capture guides"),
                    bind_group_layouts: &[Some(uniform), Some(scene), Some(&layout)],
                    ..Default::default()
                },
            );
        let pipeline = device
            .create_compute_pipeline(
                &wgpu::ComputePipelineDescriptor {
                    label: Some("Offline PT Lite capture guides"),
                    layout: Some(&pipeline_layout),
                    module,
                    entry_point: Some("native_capture_guides"),
                    compilation_options: Default::default(),
                    cache: None,
                },
            );
        Self { layout, pipeline }
    }
}
fn native_capture_texture(
    device: &wgpu::Device,
    label: &str,
    size: [u32; 3],
    format: wgpu::TextureFormat,
    volume: bool,
) -> wgpu::Texture {
    assert!(! volume);
    device
        .create_texture(
            &wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: size[0],
                    height: size[1],
                    depth_or_array_layers: size[2],
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::STORAGE_BINDING
                    | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            },
        )
}
fn native_capture_copy(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    texture: &wgpu::Texture,
    size: [u32; 2],
    row: u32,
) -> wgpu::Buffer {
    let buffer = device
        .create_buffer(
            &wgpu::BufferDescriptor {
                label: Some("Offline PT Lite linear screenshot"),
                size: u64::from(row) * u64::from(size[1]),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            },
        );
    encoder
        .copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
        );
    buffer
}
impl PathTracer {
    pub(super) fn capture(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Option<Capture> {
        let uniform = self.native_capture_uniform.as_ref()?;
        let targets = self.targets.as_ref()?;
        let scene = self.scene_group.as_ref()?;
        let size = [uniform.image[0], uniform.image[1]];
        if self.accumulation.samples == 0 || uniform.options[2] != 0 || size.contains(&0)
            || targets.size != size
        {
            return None;
        }
        let row = size[0]
            .checked_mul(16)?
            .checked_add(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT - 1)?
            / wgpu::COPY_BYTES_PER_ROW_ALIGNMENT * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        if u64::from(row) * u64::from(size[1]) > device.limits().max_buffer_size {
            return None;
        }
        let make = |label| native_capture_texture(
            device,
            label,
            [size[0], size[1], 1],
            wgpu::TextureFormat::Rgba32Float,
            false,
        );
        let albedo = make("Offline PT Lite capture albedo");
        let normal = make("Offline PT Lite capture normal");
        let albedo_view = albedo.create_view(&Default::default());
        let normal_view = normal.create_view(&Default::default());
        let outputs = device
            .create_bind_group(
                &wgpu::BindGroupDescriptor {
                    label: Some("Offline PT Lite capture guides"),
                    layout: &self.native_capture.layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::TextureView(&albedo_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 4,
                            resource: wgpu::BindingResource::TextureView(&normal_view),
                        },
                    ],
                },
            );
        {
            let mut pass = encoder
                .begin_compute_pass(
                    &wgpu::ComputePassDescriptor {
                        label: Some("Offline PT Lite capture guides"),
                        timestamp_writes: None,
                    },
                );
            pass.set_pipeline(&self.native_capture.pipeline);
            pass.set_bind_group(0, &self.uniform_group, &[]);
            pass.set_bind_group(1, scene, &[]);
            pass.set_bind_group(2, &outputs, &[]);
            pass.dispatch_workgroups(size[0].div_ceil(8), size[1].div_ceil(8), 1);
        }
        let history = &targets.history[(uniform.image[2] % 2) as usize];
        Some(Capture {
            beauty: native_capture_copy(device, encoder, history, size, row),
            albedo: native_capture_copy(device, encoder, &albedo, size, row),
            normal: native_capture_copy(device, encoder, &normal, size, row),
            size,
            exposure: uniform.optical[0],
        })
    }
}
