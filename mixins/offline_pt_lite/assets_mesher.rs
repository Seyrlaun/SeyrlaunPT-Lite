use super::*;
impl Mesher<'_> {
    pub(super) fn native_pt_mesh_reusing(
        &mut self,
        region: &[[Option<&Chunk>; 3]; 3],
        sy: i32,
        mut out: SectionMesh,
        scratch: &mut MeshScratch,
    ) -> SectionMesh {
        let native_pt_extra = self.render.model_catalog.native_pt_extra();
        out.clear();
        let center = region[1][1].expect("centre chunk");
        let section = &center.sections[sy as usize];
        if section.is_uniform(0) {
            return out;
        }
        let own = scratch.own.get_or_insert_with(|| Box::new([0; 4096]));
        scratch.padded.fill(region, sy, own);
        let pad = &scratch.padded;
        out.visibility = visibility_reusing(
            pad,
            &self.tables.info,
            &mut scratch.visibility,
        );
        let base_y = center.min_y() + sy * 16;
        let mut tints = Tints::new(
            region,
            self.biomes,
            self.zoom,
            self.blend_radius,
            [center.x, center.z, base_y],
        );
        let info = &self.tables.info;
        let catalog = &self.render.model_catalog;
        let mut models = Vec::new();
        for y in 0..16 {
            for z in 0..16 {
                for x in 0..16 {
                    let id = pad.blocks[pi(x, y, z)];
                    if id == 0 {
                        continue;
                    }
                    let state = &info[id as usize];
                    if state.block_entity {
                        out.block_entities
                            .push((
                                [center.x * 16 + x, base_y + y, center.z * 16 + z],
                                id,
                            ));
                    }
                    if state.solid
                        && Direction::ALL
                            .iter()
                            .all(|d| {
                                let [dx, dy, dz] = d.normal();
                                info[pad.blocks[pi(x + dx, y + dy, z + dz)] as usize].solid
                            })
                    {
                        continue;
                    }
                    if state.fluid.kind() != FluidKind::Empty {
                        let water = tints.get(Resolver::Water, x, y, z);
                        let nether = self.nether_shading;
                        fluid::tesselate(
                            self.tables,
                            nether,
                            pad,
                            water,
                            &mut out,
                            [x, y, z],
                        );
                    }
                    if !state.model {
                        continue;
                    }
                    let world = BlockPos::new(
                        center.x * 16 + x,
                        base_y + y,
                        center.z * 16 + z,
                    );
                    if catalog.models_at(BlockStateId(id), world, &mut models).is_err() {
                        continue;
                    }
                    let Some(first) = models.first() else {
                        continue;
                    };
                    let ao = self.smooth_lighting && state.emission == 0
                        && first.ambient_occlusion;
                    let origin = match offset(state, world.x, world.z) {
                        Some(o) => {
                            [
                                x as f32 + o[0] as f32,
                                y as f32 + o[1] as f32,
                                z as f32 + o[2] as f32,
                            ]
                        }
                        None => [x as f32, y as f32, z as f32],
                    };
                    let mut visible = [false; 6];
                    for d in Direction::ALL {
                        let [dx, dy, dz] = d.normal();
                        let neighbor = pad.blocks[pi(x + dx, y + dy, z + dz)];
                        let other = &info[neighbor as usize];
                        let opaque_leaves = !self.cutout_leaves && state.leaves
                            && other.leaves;
                        visible[d as usize] = !opaque_leaves
                            && self
                                .visibility
                                .should_render_face(
                                    BlockStateId(id),
                                    BlockStateId(neighbor),
                                    d,
                                    true,
                                )
                                .unwrap_or(true);
                    }
                    for model in &models {
                        for face in &model.faces {
                            if face.quad.cull.is_some_and(|d| !visible[d as usize]) {
                                continue;
                            }
                            self.native_pt_face(
                                pad,
                                &mut tints,
                                &mut out,
                                [x, y, z],
                                origin,
                                face,
                                native_pt_extra
                                    .ctm
                                    .select(
                                        BlockStateId(id),
                                        face.layer,
                                        face.quad.facing,
                                        [world.x, world.y, world.z],
                                        |[dx, dy, dz]| pad.blocks[pi(x + dx, y + dy, z + dz)],
                                    ),
                                ao,
                                state,
                            );
                        }
                    }
                }
            }
        }
        out
    }
    #[allow(clippy::too_many_arguments)]
    fn native_pt_face(
        &self,
        pad: &Padded,
        tints: &mut Tints,
        out: &mut SectionMesh,
        pos: [i32; 3],
        origin: [f32; 3],
        face: &BakedFace,
        layer: u32,
        ao: bool,
        state: &Info,
    ) {
        let q = &face.quad;
        let tier = state.tier.max(hdr_tier("", q.light_emission));
        let shade_face = q.shade_direction_override.unwrap_or(q.facing);
        let directional = lighting::cardinal(self.nether_shading, shade_face);
        let (gray, coords) = if ao {
            let info = &self.tables.info;
            let (brightness, coords) = lighting::ambient_occlusion(
                pad,
                info,
                state,
                pos,
                &q.positions,
                q.facing,
            );
            let gray = brightness
                .map(|b| lighting::scale(lighting::gray(b), directional));
            (gray, coords)
        } else {
            let coords = match q.cull {
                Some(d) => lighting::coords(pad, state, lighting::step(pos, d)),
                None => lighting::flat(pad, state, pos, &q.positions, q.facing),
            };
            ([lighting::gray(directional); 4], [coords; 4])
        };
        let tint = Self::tint(state, q.tint_index, tints, pos);
        let translucent = face.translucent
            || self.render.translucent_layers[layer as usize];
        let opaque = if layer == face.layer {
            face.opaque
        } else {
            !self.render.sprite_mips[layer as usize].has_transparent && !translucent
        };
        let target = if translucent { &mut out.translucent } else { &mut out.solid };
        for i in 0..4 {
            let g = gray[i] as u32;
            let rgb = match tint {
                Some(t) => [16, 8, 0].map(|shift| g * (t >> shift & 255) / 255),
                None => [g; 3],
            };
            let p = q.positions[i];
            target
                .push(
                    TerrainVertex::new(
                        [p[0] + origin[0], p[1] + origin[1], p[2] + origin[2]],
                        q.uv[i],
                        if opaque && !translucent || !self.cutout_leaves && state.leaves
                        {
                            layer as u16 | OPAQUE_LAYER
                        } else {
                            layer as u16
                        },
                        emitting(pack_rgb(rgb), tier),
                        pack_coords(lighting::with_emission(coords[i], q.light_emission)),
                    ),
                );
        }
    }
}
