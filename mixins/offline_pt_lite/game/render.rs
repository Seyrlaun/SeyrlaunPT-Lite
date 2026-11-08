use super::*;
impl Game<'_> {
    pub(super) fn native_pt_render(&mut self) -> Result<bool> {
        if let Some(loader) = &self.world_loader {
            if let Some(renderer) = &mut self.renderer {
                let mut hud = HudBatch::new([
                    renderer.config.width,
                    renderer.config.height,
                ]);
                horizon_client::level_loading::draw_progress(
                    &mut hud,
                    &self.gui.atlas,
                    if loader.cancelled { "Cancelling world" } else { "Opening world" },
                    0.0,
                );
                return renderer.render(&loading_frame(&hud));
            }
            return Ok(false);
        }
        if self.pending_entry.is_some() {
            if let (Some(renderer), Some(loader)) = (
                &mut self.renderer,
                &self.rest_loader,
            ) {
                let mut hud = HudBatch::new([
                    renderer.config.width,
                    renderer.config.height,
                ]);
                horizon_client::level_loading::draw_progress(
                    &mut hud,
                    &self.gui.atlas,
                    loader.progress.line(),
                    loader.progress.fraction(),
                );
                return renderer.render(&loading_frame(&hud));
            }
            return Ok(false);
        }
        if let Some(reload) = &self.server_packs.reload {
            if let Some(renderer) = &mut self.renderer {
                let mut hud = HudBatch::new([
                    renderer.config.width,
                    renderer.config.height,
                ]);
                horizon_client::level_loading::draw_progress(
                    &mut hud,
                    &self.gui.atlas,
                    reload.progress.line(),
                    reload.progress.fraction(),
                );
                return renderer.render(&loading_frame(&hud));
            }
            return Ok(false);
        }
        let Some(size) = self
            .renderer
            .as_ref()
            .map(|r| [r.config.width, r.config.height]) else {
            return Ok(false);
        };
        let alpha = self.clock.fraction();
        let camera_alpha = self.clock.camera_fraction();
        let view = self.camera_view(camera_alpha as f32);
        let (camera, underwater, view_yaw, view_pitch) = match view {
            Some((c, yaw, pitch)) => {
                let s = self
                    .world
                    .block(c.x.floor() as i32, c.y.floor() as i32, c.z.floor() as i32);
                (c, self.res.facts().get(s).fluid == Fluid::Water, yaw, pitch)
            }
            None => (glam::DVec3::new(0.5, 100.0, 0.5), false, self.yaw, self.pitch),
        };
        let (view_yaw, view_pitch) = self.shaken_view(view_yaw, view_pitch);
        self.audio.set_listener([camera.x, camera.y, camera.z], view_yaw, view_pitch);
        let block = [
            camera.x.floor() as i32,
            camera.y.floor() as i32,
            camera.z.floor() as i32,
        ];
        let cave_culling = self.settings.horizon.cave_culling
            && !self
                .res
                .facts()
                .get(self.world.block(block[0], block[1], block[2]))
                .solid_render;
        if cave_culling && self.title.is_none()
            && let Some(renderer) = self.renderer.as_mut()
        {
            horizon_client::spans::time(
                "occlusion",
                || {
                    renderer.start_occlusion(camera, (self.streaming.radius * 16) as f32)
                },
            );
        }
        let biome = self
            .world
            .chunks
            .get(&(block[0] >> 4, block[2] >> 4))
            .map(|c| c.biome(block[0], block[1], block[2]))
            .unwrap_or(0);
        let environment = self.dim_environment();
        let biomes = self.res.biomes();
        let weights = {
            let chunks = &self.world.chunks;
            let plains = biomes.id("minecraft:plains");
            horizon_client::biome::gaussian_weights(
                camera.to_array(),
                |qx, qy, qz| {
                    chunks
                        .get(&(qx >> 2, qz >> 2))
                        .map_or(plains, |c| c.biome(qx * 4, qy * 4, qz * 4))
                },
            )
        };
        let blend = |
            pick: fn(&horizon_client::biome::BiomeInfo) -> Option<u32>,
            base: [f32; 3]|
        {
            horizon_client::biome::blend_rgb(
                &weights,
                |b| { pick(biomes.get(b)).map_or(base, horizon_client::biome::rgb) },
            )
        };
        let biome_sky = blend(
            |b| b.sky,
            environment.base_rgb("minecraft:visual/sky_color"),
        );
        let biome_fog = blend(
            |b| b.fog,
            environment.base_rgb("minecraft:visual/fog_color"),
        );
        let water_fog = blend(|b| Some(b.water_fog), [0.0; 3]);
        let mut sky = environment
            .sample(self.world.day_time, Some(biome_sky), Some(biome_fog));
        let (rain, thunder) = if self.dim == Dim::Overworld {
            self.world.weather.levels(alpha as f32)
        } else {
            (0.0, 0.0)
        };
        horizon_client::environment::apply_weather(&mut sky, rain, thunder);
        self.end_flash_sky(&mut sky, alpha);
        if self.flash > 0 && !self.settings.file.bool("hideLightningFlashes") {
            let flash = [0.8, 0.8, 1.0];
            sky.sky_color = std::array::from_fn(|i| {
                sky.sky_color[i] + (flash[i] - sky.sky_color[i]) * 0.22
            });
            sky.sky_light_factor = 1.0;
        }
        let mut lines = self.fishing_line(alpha, size);
        let mut on_top = Vec::new();
        if self.show_hitboxes || self.show_chunk_borders {
            let g = self.debug_gizmos(alpha as f32);
            lines.extend(g.lines);
            on_top = g.on_top;
        }
        let view = horizon_client::camera::view_matrix(view_yaw, view_pitch);
        let at = camera.to_array();
        let mut crosshair = horizon_client::gizmos::Gizmos::default();
        if self.debug_crosshair() && self.camera_type.first_person() && !self.ui.hide_gui
        {
            let scale = HudBatch::new(size).scale;
            crosshair.debug_crosshair(at, view_pitch, view_yaw, scale);
        }
        let debug_lines = horizon_client::gizmos::camera_relative(&lines, at, view);
        let debug_lines_on_top = horizon_client::gizmos::camera_relative(
            &on_top,
            at,
            view,
        );
        let debug_crosshair = horizon_client::gizmos::camera_relative(
            &crosshair.on_top,
            at,
            view,
        );
        let mut outline_after_terrain = false;
        let outline = if self.screen == Screen::Playing && !self.ui.hide_gui
            && self.may_build()
        {
            self.pick()
                .map(|h| {
                    let state = self.world.block(h.pos[0], h.pos[1], h.pos[2]);
                    let f = self.res.facts().get(state);
                    outline_after_terrain = self
                        .pack
                        .render
                        .model_catalog
                        .collect(state, BlockPos::new(h.pos[0], h.pos[1], h.pos[2]))
                        .is_ok_and(|models| {
                            models
                                .iter()
                                .any(|(_, model)| model.faces.iter().any(|f| f.translucent))
                        });
                    let o = if f.outline_offset {
                        self.pack
                            .context
                            .tables
                            .offset(state, h.pos[0], h.pos[2])
                            .unwrap_or([0.0; 3])
                    } else {
                        [0.0; 3]
                    };
                    let p = h.pos.map(f64::from);
                    if let Some(b) = self.context_outline(h.pos, state) {
                        return horizon_client::blocks::cube_edges()
                            .iter()
                            .map(|e| {
                                std::array::from_fn(|i| {
                                    let k = i % 3;
                                    p[k] + b[k] + e[i] * (b[k + 3] - b[k])
                                })
                            })
                            .collect();
                    }
                    f.outline_edges
                        .iter()
                        .map(|e| std::array::from_fn(|i| p[i % 3] + o[i % 3] + e[i]))
                        .collect()
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let mut hud = horizon_client::spans::time("hud", || self.hud(size));
        let title = self.title.is_some()
            || self.loading_reason()
                == Some(horizon_client::level_loading::Reason::Other);
        let (yaw, pitch) = if title {
            (
                self
                    .panorama_rotation
                    .update(
                        self.started.elapsed().as_secs_f32(),
                        self.settings.file.double("panoramaScrollSpeed") as f32,
                    ),
                10.0,
            )
        } else {
            (view_yaw, view_pitch)
        };
        let fov_degrees = if title {
            85.0
        } else {
            self.fov_degrees(camera_alpha as f32, camera.to_array())
        };
        self.locator_camera = (!title)
            .then_some((camera.to_array(), view_yaw, view_pitch, fov_degrees));
        let bob = if title {
            glam::Mat4::IDENTITY
        } else {
            self.bob_matrix(camera_alpha as f32)
        };
        let spin = if title {
            glam::Mat4::IDENTITY
        } else {
            self.spinning_matrix(camera_alpha as f32)
        };
        let terrain_distance = self.terrain_distance();
        let distance = (terrain_distance * 16) as f32;
        let depth_far = horizon_client::camera::depth_far(
            terrain_distance,
            self.settings.cloud_range,
        );
        self.view_frustum = self
            .renderer
            .as_ref()
            .map(|r| r.camera_frustum(fov_degrees, yaw, pitch, bob * spin, depth_far));
        let mut entities = std::mem::take(&mut self.entity_vertices);
        entities.clear();
        self.see_through_vertices.clear();
        self.glowing_vertices.get_mut().clear();
        if !title {
            entities = horizon_client::spans::time(
                "entity_geometry",
                || { self.entity_geometry(camera, alpha, entities) },
            );
        }
        let bolts = if title { Vec::new() } else { self.bolt_instances() };
        let mut additive = Vec::new();
        if !title && self.dim == Dim::End {
            let mut dragon = Vec::new();
            self.dragon_geometry(camera, alpha, &mut dragon, &mut additive);
            self.dragon_fireball_geometry(camera, alpha, &mut dragon);
            entities.extend(dragon);
        }
        let (celestial, stars) = if title {
            (Vec::new(), None)
        } else if self.dim == Dim::Overworld
            && self.potions.effects.get("darkness").is_none()
            && self.potions.effects.get("blindness").is_none()
        {
            self.celestial_geometry(&sky, rain)
        } else if self.dim == Dim::End && self.settings.horizon.sky {
            (self.end_flash_geometry(), None)
        } else {
            (Vec::new(), None)
        };
        let hand = if title || self.scoping() {
            Vec::new()
        } else {
            self.hand_geometry(camera_alpha)
        };
        let screen = if title {
            Vec::new()
        } else {
            self.screen_quads(alpha as f32, size)
        };
        if !title {
            self.end_geometry(camera, &mut entities);
        }
        let weather = if !title && self.dim == Dim::Overworld
            && self.settings.horizon.weather
        {
            horizon_client::spans::time(
                "weather",
                || self.weather_geometry(camera, alpha),
            )
        } else {
            Vec::new()
        };
        let border = (!title)
            .then(|| self.border_draw(camera.to_array(), alpha as f32))
            .flatten();
        let see_through = std::mem::take(&mut self.see_through_vertices);
        if title {
            self.dynamic_lights.clear();
        } else {
            self.gather_dynamic_lights(camera, alpha);
        }
        let pack_inputs = if title {
            Default::default()
        } else {
            self.shader_pack_inputs(camera, alpha as f32)
        };
        hud.patches.append(&mut self.map_patches);
        let entity_spans = std::mem::take(self.entity_spans.get_mut());
        let glowing = std::mem::take(self.glowing_vertices.get_mut());
        let frame = Frame {
            weather: &weather,
            weather_time: horizon_client::weather::Time::new(
                self.world.game_time,
                alpha as f32,
            ),
            nether_lighting: self.dim == Dim::Nether,
            camera,
            yaw,
            pitch,
            fov_degrees,
            bob,
            spin,
            sky,
            distance,
            detail_distance: (self.streaming.radius * 16) as f32,
            depth_far,
            underwater,
            title,
            sky_blocked: self.potions.effects.get("blindness").is_some()
                || self.potions.effects.get("darkness").is_some()
                || matches!(
                    horizon_client::fog::fog_type(& self.world, self.res.facts(), [camera
                    .x, camera.y, camera.z], self.forward(),),
                    horizon_client::fog::FogType::Lava |
                    horizon_client::fog::FogType::PowderSnow
                ),
            skybox: self.dim == Dim::Overworld && self.settings.horizon.sky,
            end_sky: self.dim == Dim::End && self.settings.horizon.sky,
            end_sky_sprite: self
                .gui
                .atlas
                .sprites
                .get("environment/end_sky")
                .map(|r| [r.u0, r.v0, r.u1, r.v1]),
            environment_fog: (self.dim == Dim::Nether)
                .then_some((sky.fog_start, sky.fog_end)),
            cave_culling,
            water_fog,
            fog: Some({
                let mut fog = self.frame_fog(camera, &sky, biome, water_fog);
                if !self.settings.horizon.fog && !underwater
                    && self.darkness_fog().is_none()
                    && !self.boss_overlay.should_create_world_fog()
                    && self.potions.effects.get("blindness").is_none()
                    && fog.environmental_end >= (self.streaming.radius * 16) as f32
                {
                    (fog.render_distance_start, fog.render_distance_end) = (1e9, 2e9);
                    (fog.environmental_start, fog.environmental_end) = (1e9, 2e9);
                }
                fog
            }),
            dark_disc: self.dark_disc(camera),
            outline,
            outline_after_terrain,
            outline_high_contrast: self.settings.file.bool("highContrastBlockOutline"),
            debug_lines: &debug_lines,
            debug_lines_on_top: &debug_lines_on_top,
            debug_crosshair: &debug_crosshair,
            hud: &hud,
            glint_alpha: self.settings.file.double("glintStrength") as f32,
            glint_ticks: self.glint_ticks(),
            clouds: (!title && self.dim == Dim::Overworld
                && horizon_client::clouds::visible(
                    &self.settings.clouds,
                    sky.cloud_color[3],
                ))
                .then(|| self.gui.atlas.sprites.get("horizon/white"))
                .flatten()
                .map(|white| horizon_client::clouds::Input {
                    texture: &self.pack.clouds,
                    uv: [(white.u0 + white.u1) / 2.0, (white.v0 + white.v1) / 2.0],
                    camera,
                    time: self.world.game_time,
                    partial_tick: alpha,
                    range: self.settings.cloud_range,
                    fast: self.settings.clouds == "fast",
                    color: sky.cloud_color,
                }),
            entities: &entities,
            entity_spans: &entity_spans,
            glowing: &glowing,
            dynamic_lights: &self.dynamic_lights,
            lightning: horizon_client::gfx::lightning::Input {
                bolts: &bolts,
                uv: [self.gui.atlas.font.white().u0, self.gui.atlas.font.white().v0],
            },
            additive: &additive,
            see_through: &see_through,
            celestial: if self.settings.horizon.sky || title { &celestial } else { &[] },
            stars: stars.filter(|_| self.settings.horizon.sky || title),
            hand: &hand,
            hand_fov_degrees: self
                .hud_fov_degrees(camera_alpha as f32, camera.to_array()),
            screen: &screen,
            pack: pack_inputs,
            border,
            game_time: self.world.game_time as f32,
            shader_game_time: ((self.world.game_time % 24000) as f32 + alpha as f32)
                / 24000.0,
            crumbling_sprite: self
                .gui
                .atlas
                .sprites
                .get("block/destroy_stage_0")
                .map_or([0.0; 4], |r| [r.u0, r.v0, r.u1 - r.u0, r.v1 - r.v0]),
            end_portal_sprites: ["environment/end_sky", "entity/end_portal/end_portal"]
                .map(|n| {
                    self.gui
                        .atlas
                        .sprites
                        .get(n)
                        .map_or([0.0; 4], |r| [r.u0, r.v0, r.u1 - r.u0, r.v1 - r.v0])
                }),
            ticks: self.ticks,
            darkness: self.darkness_lighting(alpha as f32),
            night_vision: self.night_vision_intensity(alpha as f32),
            block_flicker: self.fog.block_light_flicker,
            boss_darken: self.boss_overlay.darken_amount(self.clock.fraction() as f32),
            menu_blur: self.menu_blur(),
        };
        let native_pt_options = horizon_client::native_pt_config::snapshot();
        let native_pt_world_ready = !title && self.loading_reason().is_none();
        let result = match self.renderer.as_mut() {
            Some(renderer) => {
                renderer
                    .sync_path_tracer(
                        &native_pt_options,
                        native_pt_world_ready,
                        &self.world,
                        &frame,
                        self.left || self.right || self.use_delay > 0,
                        &self.pack.assets,
                        &self.pack.render,
                        self.res.facts(),
                    );
                renderer.set_pass_timing(self.debug);
                horizon_client::spans::time("renderer", || renderer.render(&frame))
            }
            None => Err("renderer".into()),
        };
        self.entity_vertices = entities;
        *self.entity_spans.get_mut() = entity_spans;
        *self.glowing_vertices.get_mut() = glowing;
        self.see_through_vertices = see_through;
        result
    }
}
