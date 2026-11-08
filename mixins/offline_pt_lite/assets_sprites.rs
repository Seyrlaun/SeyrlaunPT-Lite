use super::*;
use crate::mipmap::Strategy;
use std::hash::{DefaultHasher, Hash, Hasher};
/// What decoding one texture gives, before it takes a layer.
#[derive(Clone)]
pub(crate) struct Decoded {
    pub side: u32,
    pub frames: Vec<Vec<u8>>,
    /// The `.mcmeta` `animation` value, as text.
    pub animation: Option<String>,
    pub strategy: Strategy,
    pub alpha_cutoff_bias: f32,
    pub has_transparent: bool,
    pub opaque: bool,
    pub translucent: bool,
}
/// CTM tiles live outside `textures/`, but otherwise use the same sprite and
/// material upload path. Keep their full resource identity for sidecar lookup.
pub(crate) fn resource_path(root: &Path, name: &str) -> Option<std::path::PathBuf> {
    let (namespace, path) = name.split_once(':')?;
    if namespace == "." || namespace == ".."
        || !namespace
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"_.-".contains(&c))
        || path.starts_with(['/', '\\']) || path.contains(':')
        || path.split(['/', '\\']).any(|part| part == "..")
    {
        return None;
    }
    let prefix = if path.starts_with("optifine/ctm/") { "" } else { "textures/" };
    Some(root.join(format!("assets/{namespace}/{prefix}{path}.png")))
}
/// Paths, sizes and modification times of every file under `assets/*/textures`,
/// summed so the directory order does not matter.
pub(crate) fn native_pt_fingerprint(root: &Path) -> Option<u64> {
    fn walk(dir: &Path, base: &Path, sum: &mut u64) -> Option<()> {
        for entry in fs::read_dir(dir).ok()? {
            let entry = entry.ok()?;
            let meta = entry.metadata().ok()?;
            if meta.is_dir() {
                walk(&entry.path(), base, sum)?;
                continue;
            }
            let mut hasher = DefaultHasher::new();
            entry.path().strip_prefix(base).ok()?.hash(&mut hasher);
            meta.len().hash(&mut hasher);
            meta.modified().ok()?.hash(&mut hasher);
            *sum = sum.wrapping_add(hasher.finish());
        }
        Some(())
    }
    let mut sum = 0x4f46464c49544531_u64;
    for namespace in fs::read_dir(root.join("assets")).ok()? {
        let namespace = namespace.ok()?.path();
        let textures = namespace.join("textures");
        if textures.is_dir() {
            walk(&textures, root, &mut sum)?;
        }
        let ctm = namespace.join("optifine/ctm");
        if ctm.is_dir() {
            walk(&ctm, root, &mut sum)?;
        }
    }
    Some(sum)
}
impl Sprites {
    pub(super) fn native_pt_load(&mut self, root: &Path, name: &str) -> Result<u32> {
        let decoded = Self::native_pt_decode(root, name)?;
        self.native_pt_register(name, decoded, !crate::pbr::has_maps(root, name))
    }
    pub(super) fn native_pt_lay_out(&mut self, pixels: bool) {
        let layers = self.native.len().max(1) as u64;
        let largest = self.native.iter().map(|n| n.0).max().unwrap_or(SPRITE_SIZE);
        self.minimum_size = self.native.iter().map(|n| n.0).min().unwrap_or(SPRITE_SIZE);
        let mut size = largest.max(SPRITE_SIZE).next_power_of_two();
        let budget = if self.layers.keys().any(|n| n.contains(":optifine/ctm/")) {
            768 << 20
        } else {
            MAX_ARRAY_BYTES
        };
        while size > SPRITE_SIZE && layers * u64::from(size * size) * 4 > budget {
            size /= 2;
        }
        if !pixels {
            self.pixels = Vec::new();
            self.animations = Vec::new();
            self.size = size;
            return;
        }
        let mut resized = Vec::with_capacity(self.native.len());
        self.pixels.clear();
        for (from, frames, initial) in &self.native {
            let frames: Arc<Vec<Vec<u8>>> = Arc::new(
                frames.iter().map(|f| resample(f, *from, size)).collect(),
            );
            self.pixels.extend_from_slice(&frames[*initial]);
            resized.push(frames);
        }
        for animation in &mut self.animations {
            animation.frames = resized[animation.layer as usize].clone();
        }
        self.size = size;
    }
    /// Read and decode one texture and its `.mcmeta`.
    pub(crate) fn native_pt_decode(
        root: &Path,
        name: &str,
    ) -> Result<crate::sprites::native_pt_assets::Decoded> {
        let path = resource_path(root, name).ok_or("invalid sprite identifier")?;
        let metadata = match fs::read_to_string(path.with_extension("png.mcmeta")) {
            Ok(text) => {
                Some(
                    serde_json::from_str::<
                        BTreeMap<String, Box<serde_json::value::RawValue>>,
                    >(&text)?,
                )
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let animation_text = metadata
            .as_ref()
            .and_then(|v| v.get("animation"))
            .map(|v| v.get().to_owned());
        let animation = animation_text
            .as_deref()
            .map(AnimationMetadata::from_json)
            .transpose()?;
        let mut decoder = png::Decoder::new(BufReader::new(File::open(path)?));
        decoder
            .set_transformations(
                png::Transformations::EXPAND | png::Transformations::STRIP_16,
            );
        let mut reader = decoder.read_info()?;
        let mut bytes = vec![
            0; reader.output_buffer_size().ok_or("sprite exceeds decoder limits") ?
        ];
        let info = reader.next_frame(&mut bytes)?;
        let (width, height) = if let Some(animation) = &animation {
            let [width, height] = animation
                .frame_size(i32::try_from(info.width)?, i32::try_from(info.height)?);
            (u32::try_from(width)?, u32::try_from(height)?)
        } else {
            (info.width, info.height)
        };
        if width == 0 || height == 0 || info.width % width != 0
            || info.height % height != 0
        {
            return Err(format!("sprite {name} has invalid frame dimensions").into());
        }
        let channels = info.color_type.samples();
        let mut pixels = Vec::with_capacity(
            info.width as usize * info.height as usize * 4,
        );
        for pixel in bytes[..info.buffer_size()].chunks_exact(channels) {
            let rgba = match info.color_type {
                png::ColorType::Rgba => [pixel[0], pixel[1], pixel[2], pixel[3]],
                png::ColorType::Rgb => [pixel[0], pixel[1], pixel[2], 255],
                png::ColorType::Grayscale => [pixel[0], pixel[0], pixel[0], 255],
                png::ColorType::GrayscaleAlpha => {
                    [pixel[0], pixel[0], pixel[0], pixel[1]]
                }
                _ => return Err("unsupported expanded sprite format".into()),
            };
            pixels.extend(rgba);
        }
        let columns = info.width / width;
        let count = columns * (info.height / height);
        let side = width.max(height);
        let mut frames = Vec::new();
        for index in 0..count {
            let x = index % columns * width;
            let y = index / columns * height;
            let mut frame = Vec::with_capacity((side * side * 4) as usize);
            for row in 0..side {
                let source_row = y + row * height / side;
                for column in 0..side {
                    let offset = ((source_row * info.width + x + column * width / side)
                        * 4) as usize;
                    frame.extend_from_slice(&pixels[offset..offset + 4]);
                }
            }
            frames.push(frame);
        }
        let texture = texture_section(metadata.as_ref());
        Ok(crate::sprites::native_pt_assets::Decoded {
            side,
            frames,
            animation: animation_text,
            strategy: texture
                .as_ref()
                .and_then(|t| t["mipmap_strategy"].as_str())
                .and_then(crate::mipmap::Strategy::parse)
                .unwrap_or_default(),
            alpha_cutoff_bias: texture
                .as_ref()
                .and_then(|t| t["alpha_cutoff_bias"].as_f64())
                .unwrap_or(0.0) as f32,
            has_transparent: pixels.as_chunks::<4>().0.iter().any(|p| p[3] == 0),
            opaque: pixels.as_chunks::<4>().0.iter().all(|p| p[3] == 255),
            translucent: pixels
                .as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[3] != 0 && p[3] != 255),
        })
    }
    /// Give a decoded texture its layer (or the one an identical still has).
    pub(super) fn native_pt_register(
        &mut self,
        name: &str,
        decoded: crate::sprites::native_pt_assets::Decoded,
        deduplicate: bool,
    ) -> Result<u32> {
        let crate::sprites::native_pt_assets::Decoded {
            side,
            frames,
            animation: animation_text,
            strategy,
            alpha_cutoff_bias,
            has_transparent,
            opaque,
            translucent,
        } = decoded;
        let animation = animation_text
            .as_deref()
            .map(AnimationMetadata::from_json)
            .transpose()?;
        let count = frames.len() as u32;
        let layer = u32::try_from(self.native.len())?;
        let settings = crate::mipmap::Settings {
            native_size: side,
            strategy,
            alpha_cutoff_bias,
            has_transparent,
            item: name.split_once(':').is_some_and(|(_, p)| p.starts_with("item/")),
        };
        let still = (deduplicate && animation.is_none() && frames.len() == 1)
            .then(|| {
                (
                    frames[0].clone(),
                    side,
                    [settings.strategy as u32, settings.alpha_cutoff_bias.to_bits()],
                    [settings.has_transparent, settings.item, opaque, translucent],
                )
            });
        if let Some(&same) = still.as_ref().and_then(|k| self.still.get(k)) {
            self.layers.insert(name.to_owned(), same);
            return Ok(same);
        }
        if let Some(key) = still {
            self.still.insert(key, layer);
        }
        let frames = Arc::new(frames);
        let mut initial = 0;
        if let Some(clip) = animation
            .as_ref()
            .map(|m| m.clip(count))
            .transpose()?
            .flatten()
        {
            initial = clip.sample(0).frame as usize;
            self.animations
                .push(SpriteAnimation {
                    layer,
                    frames: frames.clone(),
                    clip: Arc::new(clip),
                });
        }
        self.mips.push(settings);
        self.opaque.push(opaque);
        self.translucent.push(translucent);
        self.layers.insert(name.to_owned(), layer);
        self.native.push((side, frames, initial));
        Ok(layer)
    }
}
