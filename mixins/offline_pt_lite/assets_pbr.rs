use crate::{sprite_cache::Decoded, sprites::Sprites};
use std::{
    collections::{BTreeMap, hash_map::DefaultHasher},
    fs, hash::{Hash, Hasher},
    path::Path,
};
const CACHE_FILE: &str = "horizon-pbr.cache";
const CACHE_VERSION: u32 = 3;
const MAX_BYTES: usize = 768 * 1024 * 1024;
pub(crate) fn has_maps(root: &Path, name: &str) -> bool {
    ["_n", "_s"]
        .iter()
        .any(|suffix| {
            crate::sprites::native_pt_assets::resource_path(
                    root,
                    &format!("{name}{suffix}"),
                )
                .is_some_and(|p| p.is_file())
        })
}
#[derive(Default)]
pub struct PbrTextures {
    /// Four header words per albedo layer: offset, side, presence flags, reserved.
    /// Texels follow, packed as normal R/G and specular R/G, all linear UNORM8.
    /// Flag 4 adds a packed emission byte plane after the material texels.
    /// Flags 8..15 contain its peak. Alpha 255 is the labPBR non-emissive sentinel.
    pub words: Vec<u32>,
    pub normal_count: usize,
    pub specular_count: usize,
}
fn read(root: &Path, name: &str, suffix: &str) -> Option<Decoded> {
    let id = format!("{name}{suffix}");
    if !crate::sprites::native_pt_assets::resource_path(root, &id)?.is_file() {
        return None;
    }
    match Sprites::native_pt_decode(root, &id) {
        Ok(map) if map.frames.len() == 1 => Some(map),
        Ok(_) => {
            log::warn!(
                "labPBR: animated sidecar {id} is not supported; using authored defaults"
            );
            None
        }
        Err(e) => {
            log::warn!("labPBR: invalid sidecar {id}: {e}; using authored defaults");
            None
        }
    }
}
fn map_side(count: usize, max_side: u32) -> u32 {
    let mut side = max_side;
    while side > 1 && count * 16 + count * (side * side) as usize * 5 > MAX_BYTES {
        side /= 2;
    }
    side
}
impl PbrTextures {
    pub(crate) fn emission_peak(&self, layer: u32) -> f32 {
        self.words
            .get(layer as usize * 4 + 2)
            .map_or(0., |flags| ((flags >> 8) & 255) as f32 / 254.)
    }
    pub fn load(root: &Path, layers: &BTreeMap<String, u32>, max_side: u32) -> Self {
        Self::load_cached(root, layers, max_side, None)
    }
    pub(crate) fn load_cached(
        root: &Path,
        layers: &BTreeMap<String, u32>,
        max_side: u32,
        content_fingerprint: Option<u64>,
    ) -> Self {
        let started = std::time::Instant::now();
        let count = layers.values().copied().max().map_or(0, |n| n as usize + 1);
        let budget_side = map_side(count, max_side);
        let key = content_fingerprint
            .map(|fingerprint| {
                let mut hash = DefaultHasher::new();
                (CACHE_VERSION, fingerprint, layers, max_side).hash(&mut hash);
                hash.finish()
            });
        if let Some(cached) = key
            .and_then(|key| Self::read_cache(root, key, count, max_side))
        {
            log::info!(
                "labPBR: cached material maps ready in {:.2}s ({}px budget side)",
                started.elapsed().as_secs_f64(), budget_side
            );
            return cached;
        }
        let mut result = Self {
            words: vec![0; count * 4],
            ..Default::default()
        };
        let map_side = budget_side;
        let entries: Vec<_> = layers.iter().collect();
        let tiles = crate::par::par_map(
            &entries,
            |(name, _)| {
                let normal = read(root, name, "_n");
                let specular = read(root, name, "_s");
                let side = normal
                    .iter()
                    .chain(&specular)
                    .map(|m| m.side)
                    .max()?
                    .min(map_side);
                let mut flags = u32::from(normal.is_some())
                    | (u32::from(specular.is_some()) << 1);
                let pixel = |map: &Option<Decoded>, x: u32, y: u32, default: [u8; 4]| {
                    map.as_ref()
                        .map_or(
                            default,
                            |m| {
                                let offset = ((y * m.side / side * m.side
                                    + x * m.side / side) * 4) as usize;
                                m.frames[0][offset..offset + 4].try_into().unwrap()
                            },
                        )
                };
                let mut emission = Vec::with_capacity((side * side) as usize);
                let mut has_emission = false;
                let mut peak = 0;
                let mut words = (0..side * side)
                    .map(|i| {
                        let n = pixel(&normal, i % side, i / side, [128, 128, 255, 255]);
                        let s = pixel(&specular, i % side, i / side, [0, 10, 0, 255]);
                        has_emission |= s[3] != 255;
                        let value = if s[3] == 255 { 0 } else { s[3] };
                        peak = peak.max(value);
                        emission.push(value);
                        u32::from_le_bytes([n[0], n[1], s[0], s[1]])
                    })
                    .collect::<Vec<_>>();
                if has_emission {
                    flags |= 4 | (u32::from(peak) << 8);
                    emission.resize(emission.len().next_multiple_of(4), 0);
                    words
                        .extend(
                            emission
                                .as_chunks::<4>()
                                .0
                                .iter()
                                .map(|bytes| u32::from_le_bytes(*bytes)),
                        );
                }
                Some((side, flags, words))
            },
        );
        for ((name, &layer), tile) in entries.into_iter().zip(tiles) {
            let Some((side, flags, words)) = tile else {
                continue;
            };
            if (result.words.len() + words.len()) * 4 > MAX_BYTES {
                log::warn!("labPBR: material data exceeds 768 MiB; skipping {name}");
                continue;
            }
            let header = layer as usize * 4;
            let offset = result.words.len() as u32;
            result.words[header..header + 4].copy_from_slice(&[offset, side, flags, 0]);
            result.words.extend(words);
            result.normal_count += usize::from(flags & 1 != 0);
            result.specular_count += usize::from(flags & 2 != 0);
        }
        if let Some(key) = key {
            result.save_cache(root, key);
        }
        log::info!(
            "labPBR: decoded material maps ready in {:.2}s ({}px budget side)", started
            .elapsed().as_secs_f64(), budget_side
        );
        result
    }
    fn read_cache(root: &Path, key: u64, count: usize, max_side: u32) -> Option<Self> {
        let path = root.join(CACHE_FILE);
        if fs::metadata(&path).ok()?.len() > MAX_BYTES as u64 + 32 {
            return None;
        }
        let bytes = fs::read(path).ok()?;
        let word = |offset: usize| {
            Some(u32::from_le_bytes(bytes.get(offset..offset + 4)?.try_into().ok()?))
        };
        if bytes.get(..4)? != b"HPBR" || word(4)? != CACHE_VERSION
            || u64::from_le_bytes(bytes.get(8..16)?.try_into().ok()?) != key
        {
            return None;
        }
        let normal_count = word(16)? as usize;
        let specular_count = word(20)? as usize;
        let len = word(24)? as usize;
        if normal_count > count || specular_count > count || len < count.checked_mul(4)?
            || len > MAX_BYTES / 4 || bytes.len() != 28 + len * 4
        {
            return None;
        }
        let words: Vec<_> = bytes[28..]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| u32::from_le_bytes(*b))
            .collect();
        let mut normals = 0;
        let mut speculars = 0;
        for header in words[..count * 4].as_chunks::<4>().0 {
            let [offset, side, flags, reserved] = *header;
            if flags == 0 {
                if *header != [0; 4] {
                    return None;
                }
                continue;
            }
            let texels = (side as usize).checked_mul(side as usize)?;
            let length = texels
                .checked_add(if flags & 4 != 0 { texels.div_ceil(4) } else { 0 })?;
            if flags & !0xff07 != 0 || (flags & 4 == 0 && flags >> 8 != 0)
                || (flags & 4 != 0 && (flags & 2 == 0 || flags >> 8 > 254))
                || reserved != 0 || side == 0 || side > max_side
                || (offset as usize) < count * 4
                || (offset as usize).checked_add(length)? > len
            {
                return None;
            }
            normals += usize::from(flags & 1 != 0);
            speculars += usize::from(flags & 2 != 0);
        }
        (normals == normal_count && speculars == specular_count)
            .then_some(Self {
                words,
                normal_count,
                specular_count,
            })
    }
    fn save_cache(&self, root: &Path, key: u64) {
        let mut bytes = Vec::with_capacity(28 + self.words.len() * 4);
        bytes.extend_from_slice(b"HPBR");
        bytes.extend_from_slice(&CACHE_VERSION.to_le_bytes());
        bytes.extend_from_slice(&key.to_le_bytes());
        bytes.extend_from_slice(&(self.normal_count as u32).to_le_bytes());
        bytes.extend_from_slice(&(self.specular_count as u32).to_le_bytes());
        bytes.extend_from_slice(&(self.words.len() as u32).to_le_bytes());
        for word in &self.words {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        let path = root.join(CACHE_FILE);
        let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
        if fs::write(&temporary, bytes).is_ok() && fs::rename(&temporary, path).is_err()
        {
            let _ = fs::remove_file(temporary);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, io::Write};
    fn test_root(label: &str) -> std::path::PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir()
            .join(format!("horizon-pbr-{label}-{}-{nonce}", std::process::id()))
    }
    fn png(root: &Path, name: &str, width: u32, height: u32, pixels: &[u8]) {
        let path = root.join(format!("assets/test/textures/block/{name}.png"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header().unwrap().write_image_data(pixels).unwrap();
        fs::File::create(path).unwrap().write_all(&bytes).unwrap();
    }
    #[test]
    fn connected_texture_pbr_keeps_128px_maps() {
        assert_eq!(map_side(9_455, 128), 128);
    }
    #[test]
    fn emission_plane_preserves_masks_and_the_255_sentinel() {
        let root = test_root("emission");
        fs::create_dir_all(&root).unwrap();
        let pixels: Vec<_> = [0, 64, 254, 255]
            .into_iter()
            .flat_map(|alpha| [200, 10, 0, alpha])
            .collect();
        png(&root, "lamp_s", 2, 2, &pixels);
        png(&root, "unused_s", 1, 1, &[0, 10, 0, 255]);
        png(&root, "dark_s", 1, 1, &[0, 10, 0, 0]);
        let layers = BTreeMap::from([
            ("test:block/lamp".to_owned(), 0),
            ("test:block/unused".to_owned(), 1),
            ("test:block/dark".to_owned(), 2),
        ]);
        let maps = PbrTextures::load_cached(&root, &layers, 2, Some(99));
        assert_eq!(maps.words[2], 6 | (254 << 8));
        let offset = maps.words[0] as usize;
        assert_eq!(maps.words[offset + 4].to_le_bytes(), [0, 64, 254, 0]);
        assert_eq!(maps.emission_peak(0), 1.);
        assert_eq!(maps.words[6], 2, "unused alpha retains authored emission");
        assert_eq!(maps.words[10], 6, "an explicit black map disables emission");
        let dark = maps.words[8] as usize;
        assert_eq!(maps.words[dark + 1], 0);
        assert_eq!(
            PbrTextures::load_cached(& root, & layers, 2, Some(99)).words, maps.words,
            "cached emission must retain the byte plane"
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn material_cache_invalidates_for_content_layers_and_resolution() {
        let root = test_root("cache");
        fs::create_dir_all(&root).unwrap();
        png(&root, "a_s", 2, 2, &[200, 10, 0, 255].repeat(4));
        let layers = BTreeMap::from([("test:block/a".to_owned(), 0)]);
        let first = PbrTextures::load_cached(&root, &layers, 1, Some(10));
        let bytes = fs::read(root.join(CACHE_FILE)).unwrap();
        let key = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
        assert_eq!(
            PbrTextures::read_cache(& root, key, 1, 1).unwrap().words, first.words
        );
        assert!(PbrTextures::read_cache(& root, key + 1, 1, 1).is_none());
        png(&root, "a_s", 2, 2, &[250, 10, 0, 255].repeat(4));
        assert_eq!(
            PbrTextures::load_cached(& root, & layers, 1, Some(10)).words, first.words
        );
        let changed = PbrTextures::load_cached(&root, &layers, 1, Some(11));
        assert_ne!(changed.words, first.words);
        let larger = PbrTextures::load_cached(&root, &layers, 2, Some(11));
        assert_eq!(larger.words[1], 2);
        let remapped = BTreeMap::from([("test:block/a".to_owned(), 2)]);
        let moved = PbrTextures::load_cached(&root, &remapped, 2, Some(11));
        assert_eq!(& moved.words[..8], & [0; 8]);
        assert_eq!(& moved.words[8..12], & [12, 2, 2, 0]);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn malformed_material_cache_falls_back_to_the_pngs() {
        let root = test_root("corrupt");
        fs::create_dir_all(&root).unwrap();
        png(&root, "a_n", 1, 1, &[180, 128, 255, 255]);
        let layers = BTreeMap::from([("test:block/a".to_owned(), 0)]);
        let expected = PbrTextures::load_cached(&root, &layers, 1, Some(10));
        let path = root.join(CACHE_FILE);
        let valid = fs::read(&path).unwrap();
        let key = u64::from_le_bytes(valid[8..16].try_into().unwrap());
        for (offset, value) in [
            (4, 99),
            (16, 2),
            (24, u32::MAX),
            (28, u32::MAX),
            (32, 0),
            (36, 4),
        ] {
            let mut damaged = valid.clone();
            damaged[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            fs::write(&path, damaged).unwrap();
            assert!(
                PbrTextures::read_cache(& root, key, 1, 1).is_none(), "offset {offset}"
            );
            assert_eq!(
                PbrTextures::load_cached(& root, & layers, 1, Some(10)).words, expected
                .words
            );
        }
        fs::write(&path, &valid[..valid.len() - 1]).unwrap();
        assert!(PbrTextures::read_cache(& root, key, 1, 1).is_none());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn ctm_tiles_and_sidecars_use_the_same_resource_identity() {
        let root = test_root("ctm");
        let id = "test:optifine/ctm/wall/1";
        for (suffix, rgba) in [
            ("", [80, 90, 100, 255]),
            ("_n", [180, 128, 255, 255]),
            ("_s", [220, 10, 0, 255]),
        ] {
            let path = crate::sprites::native_pt_assets::resource_path(
                    &root,
                    &format!("{id}{suffix}"),
                )
                .unwrap();
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let mut encoder = png::Encoder::new(fs::File::create(path).unwrap(), 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.write_header().unwrap().write_image_data(&rgba).unwrap();
        }
        let mut sprites = Sprites::default();
        let layer = sprites.insert(&root, id).unwrap();
        sprites.finish();
        let maps = PbrTextures::load(&root, &sprites.layers, sprites.size);
        let header = layer as usize * 4;
        assert_eq!(maps.words[header + 2], 3);
        assert_eq!(
            maps.words[maps.words[header] as usize].to_le_bytes(), [180, 128, 220, 10]
        );
        assert_eq!(& sprites.pixels[..4], & [80, 90, 100, 255]);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn equal_albedo_keeps_distinct_materials_and_static_maps_beside_animation() {
        let root = test_root("materials");
        fs::create_dir_all(&root).unwrap();
        for name in ["a", "b", "c"] {
            png(&root, name, 1, 1, &[80, 90, 100, 255]);
        }
        png(&root, "a_n", 1, 1, &[200, 128, 255, 255]);
        png(&root, "a_s", 1, 1, &[128, 10, 0, 255]);
        png(&root, "b_s", 1, 1, &[250, 230, 0, 255]);
        png(&root, "animated", 1, 2, &[80, 90, 100, 255, 20, 30, 40, 255]);
        fs::write(
                root.join("assets/test/textures/block/animated.png.mcmeta"),
                r#"{"animation":{"frametime":2}}"#,
            )
            .unwrap();
        png(&root, "animated_n", 1, 1, &[128, 200, 255, 255]);
        let mut sprites = Sprites::default();
        let a = sprites.insert(&root, "test:block/a").unwrap();
        let b = sprites.insert(&root, "test:block/b").unwrap();
        let c = sprites.insert(&root, "test:block/c").unwrap();
        let animated = sprites.insert(&root, "test:block/animated").unwrap();
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_ne!(b, c);
        sprites.finish();
        let maps = PbrTextures::load(&root, &sprites.layers, sprites.size);
        assert_eq!((maps.normal_count, maps.specular_count), (2, 2));
        let sample = |layer: u32| {
            let header = layer as usize * 4;
            let offset = maps.words[header] as usize;
            (maps.words[header + 2], maps.words[offset].to_le_bytes())
        };
        assert_eq!(sample(a), (3, [200, 128, 128, 10]));
        assert_eq!(sample(b), (2, [128, 128, 250, 230]));
        assert_eq!(sample(animated), (1, [128, 200, 0, 10]));
        assert_eq!(maps.words[c as usize * 4 + 2], 0);
        fs::remove_dir_all(root).unwrap();
    }
}
