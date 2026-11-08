use super::*;
impl RenderAssets {
    pub(super) fn native_pt_load_with(
        root: &Path,
        assets: &Assets,
        draws: bool,
    ) -> Result<Self> {
        let registry = assets.registry;
        let mut sprites = crate::sprites::Sprites::with_disk_cache(root);
        for name in MATERIALS {
            sprites.insert(root, &format!("minecraft:block/{name}"))?;
        }
        let parsed = crate::model_library::ParsedModels::load(root);
        let models = if draws {
            crate::models::prepare_with_models(root, registry, &mut sprites, &parsed)?
        } else {
            crate::models::PreparedModels::default()
        };
        let model_library = parsed.into_item_library();
        for name in [
            "water_still",
            "water_flow",
            "water_overlay",
            "lava_still",
            "lava_flow",
        ] {
            sprites.insert(root, &format!("minecraft:block/{name}"))?;
        }
        for stage in 0..10 {
            sprites.insert(root, &format!("minecraft:block/destroy_stage_{stage}"))?;
        }
        let ctm = if draws {
            crate::ctm::ConnectedTextures::load(
                root,
                registry,
                &models.catalog,
                &mut sprites,
            )
        } else {
            crate::ctm::ConnectedTextures::default()
        };
        sprites.save_disk_cache();
        if draws {
            sprites.finish();
        } else {
            sprites.finish_without_pixels();
        }
        sprites.pixels.shrink_to_fit();
        let pbr = if draws {
            crate::pbr::PbrTextures::load_cached(
                root,
                &sprites.layers,
                sprites.size,
                sprites.content_fingerprint(),
            )
        } else {
            crate::pbr::PbrTextures::default()
        };
        log::info!(
            "Block textures: {}px, {} layers; labPBR: {} normal, {} specular maps ({} bytes), root {}",
            sprites.size, sprites.count(), pbr.normal_count, pbr.specular_count, pbr
            .words.len() * 4, root.display()
        );
        models
            .catalog
            .native_pt_set_extra(
                std::sync::Arc::new(crate::models::catalog::native_pt_assets::Extra {
                    ctm,
                    pbr,
                }),
            );
        Ok(Self {
            sprite_size: sprites.size,
            sprite_minimum_size: sprites.minimum_size,
            texture_layers: u32::try_from(sprites.count())?,
            pixels: sprites.pixels,
            model_catalog: models.catalog,
            model_library,
            model_placements: models.placements,
            model_gallery: models.states,
            animations: sprites.animations,
            sprite_layers: sprites.layers,
            translucent_layers: sprites.translucent,
            sprite_mips: sprites.mips,
        })
    }
}
