use super::*;
use std::{collections::HashMap, sync::{Mutex, OnceLock}};
#[derive(Default)]
struct State {
    tracer: Option<path_tracer::PathTracer>,
    enabled: bool,
    failed: bool,
    pending: Option<PendingCapture>,
    sources: Vec<wgpu::Texture>,
    sprites: Vec<wgpu::Texture>,
}
fn states() -> &'static Mutex<HashMap<wgpu::Device, State>> {
    static STATES: OnceLock<Mutex<HashMap<wgpu::Device, State>>> = OnceLock::new();
    STATES.get_or_init(|| Mutex::new(HashMap::new()))
}
fn with_state<T>(
    renderer: &mut Renderer,
    action: impl FnOnce(&mut Renderer, &mut State) -> T,
) -> T {
    let key = renderer.device.clone();
    let mut state = states()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&key)
        .unwrap_or_default();
    let result = action(renderer, &mut state);
    states()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(key, state);
    result
}
impl State {
    fn prepare_atlas(&mut self, renderer: &Renderer) {
        if self.sources == renderer.sprite_textures {
            return;
        }
        self.sources = renderer.sprite_textures.clone();
        self.tracer = None;
        self.failed = false;
        self.sprites = self
            .sources
            .iter()
            .map(|source| {
                renderer
                    .device
                    .create_texture(
                        &wgpu::TextureDescriptor {
                            label: Some("Offline PT Lite sRGB sprites"),
                            size: source.size(),
                            mip_level_count: source.mip_level_count(),
                            sample_count: source.sample_count(),
                            dimension: source.dimension(),
                            format: source.format().add_srgb_suffix(),
                            usage: wgpu::TextureUsages::TEXTURE_BINDING
                                | wgpu::TextureUsages::COPY_DST,
                            view_formats: &[],
                        },
                    )
            })
            .collect();
        let mut encoder = renderer
            .device
            .create_command_encoder(
                &wgpu::CommandEncoderDescriptor {
                    label: Some("Offline PT Lite initial sprite copy"),
                },
            );
        for (source, target) in self.sources.iter().zip(&self.sprites) {
            for level in 0..source.mip_level_count() {
                encoder
                    .copy_texture_to_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: source,
                            mip_level: level,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        wgpu::TexelCopyTextureInfo {
                            texture: target,
                            mip_level: level,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        wgpu::Extent3d {
                            width: (source.width() >> level).max(1),
                            height: (source.height() >> level).max(1),
                            depth_or_array_layers: source.depth_or_array_layers(),
                        },
                    );
            }
        }
        renderer.queue.submit([encoder.finish()]);
    }
    fn copy_animated(&self, renderer: &Renderer, encoder: &mut wgpu::CommandEncoder) {
        let split = renderer.terrain_sprites[1] as u32;
        for &layer in renderer.animations.changed() {
            let array = (layer / split) as usize;
            let (Some(source), Some(target)) = (
                self.sources.get(array),
                self.sprites.get(array),
            ) else {
                continue;
            };
            for level in 0..source.mip_level_count() {
                let origin = wgpu::Origin3d {
                    x: 0,
                    y: 0,
                    z: layer % split,
                };
                encoder
                    .copy_texture_to_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: source,
                            mip_level: level,
                            origin,
                            aspect: wgpu::TextureAspect::All,
                        },
                        wgpu::TexelCopyTextureInfo {
                            texture: target,
                            mip_level: level,
                            origin,
                            aspect: wgpu::TextureAspect::All,
                        },
                        wgpu::Extent3d {
                            width: (source.width() >> level).max(1),
                            height: (source.height() >> level).max(1),
                            depth_or_array_layers: 1,
                        },
                    );
            }
        }
    }
}
impl Drop for Renderer {
    fn drop(&mut self) {
        states()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.device);
    }
}
impl Renderer {
    pub(super) fn native_pt_draw(&mut self, frame: &Frame) -> Result<bool> {
        with_state(
            self,
            |renderer, state| {
                state.poll_capture(renderer);
                renderer.native_pt_draw_inner(frame, state)
            },
        )
    }
    pub fn sync_path_tracer(
        &mut self,
        graphics: &crate::native_pt_config::PtOptions,
        world_ready: bool,
        world: &crate::world::World,
        frame: &Frame,
        interacting: bool,
        assets: &crate::assets::Assets,
        render: &Arc<crate::assets::RenderAssets>,
        facts: &horizon_items::blocks::BlockFacts,
    ) {
        with_state(
            self,
            |renderer, state| {
                if graphics.enabled && world_ready {
                    state.prepare_atlas(renderer);
                }
                renderer
                    .native_pt_sync_inner(
                        graphics,
                        world_ready,
                        world,
                        frame,
                        interacting,
                        assets,
                        render,
                        facts,
                        state,
                    );
            },
        );
    }
}
use std::sync::mpsc;
type CaptureResult = std::result::Result<[u32; 2], String>;
struct PendingCapture {
    request: ScreenshotRequest,
    target_size: [u32; 2],
    capture: Option<path_tracer::Capture>,
    mapped: mpsc::Receiver<(u8, std::result::Result<(), String>)>,
    ready: u8,
    worker: Option<mpsc::Receiver<CaptureResult>>,
}
impl State {
    fn begin_capture(
        &mut self,
        renderer: &mut Renderer,
        request: ScreenshotRequest,
        capture: path_tracer::Capture,
    ) {
        let (sender, mapped) = mpsc::channel();
        for (index, buffer) in [&capture.beauty, &capture.albedo, &capture.normal]
            .into_iter()
            .enumerate()
        {
            let sender = sender.clone();
            buffer
                .slice(..)
                .map_async(
                    wgpu::MapMode::Read,
                    move |result| {
                        let _ = sender
                            .send((
                                index as u8,
                                result.map_err(|error| error.to_string()),
                            ));
                    },
                );
        }
        self.pending = Some(PendingCapture {
            request,
            target_size: [renderer.config.width, renderer.config.height],
            capture: Some(capture),
            mapped,
            ready: 0,
            worker: None,
        });
    }
    fn poll_capture(&mut self, renderer: &mut Renderer) {
        let Some(mut pending) = self.pending.take() else {
            return;
        };
        if let Some(worker) = pending.worker.take() {
            match worker.try_recv() {
                Ok(result) if renderer.screenshot_result.is_none() => {
                    renderer.screenshot_result = Some(result);
                    return;
                }
                Ok(result) => {
                    let (sender, receiver) = mpsc::channel();
                    let _ = sender.send(result);
                    pending.worker = Some(receiver);
                }
                Err(mpsc::TryRecvError::Empty) => pending.worker = Some(worker),
                Err(mpsc::TryRecvError::Disconnected) => {
                    renderer.screenshot_result = Some(
                        Err("PT screenshot worker stopped unexpectedly".into()),
                    );
                    return;
                }
            }
        } else {
            if let Err(error) = renderer.device.poll(wgpu::PollType::Poll) {
                renderer.screenshot_result = Some(Err(error.to_string()));
                return;
            }
            while let Ok((index, result)) = pending.mapped.try_recv() {
                if let Err(error) = result {
                    renderer.screenshot_result = Some(Err(error));
                    return;
                }
                pending.ready |= 1 << index;
            }
            if pending.ready == 0b111 {
                let Some(capture) = pending.capture.take() else {
                    renderer.screenshot_result = Some(
                        Err("PT screenshot capture was lost".into()),
                    );
                    return;
                };
                let path = pending.request.path.clone();
                let target_size = pending.target_size;
                let (sender, receiver) = mpsc::channel();
                match std::thread::Builder::new()
                    .name("offline-pt-lite-screenshot".into())
                    .spawn(move || {
                        let result = capture_pixels(capture, path, target_size)
                            .and_then(native_pt_screenshot::write);
                        let _ = sender.send(result);
                    })
                {
                    Ok(_) => pending.worker = Some(receiver),
                    Err(error) => {
                        renderer.screenshot_result = Some(
                            Err(format!("starting PT screenshot worker failed: {error}")),
                        );
                        return;
                    }
                }
            }
        }
        self.pending = Some(pending);
    }
}
impl Renderer {
    pub fn native_pt_capture_busy(&self) -> bool {
        states()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&self.device)
            .is_some_and(|state| state.pending.is_some())
    }
}
fn decode_float_rows(
    data: &[u8],
    [width,
    height,
    ]: [u32; 2],
) -> std::result::Result<Vec<[f32; 3]>, String> {
    let row = (width as usize)
        .checked_mul(16)
        .ok_or("PT screenshot dimensions overflow")?;
    let stride = row
        .checked_add(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize - 1)
        .map(|size| {
            size / wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize
                * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize
        })
        .ok_or("PT screenshot stride overflow")?;
    let length = stride
        .checked_mul(height as usize)
        .filter(|&size| size > 0 && width > 0)
        .ok_or("invalid PT screenshot dimensions")?;
    if data.len() != length {
        return Err("PT screenshot float readback has the wrong length".into());
    }
    let mut output = Vec::with_capacity(width as usize * height as usize);
    for bytes in data.chunks_exact(stride) {
        for pixel in bytes[..row].chunks_exact(16) {
            output
                .push(
                    std::array::from_fn(|channel| {
                        let offset = channel * 4;
                        f32::from_le_bytes(pixel[offset..offset + 4].try_into().unwrap())
                    }),
                );
        }
    }
    Ok(output)
}
fn capture_pixels(
    capture: path_tracer::Capture,
    path: PathBuf,
    target_size: [u32; 2],
) -> std::result::Result<native_pt_screenshot::Pixels, String> {
    let read = |buffer: &wgpu::Buffer| {
        let data = buffer
            .get_mapped_range(..)
            .map_err(|error| format!("mapping PT screenshot failed: {error}"))?;
        decode_float_rows(&data, capture.size)
    };
    let result = (|| {
        Ok(native_pt_screenshot::Pixels {
            path,
            source_size: capture.size,
            target_size,
            exposure: capture.exposure,
            beauty: read(&capture.beauty)?,
            albedo: read(&capture.albedo)?,
            normal: read(&capture.normal)?,
        })
    })();
    capture.beauty.unmap();
    capture.albedo.unmap();
    capture.normal.unmap();
    result
}
#[cfg(test)]
mod capture_tests {
    use super::decode_float_rows;
    #[test]
    fn native_pt_float_capture_ignores_row_padding_and_preserves_hdr_and_signed_normals() {
        let mut bytes = vec![0xcc; 512];
        for (row, values) in [[1000.0_f32, 0.25, -0.75, 1.0], [0.01, -1.0, 0.5, 1.0]]
            .into_iter()
            .enumerate()
        {
            for (channel, value) in values.into_iter().enumerate() {
                let at = row * 256 + channel * 4;
                bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
            }
        }
        assert_eq!(
            decode_float_rows(& bytes, [1, 2]).unwrap(), [[1000.0, 0.25, - 0.75], [0.01,
            - 1.0, 0.5]]
        );
    }
    #[test]
    fn native_pt_float_capture_rejects_truncated_and_empty_buffers() {
        assert!(decode_float_rows(& [0; 255], [1, 1]).is_err());
        assert!(decode_float_rows(& [], [0, 1]).is_err());
        assert!(decode_float_rows(& [], [1, 0]).is_err());
    }
}
impl Renderer {
    fn native_pt_draw_inner(
        &mut self,
        frame: &Frame,
        state: &mut State,
    ) -> Result<bool> {
        let path_frame = !frame.title && state.enabled && state.tracer.is_some();
        self.arena.trim();
        self.sorted_indices.trim();
        if let Some(flat) = &mut self.flat {
            flat.trim(|page| self.arena.has_page(page));
        }
        if self.suspended {
            return Ok(false);
        }
        let started = Instant::now();
        if self.first_frame == FirstFrame::Acquire {
            startup_step(format_args!("acquiring the first frame"));
            self.first_frame = FirstFrame::Submit;
        }
        let acquire = Instant::now();
        let acquired = self.surface.as_ref().map(|s| s.get_current_texture());
        self.last_wait = acquire.elapsed();
        let (surface_frame, reconfigure) = match acquired {
            None => (None, false),
            Some(wgpu::CurrentSurfaceTexture::Success(f)) => (Some(f), false),
            Some(wgpu::CurrentSurfaceTexture::Suboptimal(f)) => (Some(f), true),
            Some(
                wgpu::CurrentSurfaceTexture::Timeout
                | wgpu::CurrentSurfaceTexture::Occluded,
            ) => {
                return Ok(false);
            }
            Some(wgpu::CurrentSurfaceTexture::Outdated) => {
                if let Some(surface) = &self.surface {
                    Self::configure(
                        &self.device,
                        surface,
                        &self.surface_config(),
                        self.hdr.as_ref(),
                    );
                }
                return Ok(false);
            }
            Some(wgpu::CurrentSurfaceTexture::Lost) => {
                self.recreate_surface()?;
                return Ok(false);
            }
            Some(wgpu::CurrentSurfaceTexture::Validation) => {
                if self.acquire_validation_failures == 0 {
                    if let Some(surface) = &self.surface {
                        Self::configure(
                            &self.device,
                            surface,
                            &self.surface_config(),
                            self.hdr.as_ref(),
                        );
                    }
                } else {
                    self.recreate_surface()?;
                }
                self.acquire_validation_failures = self
                    .acquire_validation_failures
                    .saturating_add(1);
                return Ok(false);
            }
        };
        if surface_frame.is_some() {
            self.acquire_validation_failures = 0;
        }
        let size = [self.config.width, self.config.height];
        let target_texture = match (&surface_frame, &mut self.hdr) {
            (Some(_), Some(hdr)) => hdr.frame(&self.device, size),
            (Some(f), None) => {
                self.readback
                    .target(&self.device, &f.texture, self.screenshot.is_some())
                    .unwrap_or_else(|| f.texture.clone())
            }
            (None, _) => self.offscreen.clone().ok_or("no offscreen target")?,
        };
        let section_copies = self.section_copies.take();
        let has_section_copies = section_copies.is_some();
        let mut encoder = section_copies
            .unwrap_or_else(|| {
                self.device
                    .create_command_encoder(
                        &wgpu::CommandEncoderDescriptor {
                            label: Some("Horizon frame"),
                        },
                    )
            });
        if self.animated_textures {
            let tick = if path_frame {
                state.tracer.as_mut().unwrap().animation_tick(frame, size)
            } else {
                frame.ticks
            };
            self.animate(&mut encoder, tick);
            if !state.sprites.is_empty() {
                state.copy_animated(self, &mut encoder);
            }
            self.hud.animate_sprites(&mut self.uploads, &mut encoder, frame.ticks);
        }
        self.update_lightmap(
            &mut encoder,
            &frame.sky,
            frame.darkness.1,
            (self.gamma - frame.darkness.0).max(0.0),
            frame.night_vision,
            frame.block_flicker,
            frame.boss_darken,
        );
        let views = self.views(frame);
        let pose = frame.bob * frame.spin;
        let view_proj = views.primary().view_proj(pose);
        let taa = !path_frame && !frame.title
            && self.pack_post.as_ref().is_none_or(|p| p.1.is_none())
            && self.native.as_ref().is_none_or(|n| n.post.is_none());
        let jittered = self
            .world_post
            .jitter(
                [self.config.width, self.config.height],
                taa,
                view_proj,
                frame.camera,
            ) * view_proj;
        if frame.title {
            self.panorama
                .prepare(
                    &self.queue,
                    self.config.width,
                    self.config.height,
                    frame.yaw,
                    frame.pitch,
                );
        }
        let view = views.primary().view;
        let sun_angle = frame.sky.sun_angle.to_radians();
        let sun = Vec3::new(0.0, sun_angle.cos(), -sun_angle.sin());
        let (fog_color, fog) = if let Some(f) = frame.fog {
            (
                f.color,
                [
                    f.environmental_start,
                    f.environmental_end,
                    f.render_distance_start,
                    f.render_distance_end,
                ],
            )
        } else if frame.underwater {
            (
                [frame.water_fog[0], frame.water_fog[1], frame.water_fog[2], 1.0],
                [-8.0, 96.0, -8.0, 96.0],
            )
        } else {
            let d = frame.distance;
            (
                [
                    frame.sky.fog_color[0],
                    frame.sky.fog_color[1],
                    frame.sky.fog_color[2],
                    1.0,
                ],
                {
                    let (start, end) = frame
                        .environment_fog
                        .unwrap_or((f32::MAX, f32::MAX));
                    [start, end, (d - d.min(64.0) * 0.25).max(0.0), d]
                },
            )
        };
        let globals = Globals {
            view_proj: jittered.to_cols_array_2d(),
            fog_color,
            fog,
            sky_color: [
                frame.sky.sky_color[0],
                frame.sky.sky_color[1],
                frame.sky.sky_color[2],
                1.0,
            ],
            misc: [
                1.0 - frame.sky.sky_light_factor,
                frame.game_time,
                views.primary().aspect,
                views.primary().tan_half_fov,
            ],
            inverse_view: view.inverse().to_cols_array_2d(),
            sunrise: frame.sky.sunrise_color,
            sun: [sun.x, sun.y, sun.z, frame.sky.star_brightness],
            fog_ends: [
                frame.fog.map_or(frame.distance.min(512.0), |f| f.sky_end),
                frame.fog.map_or(2048.0, |f| f.cloud_end),
                if frame.dark_disc { 1.0 } else { 0.0 },
                frame.glint_alpha,
            ],
            end_portal_sprites: frame.end_portal_sprites,
            crumbling_sprite: frame.crumbling_sprite,
            time: [
                frame.shader_game_time,
                f32::from(frame.nether_lighting),
                frame.dynamic_lights.len().min(crate::dynamic_light::MAX) as f32,
                frame.glint_ticks,
            ],
            outline_params: outline_params(self.config.width, self.config.height),
            dynamic_lights: {
                let mut lights = [[0.0; 4]; crate::dynamic_light::MAX];
                for (to, from) in lights.iter_mut().zip(frame.dynamic_lights) {
                    *to = *from;
                }
                lights
            },
            sunrise_fan: sunrise_fan::normals(frame.sky.sunrise_color[3], sun),
            filtering: [
                self.texture_filtering as f32,
                self.sprite_textures[0].width() as f32,
                0.0,
                0.0,
            ],
        };
        self.uploads
            .buffer(&mut encoder, &self.globals, 0, bytemuck::bytes_of(&globals));
        let hand_projection = Mat4::from_cols_array(
            &crate::camera::hand_projection(
                frame.hand_fov_degrees,
                self.config.width as f32,
                self.config.height.max(1) as f32,
                frame.depth_far,
            ),
        );
        let hand_globals = Globals {
            view_proj: (hand_projection * frame.bob * view).to_cols_array_2d(),
            ..globals
        };
        self.uploads
            .buffer(
                &mut encoder,
                &self.hand_globals,
                0,
                bytemuck::bytes_of(&hand_globals),
            );
        let crosshair_globals = Globals {
            view_proj: (hand_projection * view).to_cols_array_2d(),
            fog_color: [0.0; 4],
            ..globals
        };
        self.uploads
            .buffer(
                &mut encoder,
                &self.crosshair_globals,
                0,
                bytemuck::bytes_of(&crosshair_globals),
            );
        let cull_proj = views.cull().view_proj(pose);
        let view = Frustum {
            planes: frustum(cull_proj),
        };
        let camera = frame.camera;
        let camera_section = camera_section(camera, self.layout.min_y());
        let eye = projection_center(cull_proj).filter(|_| self.sides_on);
        let culling = !path_frame && !frame.title && frame.cave_culling;
        if culling {
            crate::spans::time(
                "occlusion",
                || { self.start_occlusion(camera, frame.detail_distance) },
            );
        }
        let reachable = &self.occlusion;
        let cutout = &self.lod_cutout;
        let leaving: crate::fast_hash::FxHashSet<(i32, i32)> = self
            .lod_leaving
            .iter()
            .map(|&((x, _, z), _)| (x, z))
            .collect();
        let mut visible = std::mem::take(&mut self.visible);
        visible.clear();
        crate::spans::time(
            "visibility",
            || {
                self.sections
                    .visible(
                        camera,
                        (frame.detail_distance / 16.0) as i32,
                        &view,
                        |k| !culling || reachable.contains(k),
                        |x, z| {
                            (!cutout.is_empty() && cutout.contains(&(x, z)))
                                || (!leaving.is_empty() && leaving.contains(&(x, z)))
                        },
                        &mut visible,
                    )
            },
        );
        let now = std::time::Instant::now();
        let mut visible_lods = std::mem::take(&mut self.visible_lods);
        visible_lods.clear();
        if frame.title {
            visible.clear();
        } else {
            visible_lods
                .extend(
                    self
                        .lods
                        .iter()
                        .filter_map(|(&key, tile)| tile.visible(key, camera, &view, now)),
                );
            visible_lods.sort_unstable_by_key(|v| (v.distance, v.key));
        }
        let shadow_view = self
            .pack_post
            .as_ref()
            .and_then(|p| p.1.as_ref())
            .filter(|_| !frame.title)
            .and_then(|p| p.shadow_view(frame));
        let mut shadow_visible = std::mem::take(&mut self.shadow_visible);
        shadow_visible.clear();
        if let Some(v) = &shadow_view {
            let radius = (frame.detail_distance / 16.0) as i32;
            let radius = v
                .distance
                .map_or(radius, |d| radius.min((d / 16.0).ceil() as i32));
            let light = Frustum { planes: frustum(v.cull) };
            crate::spans::time(
                "shadow visibility",
                || {
                    self.sections
                        .visible(
                            camera,
                            radius,
                            &light,
                            |_| true,
                            |_, _| false,
                            &mut shadow_visible,
                        )
                },
            );
            let light = v.light;
            shadow_visible
                .sort_unstable_by(|a, b| {
                    let height = |s: &resident::Visible| {
                        glam::Vec3::from(s.origin).dot(light)
                    };
                    height(b).total_cmp(&height(a))
                });
        }
        self.drawn_sections = visible.len() + visible_lods.len();
        let instances = self.drawn_sections + shadow_visible.len();
        if instances as u64 > self.instance_capacity {
            self.instance_capacity = (instances as u64).next_power_of_two();
            self.instances = self
                .device
                .create_buffer(
                    &wgpu::BufferDescriptor {
                        label: Some("Section origins"),
                        size: self.instance_capacity * 16,
                        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST
                            | wgpu::BufferUsages::STORAGE,
                        mapped_at_creation: false,
                    },
                );
        }
        self.origins.clear();
        let (sections, retiring) = (&self.sections, &self.lod_retiring);
        let fade_ms = fade_millis(frame.chunk_fade);
        let uploaded = &self.section_uploaded;
        let visibility = |key: SectionKey| {
            if fade_ms == 0 {
                return 1.0;
            }
            uploaded
                .get(&key)
                .map_or(
                    1.0,
                    |t| {
                        chunk_visibility(
                            now.saturating_duration_since(*t).as_millis() as u64,
                            fade_ms,
                        )
                    },
                )
        };
        self.origins
            .extend(
                visible
                    .iter()
                    .map(|v| {
                        let code = sections
                            .get(v.slot)
                            .fade(now, retiring.contains(&v.key));
                        section_instance(v.origin, code, visibility(v.key))
                    })
                    .chain(visible_lods.iter().map(|v| lod::instance(v.origin, v.fade)))
                    .chain(
                        shadow_visible
                            .iter()
                            .map(|v| section_instance(v.origin, 0, visibility(v.key))),
                    ),
            );
        if !self.origins.is_empty() {
            self.uploads
                .buffer(
                    &mut encoder,
                    &self.instances,
                    0,
                    bytemuck::cast_slice(&self.origins),
                );
        }
        self.draws.clear();
        self.draw_pages.clear();
        let pack_terrain = !frame.title
            && self
                .pack_post
                .as_ref()
                .and_then(|p| p.1.as_ref())
                .is_some_and(|p| p.has_terrain());
        let mut cull = self
            .cull
            .as_mut()
            .filter(|c| {
                !path_frame && self.cull_on && !frame.title && !pack_terrain
                    && self.multi_draw && visible.len() >= c.min_sections
            })
            .and_then(|cull| cull.begin(&self.device).then_some(cull));
        self.phase2_draws.clear();
        self.phase2_pages.clear();
        self.solid_runs.clear();
        self.phase2_runs.clear();
        for (i, v) in visible.iter().enumerate() {
            let s = self.sections.get(v.slot);
            let first_phase = cull.as_ref().is_none_or(|c| c.first(v.slot));
            let (draws, pages) = if first_phase {
                (&mut self.draws, &mut self.draw_pages)
            } else {
                (&mut self.phase2_draws, &mut self.phase2_pages)
            };
            let start = draws.len();
            let instances = u32::from(first_phase);
            let mut indices = 0;
            facing_runs(
                &s.sides,
                eye,
                v.origin,
                |first, quads| {
                    indices += quads * 6;
                    let base = s.start as u32 + first * 4;
                    draws.push([quads * 6, instances, 0, base, i as u32]);
                    pages.push((s.page, None));
                },
            );
            let runs = if first_phase {
                &mut self.solid_runs
            } else {
                &mut self.phase2_runs
            };
            runs.push(
                v.distance,
                s.page,
                start,
                draws.len() - start,
                [i as u32, v.slot, indices],
            );
            if let Some(c) = &mut cull && first_phase {
                c.drawn(i, v.slot, indices);
            }
        }
        self.solid_runs.regroup(&mut self.draws, &mut self.draw_pages, 0);
        let runs = self
            .phase2_runs
            .regroup(&mut self.phase2_draws, &mut self.phase2_pages, 0);
        if let Some(c) = &mut cull {
            for r in runs {
                let [i, slot, indices] = r.tag;
                c.deferred(i as usize, slot, r.start as usize, r.len as usize, indices);
            }
        }
        let tiles = self.draws.len();
        self.solid_runs.clear();
        for (i, v) in visible_lods.iter().enumerate() {
            let s = self.lods[&v.key].drawn();
            let start = self.draws.len();
            let instance = (visible.len() + i) as u32;
            facing_runs(
                &s.sides,
                eye,
                v.origin,
                |first, quads| {
                    let base = s.start as u32 + first * 4;
                    self.draws.push([quads * 6, 1, 0, base, instance]);
                    self.draw_pages.push((s.page, None));
                },
            );
            self.solid_runs
                .push(v.distance, s.page, start, self.draws.len() - start, [0; 3]);
        }
        self.solid_runs.regroup(&mut self.draws, &mut self.draw_pages, tiles);
        let phase1_draws = self.draws.len() as u32;
        self.draws.extend_from_slice(&self.phase2_draws);
        self.draw_pages.extend_from_slice(&self.phase2_pages);
        let culling = cull.is_some();
        let solid_draws = self.draws.len() as u32;
        self.translucent_draws.clear();
        let count = visible.len() + visible_lods.len();
        let budget = (count / 8).max(15).min(count);
        self.translucency_cursor %= count.max(1);
        for (i, v) in visible.iter().enumerate().rev() {
            let s = self.sections.get_mut(v.slot);
            if let Some((index_page, index_start)) = s.sorted_indices {
                let nearby = (v.key.0 - camera_section.0).abs() <= 2
                    && (v.key.1 - camera_section.1).abs() <= 2
                    && (v.key.2 - camera_section.2).abs() <= 2;
                let scheduled = (i + count - self.translucency_cursor) % count < budget;
                let origin = [
                    v.key.0 * 16,
                    self.layout.min_y() + v.key.1 * 16,
                    v.key.2 * 16,
                ];
                if (!s.sort.is_sorted() || nearby || scheduled)
                    && let Some(indices) = s
                        .sort
                        .update(camera.to_array(), origin, &mut self.translucent_sorter)
                {
                    self.uploads
                        .buffer(
                            &mut encoder,
                            self.sorted_indices.buffer(index_page),
                            index_start * 4,
                            bytemuck::cast_slice(indices),
                        );
                }
                let first = s.start + u64::from(s.solid) * 4;
                self.translucent_draws
                    .push((
                        v.distance,
                        i,
                        [
                            s.translucent * 6,
                            1,
                            index_start as u32,
                            first as u32,
                            i as u32,
                        ],
                        (s.page, Some(index_page)),
                    ));
            }
        }
        let section_translucent = self.translucent_draws.len();
        for (i, v) in visible_lods.iter().enumerate() {
            let instance = visible.len() + i;
            let tile = self.lods.get_mut(&v.key).expect("visible resident tile");
            let s = tile.drawn_mut();
            if let Some((index_page, index_start)) = s.sorted_indices {
                let scheduled = (instance + count - self.translucency_cursor) % count
                    < budget;
                if (!s.sort.is_sorted() || scheduled)
                    && let Some(indices) = s
                        .sort
                        .update_lod(
                            camera.to_array(),
                            v.key.origin(),
                            &mut self.translucent_sorter,
                        )
                {
                    self.uploads
                        .buffer(
                            &mut encoder,
                            self.sorted_indices.buffer(index_page),
                            index_start * 4,
                            bytemuck::cast_slice(indices),
                        );
                }
                let first = s.start + u64::from(s.solid) * 4;
                self.translucent_draws
                    .push((
                        v.distance,
                        instance,
                        [
                            s.translucent * 6,
                            1,
                            index_start as u32,
                            first as u32,
                            instance as u32,
                        ],
                        (s.page, Some(index_page)),
                    ));
            }
        }
        if self.translucent_draws.len() > section_translucent {
            let tier = |instance: usize| match instance.checked_sub(visible.len()) {
                None => 0,
                Some(tile) if visible_lods[tile].old => 2,
                Some(_) => 1,
            };
            self.translucent_draws
                .sort_unstable_by_key(|&(distance, instance, ..)| {
                    (
                        tier(instance),
                        std::cmp::Reverse(distance),
                        std::cmp::Reverse(instance),
                    )
                });
        }
        for &(_, _, draw, pages) in &self.translucent_draws {
            self.draws.push(draw);
            self.draw_pages.push(pages);
        }
        self.translucency_cursor += budget;
        let translucent_draws = self.draws.len() as u32 - solid_draws;
        self.section_draws = self.draws.len();
        let shadow_first = self.draws.len() as u32;
        let mut shadow_translucent = Vec::new();
        self.shadow_runs.clear();
        if let Some(view) = &shadow_view {
            let base = self.drawn_sections;
            let toward = view.light * 1.0e5;
            let height = |v: &resident::Visible| {
                glam::Vec3::from(v.origin).dot(view.light)
            };
            let top = shadow_visible.first().map_or(0.0, height);
            for (k, v) in shadow_visible.iter().enumerate() {
                let s = self.sections.get_mut(v.slot);
                let instance = (base + k) as u32;
                let depth = f64::from(top - height(v)).max(0.0);
                let distance = (depth * depth).to_bits();
                if let Some((index_page, index_start)) = s.sorted_indices
                    && !s.sort.is_sorted()
                {
                    let origin = [
                        v.key.0 * 16,
                        self.layout.min_y() + v.key.1 * 16,
                        v.key.2 * 16,
                    ];
                    if let Some(indices) = s
                        .sort
                        .update(camera.to_array(), origin, &mut self.translucent_sorter)
                    {
                        self.uploads
                            .buffer(
                                &mut encoder,
                                self.sorted_indices.buffer(index_page),
                                index_start * 4,
                                bytemuck::cast_slice(indices),
                            );
                    }
                }
                let before = self.draws.len();
                facing_runs(
                    &s.sides,
                    Some(toward),
                    v.origin,
                    |first, quads| {
                        let start = s.start as u32 + first * 4;
                        self.draws.push([quads * 6, 1, 0, start, instance]);
                        self.draw_pages.push((s.page, None));
                    },
                );
                let len = self.draws.len() - before;
                self.shadow_runs.push(distance, s.page, before, len, [0; 3]);
                if let Some((index_page, index_start)) = s.sorted_indices {
                    let start = s.start + u64::from(s.solid) * 4;
                    shadow_translucent
                        .push((
                            distance,
                            [
                                s.translucent * 6,
                                1,
                                index_start as u32,
                                start as u32,
                                instance,
                            ],
                            (s.page, Some(index_page)),
                        ));
                }
            }
        }
        let shadow_solid = self.draws.len() as u32 - shadow_first;
        self.shadow_runs
            .regroup(&mut self.draws, &mut self.draw_pages, shadow_first as usize);
        self.shadow_runs.clear();
        for (distance, draw, pages) in shadow_translucent {
            self.shadow_runs.push(distance, pages.0, self.draws.len(), 1, [0; 3]);
            self.draws.push(draw);
            self.draw_pages.push(pages);
        }
        self.shadow_runs
            .regroup(
                &mut self.draws,
                &mut self.draw_pages,
                (shadow_first + shadow_solid) as usize,
            );
        let shadow_translucent = self.draws.len() as u32 - shadow_first - shadow_solid;
        self.section_draw_calls = if self.multi_draw
            && self.info.backend != wgpu::Backend::Metal
        {
            (0..self.section_draws)
                .filter(|&i| {
                    i == 0 || i == phase1_draws as usize || i == solid_draws as usize
                        || self.draw_pages[i] != self.draw_pages[i - 1]
                })
                .count()
        } else {
            self.section_draws
        };
        let debug_depth = frame.debug_lines.len() as u32;
        let debug_top = frame.debug_lines_on_top.len() as u32;
        let crosshair = frame.debug_crosshair.len() as u32;
        if debug_depth + debug_top + crosshair > 0 {
            let needed = u64::from(debug_depth + debug_top + crosshair);
            if needed > self.debug_line_capacity {
                self.debug_line_capacity = needed.next_power_of_two();
                self.debug_line_buffer = self
                    .device
                    .create_buffer(
                        &wgpu::BufferDescriptor {
                            label: Some("Debug lines"),
                            size: 32 * self.debug_line_capacity,
                            usage: wgpu::BufferUsages::VERTEX
                                | wgpu::BufferUsages::COPY_DST,
                            mapped_at_creation: false,
                        },
                    );
            }
            let all: Vec<[f32; 8]> = frame
                .debug_lines
                .iter()
                .chain(frame.debug_lines_on_top)
                .chain(frame.debug_crosshair)
                .copied()
                .collect();
            self.uploads
                .buffer(
                    &mut encoder,
                    &self.debug_line_buffer,
                    0,
                    bytemuck::cast_slice(&all),
                );
        }
        self.weather
            .prepare(
                &self.device,
                &mut encoder,
                &mut self.uploads,
                frame.weather,
                frame.weather_time,
            );
        let outline_edges = frame.outline.len().min(MAX_OUTLINE_EDGES as usize);
        if outline_edges > 0 {
            let edges = outline_vertices(&frame.outline[..outline_edges], camera);
            let width = outline_width(self.config.width);
            let stroke = |color: u32, width: f32| {
                edges
                    .iter()
                    .map(move |e| {
                        let mut line = [0.0; 8];
                        line[..6].copy_from_slice(e);
                        line[6] = f32::from_bits(color);
                        line[7] = width;
                        line
                    })
            };
            let lines: Vec<[f32; 8]> = if frame.outline_high_contrast {
                stroke(0xff00_0000, 7.0).chain(stroke(0xff57_ffe1, width)).collect()
            } else {
                stroke(0x6600_0000, width).collect()
            };
            self.uploads
                .buffer(&mut encoder, &self.outline, 0, bytemuck::cast_slice(&lines));
        }
        let mut entities = frame.entities;
        if !frame.title && let Some((_, Some(post))) = &mut self.pack_post
            && post.has_entities()
        {
            post.prepare_entities(
                &self.device,
                &self.queue,
                frame.entities,
                frame.entity_spans,
                &mut self.pack_entity_rest,
            );
            entities = &self.pack_entity_rest;
        }
        let oit_on = !path_frame && self.improved_transparency && !frame.title
            && self.pack_post.as_ref().is_none_or(|p| p.1.is_none())
            && self.native.is_none() && oit::supported(&self.device);
        let [entity_quads, text_quads, crumbling_quads, translucent_crumbling_quads,
        water_mask_quads, swirl_quads, shadow_quads, translucent_particle_quads,
        translucent_item_quads, oit_entity_quads] = self
            .entity_batch
            .update(entities, frame.additive, frame.hand, frame.see_through, oit_on);
        let weather_quads = frame.weather.len() as u32 / 4;
        let world_quads = entity_quads + text_quads + crumbling_quads
            + translucent_crumbling_quads + water_mask_quads + swirl_quads + shadow_quads
            + translucent_particle_quads + translucent_item_quads + oit_entity_quads;
        let base_quads = world_quads - oit_entity_quads;
        let cloud_quads = if let Some(input) = &frame.clouds {
            let (rebuilt, uniform) = crate::spans::time(
                "clouds",
                || self.cloud_mesh.prepare(input),
            );
            let vertices = self.cloud_mesh.vertices.len() as u64;
            if rebuilt && vertices > 0 {
                if vertices > self.cloud_capacity {
                    self.cloud_capacity = vertices.next_power_of_two();
                    self.cloud_buffer = self
                        .device
                        .create_buffer(
                            &wgpu::BufferDescriptor {
                                label: Some("Cloud vertices"),
                                size: self.cloud_capacity
                                    * std::mem::size_of::<Vertex>() as u64,
                                usage: wgpu::BufferUsages::VERTEX
                                    | wgpu::BufferUsages::COPY_DST,
                                mapped_at_creation: false,
                            },
                        );
                }
                self.uploads
                    .buffer(
                        &mut encoder,
                        &self.cloud_buffer,
                        0,
                        bytemuck::cast_slice(&self.cloud_mesh.vertices),
                    );
            }
            self.uploads
                .buffer(
                    &mut encoder,
                    &self.cloud_uniform,
                    0,
                    bytemuck::bytes_of(&uniform),
                );
            (vertices / 4) as u32
        } else {
            0
        };
        self.lightning
            .prepare(
                &self.device,
                &mut self.uploads,
                &mut encoder,
                &frame.lightning,
                camera,
            );
        let all_entities = &self.entity_batch.combined;
        let hand_quads = (frame.hand.len() / 4) as u32;
        let additive_quads = (frame.additive.len() / 4) as u32
            - self.entity_batch.moved_additive;
        let see_through_quads = (frame.see_through.len() / 4) as u32;
        let entity_vertices = all_entities.len() as u64;
        if entity_vertices > self.entity_capacity {
            self.entity_capacity = entity_vertices.next_power_of_two();
            self.entity_buffer = self
                .device
                .create_buffer(
                    &wgpu::BufferDescriptor {
                        label: Some("Entity vertices"),
                        size: self.entity_capacity
                            * std::mem::size_of::<Vertex>() as u64,
                        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    },
                );
        }
        if entity_vertices > 0 {
            self.uploads
                .buffer(
                    &mut encoder,
                    &self.entity_buffer,
                    0,
                    bytemuck::cast_slice(all_entities),
                );
        }
        self.entity_outline
            .prepare(
                &self.device,
                &mut self.uploads,
                &mut encoder,
                frame.glowing,
                [self.config.width, self.config.height],
            );
        if let Some((rotation, brightness)) = frame.stars {
            self.stars
                .prepare(
                    &mut self.uploads,
                    &mut encoder,
                    jittered * rotation,
                    brightness,
                );
        }
        let celestial_vertices = frame.celestial.len().min(2048 * 4);
        if frame.end_sky && !frame.title {
            self.end_sky.prepare(&self.device, frame.end_sky_sprite);
        }
        if celestial_vertices > 0 {
            self.uploads
                .buffer(
                    &mut encoder,
                    &self.celestial_buffer,
                    0,
                    bytemuck::cast_slice(&frame.celestial[..celestial_vertices]),
                );
        }
        let max_quads = (world_quads + additive_quads + hand_quads + see_through_quads)
            .max(cloud_quads)
            .max(if frame.lightning.bolts.is_empty() { 0 } else { lightning::QUADS })
            .max(weather_quads)
            .max(frame.glowing.len() as u32 / 4);
        if max_quads > self.index_quads {
            self.index_quads = max_quads.next_power_of_two();
            self.indices = Self::quad_indices(&self.device, self.index_quads);
        }
        if let Some(b) = &frame.border {
            if b.rebuilt {
                self.uploads
                    .buffer(
                        &mut encoder,
                        &self.border_vertices,
                        0,
                        bytemuck::cast_slice(&b.vertices),
                    );
            }
            let t = &b.transforms;
            let uniform = BorderUniform {
                color: t.color,
                offset: [
                    t.model_offset[0],
                    t.model_offset[1],
                    t.model_offset[2],
                    t.texture_offset,
                ],
            };
            self.uploads
                .buffer(
                    &mut encoder,
                    &self.border_uniform,
                    0,
                    bytemuck::bytes_of(&uniform),
                );
        }
        if !frame.screen.is_empty() {
            let vertices: Vec<crate::screen_effects::ScreenVertex> = frame
                .screen
                .iter()
                .flat_map(|q| q.vertices)
                .collect();
            if vertices.len() as u64 > self.screen_capacity {
                self.screen_capacity = (vertices.len() as u64).next_power_of_two();
                self.screen_buffer = self
                    .device
                    .create_buffer(
                        &wgpu::BufferDescriptor {
                            label: Some("Screen effect vertices"),
                            size: self.screen_capacity
                                * std::mem::size_of::<crate::screen_effects::ScreenVertex>()
                                    as u64,
                            usage: wgpu::BufferUsages::VERTEX
                                | wgpu::BufferUsages::COPY_DST,
                            mapped_at_creation: false,
                        },
                    );
            }
            self.uploads
                .buffer(
                    &mut encoder,
                    &self.screen_buffer,
                    0,
                    bytemuck::cast_slice(&vertices),
                );
        }
        self.hud
            .prepare(
                &self.device,
                &mut self.uploads,
                &mut encoder,
                frame.hud,
                [self.config.width, self.config.height],
            );
        if culling && let Some(cull) = &mut self.cull {
            cull.upload(
                &self.device,
                &mut self.uploads,
                &mut encoder,
                &self.draws[phase1_draws as usize..solid_draws as usize],
                &self.instances,
                &self.globals,
            );
        }
        let flat_frame = match &mut self.flat {
            Some(
                flat,
            ) if self.flat_on && !self.draws.is_empty()
                && (self.drawn_sections >= flat.min_sections || pack_terrain
                    || shadow_view.is_some()) => {
                let (sections, retiring) = (&self.sections, &self.lod_retiring);
                let lods = &self.lods;
                let sorted = &self.sorted_indices;
                let phase2 = self.cull.as_ref().filter(|_| culling).map(|c| &c.phase2);
                crate::spans::time(
                    "flatten",
                    || {
                        flat.prepare(
                            &self.device,
                            &mut encoder,
                            &mut self.uploads,
                            &self.draws,
                            &self.draw_pages,
                            phase1_draws as usize,
                            solid_draws as usize,
                            [shadow_first, shadow_first + shadow_solid]
                                .map(|d| d as usize),
                            phase2,
                            visible
                                .iter()
                                .chain(&shadow_visible)
                                .filter_map(|v| {
                                    let section = sections.get(v.slot);
                                    let code = section.fade(now, retiring.contains(&v.key));
                                    Some((
                                        section.flat_id?,
                                        section_instance(v.origin, code, visibility(v.key)),
                                    ))
                                })
                                .chain(
                                    visible_lods
                                        .iter()
                                        .filter_map(|v| {
                                            let id = lods[&v.key].drawn().flat_id?;
                                            Some((id, lod::instance(v.origin, v.fade)))
                                        }),
                                ),
                            |page| sorted.buffer(page).clone(),
                        )
                    },
                )
            }
            _ => false,
        };
        if flat_frame {
            self.section_draw_calls = self
                .flat
                .as_ref()
                .map_or(0, |f| f.shadow_calls[0]);
        } else if self.multi_draw && !self.draws.is_empty() {
            if self.draws.len() as u64 > self.indirect_capacity {
                self.indirect_capacity = (self.draws.len() as u64).next_power_of_two();
                self.indirect = self
                    .device
                    .create_buffer(
                        &wgpu::BufferDescriptor {
                            label: Some("Section draws"),
                            size: self.indirect_capacity * 20,
                            usage: wgpu::BufferUsages::INDIRECT
                                | wgpu::BufferUsages::COPY_DST,
                            mapped_at_creation: false,
                        },
                    );
            }
            self.uploads
                .buffer(
                    &mut encoder,
                    &self.indirect,
                    0,
                    bytemuck::cast_slice(&self.draws),
                );
        }
        let view_target = target_texture
            .create_view(
                &wgpu::TextureViewDescriptor {
                    format: Some(self.config.format.remove_srgb_suffix()),
                    ..Default::default()
                },
            );
        if let Some(timer) = &mut self.gpu_timer {
            timer.begin(&mut encoder);
        }
        let pack_post = !path_frame && !frame.title
            && self.pack_post.as_ref().is_some_and(|p| p.1.is_some());
        let native_post = !path_frame && !frame.title
            && self.native.as_ref().is_some_and(|n| n.post.is_some());
        if let Some(timer) = Self::pass_timer(&mut self.pack_post, &mut self.native)
            && timer.is_some() != self.pass_timing
        {
            *timer = self
                .pass_timing
                .then(|| pack_timing::PassTiming::new(&self.device, &self.queue))
                .flatten();
        }
        let pack_reads_world = pack_post || native_post;
        let keep_depth = pack_reads_world || (taa && self.world_post.taa) || oit_on;
        let pack_draws_terrain = pack_terrain && flat_frame == self.pack_terrain_flat;
        let pack_draws_shadow = shadow_view.is_some()
            && flat_frame == self.pack_terrain_flat;
        let pack_draws_sky = pack_post
            && self
                .pack_post
                .as_ref()
                .and_then(|p| p.1.as_ref())
                .is_some_and(|p| p.has_sky());
        let pack_draws_celestial = pack_post
            && self
                .pack_post
                .as_ref()
                .and_then(|p| p.1.as_ref())
                .is_some_and(|p| p.has_celestial());
        let pack_draws_water = pack_draws_terrain
            && self
                .pack_post
                .as_ref()
                .and_then(|p| p.1.as_ref())
                .is_some_and(|p| p.has_water());
        let pack_draws_clouds = pack_post
            && self
                .pack_post
                .as_ref()
                .and_then(|p| p.1.as_ref())
                .is_some_and(|p| p.has_clouds());
        let pack_draws_hand = pack_post
            && self
                .pack_post
                .as_ref()
                .and_then(|p| p.1.as_ref())
                .is_some_and(|p| p.has_hand());
        let pack_draws_weather = pack_post
            && self
                .pack_post
                .as_ref()
                .and_then(|p| p.1.as_ref())
                .is_some_and(|p| p.has_weather());
        let scaled = if path_frame {
            None
        } else {
            self.world_post
                .prepare(
                    &self.device,
                    [self.config.width, self.config.height],
                    pack_reads_world,
                )
        };
        let (world_view, world_depth) = match &scaled {
            Some((color, depth)) => (color, depth),
            None => (&view_target, &self.depth),
        };
        if path_frame {
            state
                .tracer
                .as_mut()
                .unwrap()
                .render(
                    &self.device,
                    &self.queue,
                    &mut encoder,
                    &view_target,
                    frame,
                    [self.config.width, self.config.height],
                );
        } else {
            if culling && let Some(cull) = &mut self.cull {
                cull.bind_depth(&self.device, world_depth);
            }
            if oit_on {
                let size = self
                    .world_post
                    .size()
                    .filter(|_| scaled.is_some())
                    .unwrap_or([self.config.width, self.config.height]);
                let (device, layout, sprites) = (
                    &self.device,
                    &self.terrain_layout,
                    self.terrain_sprites,
                );
                let clouds = [0, 1]
                    .map(|i| self.cloud_pipeline.get_bind_group_layout(i));
                let format = self.config.format.remove_srgb_suffix();
                let flat = self
                    .flat
                    .as_ref()
                    .map(|f| f.translucent.get_bind_group_layout(1));
                let features = [
                    self.entity_pipeline.get_bind_group_layout(0),
                    self.weather.pipeline().get_bind_group_layout(1),
                    self.border_pipeline.get_bind_group_layout(0),
                ];
                self.oit
                    .get_or_insert_with(|| {
                        oit::Oit::new(
                            device,
                            layout,
                            flat.as_ref(),
                            [&features[0], &features[1], &features[2]],
                            [&clouds[0], &clouds[1]],
                            sprites,
                            format,
                        )
                    })
                    .prepare(
                        &self.device,
                        &self.queue,
                        size,
                        &views.primary().projection,
                        [water_mask_quads > 0, cloud_quads > 0],
                    );
            }
            let oit = self.oit.as_ref().filter(|_| oit_on);
            let cull = self.cull.as_ref().filter(|_| culling && solid_draws > 0);
            let clear = if frame.title
                || (frame.end_sky && !frame.underwater && sky_open(frame))
            {
                wgpu::Color::BLACK
            } else {
                wgpu::Color {
                    r: f64::from(fog_color[0]),
                    g: f64::from(fog_color[1]),
                    b: f64::from(fog_color[2]),
                    a: 1.0,
                }
            };
            let sky_visible = sky_open(frame);
            let mut native_sky_pass = false;
            if !frame.title && let Some(native) = &mut self.native {
                let size = self
                    .world_post
                    .size()
                    .filter(|_| scaled.is_some())
                    .unwrap_or([self.config.width, self.config.height]);
                native.frame.write(&self.queue, frame, size);
                if let Some(t) = &mut native.timing {
                    t.begin(&self.device, &self.queue);
                    native_sky_pass = native.sky.is_some() && t.recording()
                        && sky_visible && frame.skybox;
                }
            }
            if native_sky_pass && let Some(native) = &mut self.native
                && let Some(sky) = &native.sky
            {
                if let Some(t) = &mut native.timing {
                    let format = self.config.format.remove_srgb_suffix();
                    t.mark(&self.device, &mut encoder, (world_view, format));
                }
                let mut pass = encoder
                    .begin_render_pass(
                        &wgpu::RenderPassDescriptor {
                            label: Some(&sky.label),
                            color_attachments: &[
                                Some(wgpu::RenderPassColorAttachment {
                                    view: world_view,
                                    resolve_target: None,
                                    depth_slice: None,
                                    ops: wgpu::Operations {
                                        load: wgpu::LoadOp::Clear(clear),
                                        store: wgpu::StoreOp::Store,
                                    },
                                }),
                            ],
                            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                                view: world_depth,
                                depth_ops: Some(wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(0.0),
                                    store: wgpu::StoreOp::Discard,
                                }),
                                stencil_ops: None,
                            }),
                            timestamp_writes: native
                                .timing
                                .as_mut()
                                .and_then(|t| t.writes(&sky.label)),
                            ..Default::default()
                        },
                    );
                pass.set_bind_group(0, &self.group, &[]);
                sky.draw(&mut pass, &native.frame);
            }
            let material = self
                .native
                .as_ref()
                .filter(|_| !frame.title)
                .and_then(|n| Some((n.material.as_ref()?, &n.frame.group)));
            let solid_pipeline = material.map_or(&self.solid, |(m, _)| &m.solid);
            let translucent_pipeline = material
                .map_or(&self.translucent, |(m, _)| &m.translucent);
            let pack_flat = material.and_then(|(m, _)| m.flat.as_ref());
            let pack_frame = |pass: &mut wgpu::RenderPass<'_>| {
                if let Some((_, group)) = material {
                    pass.set_bind_group(horizon_shader_api::stages::GROUP, group, &[]);
                }
            };
            {
                let attachments = [
                    Some(wgpu::RenderPassColorAttachment {
                        view: world_view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: if native_sky_pass {
                                wgpu::LoadOp::Load
                            } else {
                                wgpu::LoadOp::Clear(clear)
                            },
                            store: wgpu::StoreOp::Store,
                        },
                    }),
                ];
                let mut pass = encoder
                    .begin_render_pass(
                        &wgpu::RenderPassDescriptor {
                            label: Some("Horizon world"),
                            color_attachments: &attachments,
                            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                                view: world_depth,
                                depth_ops: Some(wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(0.0),
                                    store: if cull.is_some() || keep_depth {
                                        wgpu::StoreOp::Store
                                    } else {
                                        wgpu::StoreOp::Discard
                                    },
                                }),
                                stencil_ops: None,
                            }),
                            ..Default::default()
                        },
                    );
                pass.set_bind_group(0, &self.group, &[]);
                pack_frame(&mut pass);
                if frame.title || (sky_visible && (frame.skybox || frame.end_sky)) {
                    if !frame.title && frame.skybox && !pack_draws_sky
                        && !native_sky_pass
                    {
                        match self
                            .native
                            .as_ref()
                            .and_then(|n| Some((n.sky.as_ref()?, &n.frame)))
                        {
                            Some((sky, native)) => sky.draw(&mut pass, native),
                            None => {
                                pass.set_pipeline(&self.sky);
                                pass.draw(0..3, 0..1);
                            }
                        }
                    }
                    if frame.title {
                        self.panorama.draw(&mut pass);
                        pass.set_bind_group(0, &self.group, &[]);
                    } else if celestial_vertices > 0 || frame.end_sky {
                        pass.set_pipeline(&self.celestial_pipeline);
                        pass.set_bind_group(0, &self.entity_group, &[]);
                        pass.set_index_buffer(
                            self.indices.slice(..),
                            wgpu::IndexFormat::Uint32,
                        );
                        if frame.end_sky {
                            self.end_sky.draw(&mut pass);
                        }
                        if celestial_vertices > 0 && !pack_draws_celestial {
                            pass.set_vertex_buffer(0, self.celestial_buffer.slice(..));
                            pass.draw_indexed(
                                0..(celestial_vertices as u32 / 4) * 6,
                                0,
                                0..1,
                            );
                        }
                        pass.set_bind_group(0, &self.group, &[]);
                    }
                    if !frame.title && frame.stars.is_some() && !pack_draws_sky {
                        self.stars.draw(&mut pass);
                        pass.set_bind_group(0, &self.group, &[]);
                    }
                    if !frame.title && frame.dark_disc && !pack_draws_sky {
                        pass.set_pipeline(&self.dark_disc);
                        pass.draw(0..3, 0..1);
                    }
                }
                match self.flat.as_ref().filter(|_| flat_frame && !pack_draws_terrain) {
                    _ if pack_draws_terrain => {
                        pass.set_index_buffer(
                            self.indices.slice(..),
                            wgpu::IndexFormat::Uint32,
                        );
                    }
                    Some(flat) => {
                        pass.set_pipeline(pack_flat.map_or(&flat.solid, |p| &p.0));
                        let vertices = |p| self.arena.buffer(p);
                        match cull {
                            Some(cull) => {
                                flat.draw(
                                    &mut pass,
                                    &flat.calls[..flat.phase1_calls],
                                    vertices,
                                );
                                cull.test(&mut pass);
                                drop(pass);
                                cull.test_pass(&mut encoder);
                                flat.flatten_phase2(&mut encoder);
                                pass = world_pass_after_culling(
                                    &mut encoder,
                                    world_view,
                                    world_depth,
                                    keep_depth,
                                );
                                pass.set_bind_group(0, &self.group, &[]);
                                pack_frame(&mut pass);
                                pass.set_pipeline(pack_flat.map_or(&flat.solid, |p| &p.0));
                                flat.draw_phase2(&mut pass, vertices);
                            }
                            None => {
                                flat.draw(
                                    &mut pass,
                                    &flat.calls[..flat.solid_calls],
                                    vertices,
                                )
                            }
                        }
                        pass.set_bind_group(0, &self.group, &[]);
                        pass.set_index_buffer(
                            self.indices.slice(..),
                            wgpu::IndexFormat::Uint32,
                        );
                    }
                    None => {
                        pass.set_pipeline(solid_pipeline);
                        pass.set_index_buffer(
                            self.indices.slice(..),
                            wgpu::IndexFormat::Uint32,
                        );
                        pass.set_vertex_buffer(1, self.instances.slice(..));
                        match cull {
                            Some(cull) => {
                                self.draw_sections(
                                    &mut pass,
                                    &self.indirect,
                                    0,
                                    0,
                                    phase1_draws,
                                );
                                cull.test(&mut pass);
                                drop(pass);
                                cull.test_pass(&mut encoder);
                                pass = world_pass_after_culling(
                                    &mut encoder,
                                    world_view,
                                    world_depth,
                                    keep_depth,
                                );
                                pass.set_bind_group(0, &self.group, &[]);
                                pack_frame(&mut pass);
                                pass.set_pipeline(solid_pipeline);
                                pass.set_index_buffer(
                                    self.indices.slice(..),
                                    wgpu::IndexFormat::Uint32,
                                );
                                pass.set_vertex_buffer(1, self.instances.slice(..));
                                self.draw_sections(
                                    &mut pass,
                                    &cull.phase2,
                                    phase1_draws,
                                    phase1_draws,
                                    solid_draws - phase1_draws,
                                );
                            }
                            None => {
                                self.draw_sections(
                                    &mut pass,
                                    &self.indirect,
                                    0,
                                    0,
                                    solid_draws,
                                )
                            }
                        }
                    }
                }
                if entity_quads > 0 {
                    pass.set_pipeline(&self.entity_pipeline);
                    pass.set_bind_group(0, &self.entity_group, &[]);
                    pass.set_vertex_buffer(0, self.entity_buffer.slice(..));
                    pass.draw_indexed(0..entity_quads * 6, 0, 0..1);
                    if self.hdr.is_some() {
                        pass.set_pipeline(&self.hdr_mark_pipeline);
                        pass.draw_indexed(0..entity_quads * 6, 0, 0..1);
                    }
                    pass.set_bind_group(0, &self.group, &[]);
                }
                if text_quads > 0 && !oit_on {
                    pass.set_pipeline(&self.text_polygon_offset_pipeline);
                    pass.set_bind_group(0, &self.entity_group, &[]);
                    pass.set_vertex_buffer(0, self.entity_buffer.slice(..));
                    pass.draw_indexed(
                        0..text_quads * 6,
                        (entity_quads * 4) as i32,
                        0..1,
                    );
                    pass.set_bind_group(0, &self.group, &[]);
                }
                if translucent_item_quads > 0 && !oit_on {
                    pass.set_pipeline(&self.translucent_item_pipeline);
                    pass.set_bind_group(0, &self.entity_group, &[]);
                    pass.set_vertex_buffer(0, self.entity_buffer.slice(..));
                    pass.draw_indexed(
                        0..translucent_item_quads * 6,
                        ((base_quads - translucent_item_quads) * 4) as i32,
                        0..1,
                    );
                    pass.set_bind_group(0, &self.group, &[]);
                }
                if !pack_draws_weather && !oit_on {
                    self.weather.draw(&mut pass, &self.entity_group);
                }
                pass.set_bind_group(0, &self.group, &[]);
                if crumbling_quads > 0 && !oit_on {
                    pass.set_pipeline(&self.crumbling_pipeline);
                    pass.set_bind_group(0, &self.entity_group, &[]);
                    pass.set_vertex_buffer(0, self.entity_buffer.slice(..));
                    pass.draw_indexed(
                        0..crumbling_quads * 6,
                        ((entity_quads + text_quads) * 4) as i32,
                        0..1,
                    );
                    pass.set_bind_group(0, &self.group, &[]);
                }
                if shadow_quads > 0 && !oit_on {
                    pass.set_pipeline(&self.shadow_blob_pipeline);
                    pass.set_bind_group(0, &self.entity_group, &[]);
                    pass.set_vertex_buffer(0, self.entity_buffer.slice(..));
                    pass.draw_indexed(
                        0..shadow_quads * 6,
                        ((base_quads - translucent_item_quads
                            - translucent_particle_quads - shadow_quads) * 4) as i32,
                        0..1,
                    );
                    pass.set_bind_group(0, &self.group, &[]);
                }
                if water_mask_quads > 0 && !oit_on {
                    pass.set_pipeline(&self.water_mask_pipeline);
                    pass.set_bind_group(0, &self.entity_group, &[]);
                    pass.set_vertex_buffer(0, self.entity_buffer.slice(..));
                    pass.draw_indexed(
                        0..water_mask_quads * 6,
                        ((entity_quads + text_quads + crumbling_quads
                            + translucent_crumbling_quads) * 4) as i32,
                        0..1,
                    );
                    pass.set_bind_group(0, &self.group, &[]);
                }
                if !frame.lightning.bolts.is_empty() && !oit_on {
                    pass.set_pipeline(&self.lightning_pipeline);
                    pass.set_bind_group(0, &self.entity_group, &[]);
                    self.lightning.draw(&mut pass, &frame.lightning);
                    pass.set_bind_group(0, &self.group, &[]);
                }
                if swirl_quads > 0 && !oit_on {
                    pass.set_pipeline(&self.energy_swirl_pipeline);
                    pass.set_bind_group(0, &self.entity_group, &[]);
                    pass.set_vertex_buffer(0, self.entity_buffer.slice(..));
                    pass.draw_indexed(
                        0..swirl_quads * 6,
                        ((entity_quads + text_quads + crumbling_quads
                            + translucent_crumbling_quads + water_mask_quads) * 4)
                            as i32,
                        0..1,
                    );
                    pass.set_bind_group(0, &self.group, &[]);
                }
                if additive_quads > 0 && !oit_on {
                    pass.set_pipeline(&self.additive_pipeline);
                    pass.set_bind_group(0, &self.entity_group, &[]);
                    pass.set_vertex_buffer(0, self.entity_buffer.slice(..));
                    pass.draw_indexed(
                        0..additive_quads * 6,
                        (world_quads * 4) as i32,
                        0..1,
                    );
                    pass.set_bind_group(0, &self.group, &[]);
                }
                if outline_edges > 0 && !frame.outline_after_terrain {
                    self.draw_outline(
                        &mut pass,
                        outline_edges as u32,
                        frame.outline_high_contrast,
                    );
                }
                if debug_depth > 0 {
                    pass.set_pipeline(&self.debug_lines);
                    pass.set_vertex_buffer(0, self.debug_line_buffer.slice(..));
                    pass.draw(0..4, 0..debug_depth);
                }
                if debug_top > 0 {
                    pass.set_pipeline(&self.debug_lines_on_top);
                    pass.set_vertex_buffer(0, self.debug_line_buffer.slice(..));
                    pass.draw(0..4, debug_depth..debug_depth + debug_top);
                }
                match self.flat.as_ref().filter(|_| flat_frame) {
                    _ if pack_draws_water => {}
                    flat if let Some(oit) = oit => {
                        drop(pass);
                        let mask = |pass: &mut wgpu::RenderPass<'_>| {
                            pass.set_pipeline(&self.water_mask_pipeline);
                            pass.set_bind_group(0, &self.entity_group, &[]);
                            pass.set_index_buffer(
                                self.indices.slice(..),
                                wgpu::IndexFormat::Uint32,
                            );
                            pass.set_vertex_buffer(0, self.entity_buffer.slice(..));
                            pass.draw_indexed(
                                0..water_mask_quads * 6,
                                ((entity_quads + text_quads + crumbling_quads
                                    + translucent_crumbling_quads) * 4) as i32,
                                0..1,
                            );
                        };
                        let clouds = |
                            pass: &mut wgpu::RenderPass<'_>,
                            pipeline: &wgpu::RenderPipeline|
                        {
                            pass.set_pipeline(pipeline);
                            pass.set_bind_group(0, &self.entity_group, &[]);
                            pass.set_bind_group(1, &self.cloud_group, &[]);
                            pass.set_index_buffer(
                                self.indices.slice(..),
                                wgpu::IndexFormat::Uint32,
                            );
                            pass.set_vertex_buffer(0, self.cloud_buffer.slice(..));
                            pass.draw_indexed(0..cloud_quads * 6, 0, 0..1);
                        };
                        let features = |
                            pass: &mut wgpu::RenderPass<'_>,
                            stage: oit::Stage|
                        {
                            let f = oit.features();
                            let s = stage as usize;
                            pass.set_index_buffer(
                                self.indices.slice(..),
                                wgpu::IndexFormat::Uint32,
                            );
                            pass.set_bind_group(0, &self.entity_group, &[]);
                            pass.set_vertex_buffer(0, self.entity_buffer.slice(..));
                            let quads = |
                                pass: &mut wgpu::RenderPass<'_>,
                                pipeline: &wgpu::RenderPipeline,
                                count: u32,
                                first: u32|
                            {
                                if count > 0 {
                                    pass.set_pipeline(pipeline);
                                    pass.draw_indexed(0..count * 6, (first * 4) as i32, 0..1);
                                }
                            };
                            quads(pass, &f.text[s], text_quads, entity_quads);
                            quads(
                                pass,
                                &f.crumbling[s],
                                crumbling_quads,
                                entity_quads + text_quads,
                            );
                            quads(
                                pass,
                                &f.crumbling[s],
                                translucent_crumbling_quads,
                                entity_quads + text_quads + crumbling_quads,
                            );
                            quads(
                                pass,
                                &f.swirl[s],
                                swirl_quads,
                                entity_quads + text_quads + crumbling_quads
                                    + translucent_crumbling_quads + water_mask_quads,
                            );
                            quads(
                                pass,
                                &f.shadow[s],
                                shadow_quads,
                                base_quads - translucent_item_quads
                                    - translucent_particle_quads - shadow_quads,
                            );
                            quads(
                                pass,
                                &f.entity[s],
                                translucent_particle_quads,
                                base_quads - translucent_item_quads
                                    - translucent_particle_quads,
                            );
                            quads(
                                pass,
                                &f.entity[s],
                                translucent_item_quads,
                                base_quads - translucent_item_quads,
                            );
                            quads(pass, &f.entity[s], oit_entity_quads, base_quads);
                            if !frame.lightning.bolts.is_empty() {
                                pass.set_pipeline(&f.lightning[s]);
                                self.lightning.draw(pass, &frame.lightning);
                                pass.set_vertex_buffer(0, self.entity_buffer.slice(..));
                            }
                            quads(pass, &f.additive[s], additive_quads, world_quads);
                            self.weather
                                .draw_with(pass, &f.weather[s], &self.entity_group);
                            if let Some(b) = frame
                                .border
                                .as_ref()
                                .filter(|_| !frame.title)
                            {
                                pass.set_pipeline(&f.border[s]);
                                pass.set_bind_group(0, &self.border_group, &[]);
                                pass.set_vertex_buffer(0, self.border_vertices.slice(..));
                                for &side in &b.sides {
                                    let first = side as u32 * 6;
                                    pass.draw_indexed(first..first + 6, 0, 0..1);
                                }
                            }
                        };
                        let extras = oit::Extras {
                            water_mask: (water_mask_quads > 0).then_some(&mask as _),
                            clouds: (cloud_quads > 0).then_some(&clouds as _),
                            features: Some(&features),
                        };
                        oit.run(
                            &mut encoder,
                            world_view,
                            world_depth,
                            &|pass, stage| {
                                pass.set_bind_group(0, &self.group, &[]);
                                if let Some((flat, pipeline)) = flat.zip(oit.flat(stage)) {
                                    pass.set_pipeline(pipeline);
                                    flat.draw(
                                        pass,
                                        flat.translucent_calls(),
                                        |p| { self.arena.buffer(p) },
                                    );
                                    return;
                                }
                                pass.set_pipeline(oit.terrain(stage));
                                pass.set_index_buffer(
                                    self.indices.slice(..),
                                    wgpu::IndexFormat::Uint32,
                                );
                                pass.set_vertex_buffer(1, self.instances.slice(..));
                                self.draw_sections(
                                    pass,
                                    &self.indirect,
                                    0,
                                    solid_draws,
                                    translucent_draws,
                                );
                            },
                            &extras,
                        );
                        pass = world_pass_after_culling(
                            &mut encoder,
                            world_view,
                            world_depth,
                            true,
                        );
                        pass.set_bind_group(0, &self.group, &[]);
                        pass.set_index_buffer(
                            self.indices.slice(..),
                            wgpu::IndexFormat::Uint32,
                        );
                        if oit_entity_quads > 0 && self.hdr.is_some() {
                            pass.set_pipeline(&self.hdr_mark_pipeline);
                            pass.set_bind_group(0, &self.entity_group, &[]);
                            pass.set_vertex_buffer(0, self.entity_buffer.slice(..));
                            pass.draw_indexed(
                                0..oit_entity_quads * 6,
                                (base_quads * 4) as i32,
                                0..1,
                            );
                            pass.set_bind_group(0, &self.group, &[]);
                        }
                    }
                    Some(flat) => {
                        pass.set_pipeline(pack_flat.map_or(&flat.translucent, |p| &p.1));
                        flat.draw(
                            &mut pass,
                            flat.translucent_calls(),
                            |p| { self.arena.buffer(p) },
                        );
                        pass.set_bind_group(0, &self.group, &[]);
                        pass.set_index_buffer(
                            self.indices.slice(..),
                            wgpu::IndexFormat::Uint32,
                        );
                    }
                    None => {
                        pass.set_pipeline(translucent_pipeline);
                        pass.set_index_buffer(
                            self.indices.slice(..),
                            wgpu::IndexFormat::Uint32,
                        );
                        pass.set_vertex_buffer(1, self.instances.slice(..));
                        self.draw_sections(
                            &mut pass,
                            &self.indirect,
                            0,
                            solid_draws,
                            translucent_draws,
                        );
                    }
                }
                if translucent_particle_quads > 0 && !oit_on {
                    pass.set_pipeline(&self.entity_pipeline);
                    pass.set_bind_group(0, &self.entity_group, &[]);
                    pass.set_index_buffer(
                        self.indices.slice(..),
                        wgpu::IndexFormat::Uint32,
                    );
                    pass.set_vertex_buffer(0, self.entity_buffer.slice(..));
                    pass.draw_indexed(
                        0..translucent_particle_quads * 6,
                        ((base_quads - translucent_item_quads
                            - translucent_particle_quads) * 4) as i32,
                        0..1,
                    );
                    pass.set_bind_group(0, &self.group, &[]);
                }
                if outline_edges > 0 && frame.outline_after_terrain {
                    self.draw_outline(
                        &mut pass,
                        outline_edges as u32,
                        frame.outline_high_contrast,
                    );
                }
                pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
                if let Some(b) = frame
                    .border
                    .as_ref()
                    .filter(|_| !frame.title && !oit_on)
                {
                    pass.set_pipeline(&self.border_pipeline);
                    pass.set_bind_group(0, &self.border_group, &[]);
                    pass.set_index_buffer(
                        self.indices.slice(..),
                        wgpu::IndexFormat::Uint32,
                    );
                    pass.set_vertex_buffer(0, self.border_vertices.slice(..));
                    for &side in &b.sides {
                        let first = side as u32 * 6;
                        pass.draw_indexed(first..first + 6, 0, 0..1);
                    }
                    pass.set_bind_group(0, &self.group, &[]);
                }
                if translucent_crumbling_quads > 0 && !oit_on {
                    pass.set_pipeline(&self.crumbling_pipeline);
                    pass.set_bind_group(0, &self.entity_group, &[]);
                    pass.set_vertex_buffer(0, self.entity_buffer.slice(..));
                    pass.draw_indexed(
                        0..translucent_crumbling_quads * 6,
                        ((entity_quads + text_quads + crumbling_quads) * 4) as i32,
                        0..1,
                    );
                }
                if cloud_quads > 0 && !pack_draws_clouds && !oit_on {
                    pass.set_pipeline(&self.cloud_pipeline);
                    pass.set_bind_group(0, &self.entity_group, &[]);
                    pass.set_bind_group(1, &self.cloud_group, &[]);
                    pass.set_vertex_buffer(0, self.cloud_buffer.slice(..));
                    pass.draw_indexed(0..cloud_quads * 6, 0, 0..1);
                }
                if see_through_quads > 0 {
                    pass.set_pipeline(&self.see_through_pipeline);
                    pass.set_bind_group(0, &self.entity_group, &[]);
                    pass.set_vertex_buffer(0, self.entity_buffer.slice(..));
                    pass.draw_indexed(
                        0..see_through_quads * 6,
                        ((world_quads + additive_quads + hand_quads) * 4) as i32,
                        0..1,
                    );
                }
            }
            if let Some((color, depth)) = &scaled && pack_post
                && let Some((dimension, Some(mut post))) = self.pack_post.take()
            {
                let size = self.world_post.size().unwrap_or([1, 1]);
                let weather = match &self.hud.atlas_texture {
                    Some(atlas) if pack_draws_weather => {
                        self.pack_weather
                            .get_or_insert_with(|| pack_weather::PackWeather::new(
                                &self.device,
                            ))
                            .prepare(
                                &self.device,
                                &mut encoder,
                                &mut self.uploads,
                                atlas,
                                frame.weather,
                                frame.weather_time,
                                &self.indices,
                            )
                    }
                    _ => None,
                };
                post.set_weather(weather);
                let flat = self.flat.as_ref().filter(|_| flat_frame);
                let pages: Vec<(u32, wgpu::Buffer)> = match flat {
                    Some(_) if pack_draws_terrain || pack_draws_shadow => {
                        self.arena.page_buffers().map(|(p, b)| (p, b.clone())).collect()
                    }
                    _ => Vec::new(),
                };
                let mut terrain = |
                    rp: &mut wgpu::RenderPass<'_>,
                    list: pack_post::TerrainList,
                    page: &dyn Fn(&mut wgpu::RenderPass<'_>, u32)|
                {
                    use pack_post::TerrainList as L;
                    let shadow = matches!(list, L::ShadowSolid | L::ShadowTranslucent);
                    if !(if shadow { pack_draws_shadow } else { pack_draws_terrain }) {
                        return;
                    }
                    match flat {
                        Some(flat) => {
                            let [shadow, shadow_translucent] = flat.shadow_calls;
                            let calls = match list {
                                L::Solid => &flat.calls[..flat.solid_calls],
                                L::Translucent => flat.translucent_calls(),
                                L::ShadowSolid => &flat.calls[shadow..shadow_translucent],
                                L::ShadowTranslucent => &flat.calls[shadow_translucent..],
                            };
                            flat.draw_with(rp, calls, |p| self.arena.buffer(p), page);
                        }
                        None => {
                            rp.set_index_buffer(
                                self.indices.slice(..),
                                wgpu::IndexFormat::Uint32,
                            );
                            rp.set_vertex_buffer(1, self.instances.slice(..));
                            let (first, count) = match list {
                                L::Solid => (0, solid_draws),
                                L::Translucent => (solid_draws, translucent_draws),
                                L::ShadowSolid => (shadow_first, shadow_solid),
                                L::ShadowTranslucent => {
                                    (shadow_first + shadow_solid, shadow_translucent)
                                }
                            };
                            self.draw_sections(rp, &self.indirect, 0, first, count);
                        }
                    }
                };
                post.set_clouds(
                    pack_draws_clouds
                        .then(|| pack_post::Clouds {
                            vertices: self.cloud_buffer.clone(),
                            group: self.cloud_group.clone(),
                            indices: self.indices.clone(),
                            quads: cloud_quads,
                        }),
                );
                post.run(
                    &self.device,
                    &self.queue,
                    &mut encoder,
                    color,
                    depth,
                    &view_target,
                    frame,
                    &frame.pack,
                    size,
                    [self.config.width, self.config.height],
                    &pages,
                    &mut terrain,
                );
                self.pack_post = Some((dimension, Some(post)));
            } else if let Some((color, depth)) = &scaled && native_post
                && let Some(native) = &mut self.native
                && let Some(post) = &mut native.post
            {
                let size = self.world_post.size().unwrap_or([1, 1]);
                post.run(
                    &self.device,
                    &mut encoder,
                    (color, depth),
                    &view_target,
                    size,
                    native.timing.as_mut(),
                );
            } else if scaled.is_some() {
                self.world_post.finish(&self.queue, &mut encoder, &view_target);
            }
        }
        if let Some(hdr) = &self.hdr {
            hdr.keep_world(&mut encoder);
        }
        if hand_quads > 0 && !pack_draws_hand {
            let attachments = [
                Some(wgpu::RenderPassColorAttachment {
                    view: &view_target,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                }),
            ];
            let mut pass = encoder
                .begin_render_pass(
                    &wgpu::RenderPassDescriptor {
                        label: Some("First-person hand"),
                        color_attachments: &attachments,
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view: &self.depth,
                            depth_ops: Some(wgpu::Operations {
                                load: wgpu::LoadOp::Clear(0.0),
                                store: wgpu::StoreOp::Discard,
                            }),
                            stencil_ops: None,
                        }),
                        ..Default::default()
                    },
                );
            pass.set_pipeline(&self.entity_pipeline);
            pass.set_bind_group(0, &self.hand_group, &[]);
            pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.set_vertex_buffer(0, self.entity_buffer.slice(..));
            pass.draw_indexed(
                0..hand_quads * 6,
                ((world_quads + additive_quads) * 4) as i32,
                0..1,
            );
            drop(pass);
            if let Some(hdr) = &self.hdr {
                hdr.mark_hand(&mut encoder);
            }
        }
        if !path_frame {
            self.entity_outline
                .draw(&mut encoder, &view_target, &self.entity_group, &self.indices);
        }
        let native_pt_capture = if path_frame
            && crate::native_pt_config::snapshot().denoise
            && self.screenshot.as_ref().is_some_and(|request| request.size.is_none())
        {
            if state.pending.is_none() {
                state
                    .tracer
                    .as_mut()
                    .and_then(|tracer| tracer.capture(&self.device, &mut encoder))
                    .and_then(|capture| {
                        self.screenshot.take().map(|request| (request, capture))
                    })
            } else {
                self.screenshot.take();
                self.screenshot_result = Some(
                    Err("PT screenshot is still processing".into()),
                );
                None
            }
        } else {
            None
        };
        let readback = self
            .screenshot
            .take_if(|request| request.size.is_some())
            .and_then(|request| {
                self.take_readback(request, &mut encoder, &target_texture)
            });
        if !frame.title && !frame.post_effects.effects.is_empty()
            && target_texture.usage().contains(wgpu::TextureUsages::COPY_SRC)
        {
            let device = &self.device;
            self.post_effects
                .get_or_insert_with(|| crate::post_chain::PostEffects::new(device))
                .apply(
                    &self.device,
                    &self.queue,
                    &mut encoder,
                    &target_texture,
                    &view_target,
                    [self.config.width, self.config.height],
                    scaled.is_none().then_some(&self.depth),
                    &frame.post_effects,
                );
        }
        if !frame.screen.is_empty() {
            let attachments = [
                Some(wgpu::RenderPassColorAttachment {
                    view: &view_target,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                }),
            ];
            let mut pass = encoder
                .begin_render_pass(
                    &wgpu::RenderPassDescriptor {
                        label: Some("Screen effects"),
                        color_attachments: &attachments,
                        ..Default::default()
                    },
                );
            pass.set_bind_group(0, &self.entity_group, &[]);
            pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.set_vertex_buffer(0, self.screen_buffer.slice(..));
            for (i, quad) in frame.screen.iter().enumerate() {
                let blend = match quad.blend {
                    crate::screen_effects::Blend::Alpha => 0,
                    crate::screen_effects::Blend::Vignette => 4,
                };
                pass.set_pipeline(
                    &self.screen_pipelines[blend + (quad.sampling & 3) as usize],
                );
                pass.draw_indexed(0..6, (i * 4) as i32, 0..1);
            }
        }
        if crosshair > 0 {
            let attachments = [
                Some(wgpu::RenderPassColorAttachment {
                    view: &view_target,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                }),
            ];
            let mut pass = encoder
                .begin_render_pass(
                    &wgpu::RenderPassDescriptor {
                        label: Some("3d crosshair"),
                        color_attachments: &attachments,
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view: &self.depth,
                            depth_ops: Some(wgpu::Operations {
                                load: wgpu::LoadOp::Clear(0.0),
                                store: wgpu::StoreOp::Discard,
                            }),
                            stencil_ops: None,
                        }),
                        ..Default::default()
                    },
                );
            pass.set_pipeline(&self.debug_lines_on_top);
            pass.set_bind_group(0, &self.crosshair_group, &[]);
            pass.set_vertex_buffer(0, self.debug_line_buffer.slice(..));
            let first = debug_depth + debug_top;
            pass.draw(0..4, first..first + crosshair);
        }
        let menu_blur = frame
            .menu_blur
            .filter(|_| self.config.usage.contains(wgpu::TextureUsages::COPY_SRC));
        let blur_at = menu_blur.map_or(0, |_| frame.hud.blur_at);
        self.hud
            .draw_range(
                &mut encoder,
                &view_target,
                &self.depth,
                frame.hud,
                [self.config.width, self.config.height],
                0..blur_at,
            );
        if let Some(radius) = menu_blur {
            let format = self.config.format;
            let device = &self.device;
            self.menu_blur
                .get_or_insert_with(|| crate::menu_blur::MenuBlur::new(device, format))
                .apply(
                    &self.device,
                    &self.queue,
                    &mut encoder,
                    &target_texture,
                    &view_target,
                    [self.config.width, self.config.height],
                    radius,
                );
        }
        self.hud
            .draw_range(
                &mut encoder,
                &view_target,
                &self.depth,
                frame.hud,
                [self.config.width, self.config.height],
                blur_at..frame.hud.vertices.len() as u32,
            );
        if culling && let Some(cull) = &mut self.cull {
            cull.end(&self.device, &mut encoder);
        }
        if let Some(timer) = &mut self.gpu_timer {
            timer
                .end(
                    &self.device,
                    &mut encoder,
                    (&view_target, self.config.format.remove_srgb_suffix()),
                );
        }
        let readback = readback
            .or_else(|| {
                self.screenshot
                    .take()
                    .and_then(|request| {
                        self.take_readback(request, &mut encoder, &target_texture)
                    })
            });
        self.uploads.finish(&encoder);
        if has_section_copies {
            self.section_copy_bytes = 0;
            self.section_uploads.finish(&encoder);
        }
        if let Some(f) = &surface_frame {
            self.readback.present(&self.device, &mut encoder, &f.texture);
        }
        if let (Some(hdr), Some(f)) = (&self.hdr, &surface_frame) {
            hdr.present(&self.queue, &mut encoder, &f.texture, !frame.title);
        }
        let hdr_readback = readback
            .as_ref()
            .filter(|(request, _)| request.size.is_none())
            .zip(self.hdr.as_ref())
            .and_then(|(_, hdr)| {
                let texture = hdr
                    .capture(&self.device, &self.queue, &mut encoder, !frame.title)?;
                let size = [self.config.width, self.config.height];
                Some(copy_frame(&self.device, &mut encoder, &texture, size))
            });
        let commands = crate::spans::time("encode", || encoder.finish());
        let first_frame = self.first_frame == FirstFrame::Submit;
        if first_frame {
            startup_step(format_args!("submitting the first frame"));
            self.first_frame = FirstFrame::Drawn;
        }
        let submission = crate::spans::time("submit", || self.queue.submit([commands]));
        if let Some((request, capture)) = native_pt_capture {
            state.begin_capture(self, request, capture);
        }
        if let Some(cull) = &mut self.cull {
            cull.submitted();
        }
        if let Some(timer) = &mut self.gpu_timer {
            timer.submitted(&self.queue);
            timer.collect(&self.device, &mut self.gpu_times);
        }
        if let Some(Some(timer)) = Self::pass_timer(
            &mut self.pack_post,
            &mut self.native,
        ) {
            timer.submitted(&self.queue);
        }
        if let Some((request, buffer)) = readback {
            if let Some(hdr_buffer) = hdr_readback {
                let hdr_path = hdr_screenshot_path(&request.path);
                match self.write_hdr_screenshot(&hdr_path, &hdr_buffer) {
                    Ok(()) => {
                        log::info!("Saved HDR screenshot to {}", hdr_path.display())
                    }
                    Err(e) => {
                        log::warn!(
                            "Horizon renderer: HDR screenshot {}: {e}", hdr_path
                            .display()
                        )
                    }
                }
            }
            self.screenshot_result = Some(
                self.write_screenshot(&request, &buffer).map_err(|e| e.to_string()),
            );
        }
        let present = Instant::now();
        if let Some(surface_frame) = surface_frame {
            if self.config.present_mode == wgpu::PresentMode::Fifo {
                self.window.pre_present_notify();
            }
            if first_frame {
                startup_step(format_args!("presenting the first frame"));
            }
            self.queue.present(surface_frame);
            if self.hdr.is_some() && let Some(marker) = self.hdr_marker.take() {
                let _ = std::fs::remove_file(marker);
                log::info!("Horizon renderer: HDR output presented its first frame");
            }
            if reconfigure && let Some(surface) = &self.surface {
                Self::configure(
                    &self.device,
                    surface,
                    &self.surface_config(),
                    self.hdr.as_ref(),
                );
            }
        } else {
            self.in_flight.push_back(submission);
            while self.in_flight.len() > 2 {
                let index = self.in_flight.pop_front();
                let _ = self
                    .device
                    .poll(wgpu::PollType::Wait {
                        submission_index: index,
                        timeout: Some(std::time::Duration::from_secs(5)),
                    });
            }
        }
        self.last_wait += present.elapsed();
        self.last_encode = started.elapsed().saturating_sub(self.last_wait);
        let _ = self.started;
        self.visible = visible;
        self.visible_lods = visible_lods;
        self.shadow_visible = shadow_visible;
        Ok(true)
    }
    /// Feed the independent ray scene from client chunk storage, without consuming the mesher's
    /// dirty queue. The experimental mode is deliberately separate from shader packs.
    fn native_pt_sync_inner(
        &mut self,
        graphics: &crate::native_pt_config::PtOptions,
        world_ready: bool,
        world: &crate::world::World,
        frame: &Frame,
        interacting: bool,
        assets: &crate::assets::Assets,
        render: &Arc<crate::assets::RenderAssets>,
        facts: &horizon_items::blocks::BlockFacts,
        state: &mut State,
    ) {
        let enabled = graphics.enabled && world_ready;
        if state.enabled != enabled {
            if let Some(tracer) = &mut state.tracer {
                tracer.invalidate();
            }
            log::info!("Native path tracing {}", if enabled { "on" } else { "off" });
        }
        state.enabled = enabled;
        if state.failed || !enabled {
            return;
        }
        if state.tracer.is_none() {
            let started = std::time::Instant::now();
            match path_tracer::PathTracer::new(
                &self.device,
                &state.sprites,
                self.terrain_sprites[1] as u32,
                self.config.format.remove_srgb_suffix(),
            ) {
                Ok(tracer) => {
                    log::info!(
                        "Experimental native path tracer ready in {:.1} ms: 128-block scene, progressive transport",
                        started.elapsed().as_secs_f64() * 1000.0
                    );
                    state.tracer = Some(tracer);
                }
                Err(error) => {
                    state.failed = true;
                    log::error!(
                        "Path tracer unavailable; retaining raster renderer: {error}"
                    );
                }
            }
        }
        if let Some(tracer) = &mut state.tracer {
            tracer
                .configure(
                    graphics.scale,
                    graphics.bounces,
                    graphics.freeze,
                    graphics.exposure,
                );
            tracer
                .prepare_scene(
                    frame,
                    [self.config.width, self.config.height],
                    interacting,
                );
            tracer
                .sync(
                    &self.device,
                    &self.queue,
                    world,
                    frame.camera,
                    assets,
                    render,
                    facts,
                    &state.sprites,
                );
        }
    }
}
