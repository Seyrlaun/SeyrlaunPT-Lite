use super::*;
impl RenderAssets {
    pub(super) fn native_pt_load(root: &Path, assets: &Assets) -> Result<Self> {
        let registry = assets.registry;
        let mut sprites = crate::sprites::Sprites::default();
        for name in MATERIALS {
            sprites.insert(root, &format!("minecraft:block/{name}"))?;
        }
        let parsed = crate::model_library::ParsedModels::load(root);
        let models = crate::models::prepare_with_models(
            root,
            registry,
            &mut sprites,
            &parsed,
        )?;
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
        let ctm = crate::ctm::ConnectedTextures::load(
            root,
            registry,
            &models.catalog,
            &mut sprites,
        );
        sprites.finish();
        let pbr = crate::pbr::PbrTextures::load_cached(
            root,
            &sprites.layers,
            sprites.size,
            crate::sprites::native_pt_assets::native_pt_fingerprint(root),
        );
        sprites.pixels.shrink_to_fit();
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
